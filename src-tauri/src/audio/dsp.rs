//! Processamento puro do áudio capturado: mono, reamostragem para 16 kHz, i16,
//! nível, detecção de silêncio digital e limite de duração. Sem I/O, testável.

use std::f64::consts::PI;

use crate::core::types::SAMPLE_RATE;

/// Média dos canais de um buffer intercalado.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32),
    );
}

pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

/// Reamostrador em fluxo por sinc janelada (Blackman). Suficiente para voz: corte em
/// 90% do Nyquist menor; ~32 taps por amostra de saída.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Passo na entrada por amostra de saída (in_rate / out_rate).
    step: f64,
    cutoff: f64,
    half_taps: usize,
    /// Amostras de entrada ainda necessárias (janela deslizante).
    history: Vec<f32>,
    /// Posição (em amostras de `history`) da próxima saída.
    pos: f64,
    passthrough: bool,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = f64::from(in_rate) / f64::from(out_rate);
        let cutoff = 0.9 * (1.0_f64).min(1.0 / step);
        let half_taps = (16.0 * step.max(1.0)).ceil() as usize;
        Self {
            step,
            cutoff,
            half_taps,
            history: vec![0.0; half_taps],
            pos: half_taps as f64,
            passthrough: in_rate == out_rate,
        }
    }

    pub fn to_16k(in_rate: u32) -> Self {
        Self::new(in_rate, SAMPLE_RATE)
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        self.history.extend_from_slice(input);
        let ht = self.half_taps as f64;
        while self.pos + ht < self.history.len() as f64 {
            let center = self.pos.floor() as isize;
            let frac = self.pos - center as f64;
            let mut acc = 0.0f64;
            let mut norm = 0.0f64;
            for k in -(self.half_taps as isize) + 1..=self.half_taps as isize {
                let idx = center + k;
                let x = k as f64 - frac;
                let w = kernel(x, self.cutoff, ht);
                acc += w * f64::from(self.history[idx as usize]);
                norm += w;
            }
            out.push(if norm.abs() > 1e-9 {
                (acc / norm) as f32
            } else {
                0.0
            });
            self.pos += self.step;
        }
        // Descarta o que não será mais usado, mantendo a janela.
        let keep_from = (self.pos.floor() as usize).saturating_sub(self.half_taps);
        if keep_from > 0 {
            self.history.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
    }
}

fn kernel(x: f64, cutoff: f64, half_width: f64) -> f64 {
    if x.abs() >= half_width {
        return 0.0;
    }
    let sinc = if x == 0.0 {
        cutoff
    } else {
        (PI * cutoff * x).sin() / (PI * x)
    };
    // Janela de Blackman centrada em 0 com largura 2·half_width.
    let n = (x + half_width) / (2.0 * half_width);
    let window = 0.42 - 0.5 * (2.0 * PI * n).cos() + 0.08 * (4.0 * PI * n).cos();
    sinc * window
}

/// RMS normalizado (0..=1) de PCM i16.
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s) / f64::from(i16::MAX);
            v * v
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Junta amostras e devolve o RMS a cada janela (~30 Hz para o overlay).
#[derive(Debug, Default)]
pub struct LevelMeter {
    window: usize,
    buf: Vec<i16>,
}

impl LevelMeter {
    pub fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
            buf: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[i16], mut on_level: impl FnMut(f32)) {
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == self.window {
                on_level(rms(&self.buf));
                self.buf.clear();
            }
        }
    }
}

/// Aviso de silêncio digital contínuo (mic mudo no hardware, driver travado).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceAlert {
    /// 5 s sem nenhum sinal.
    Warn,
    /// 15 s sem nenhum sinal.
    Escalate,
}

/// Conta amostras seguidas de "zero digital" (|amostra| ≤ 2). Fala baixa não conta:
/// só silêncio absoluto, que nenhum microfone real produz.
#[derive(Debug, Default)]
pub struct SilenceWatch {
    silent: usize,
    warned: bool,
    escalated: bool,
}

const WARN_SAMPLES: usize = 5 * SAMPLE_RATE as usize;
const ESCALATE_SAMPLES: usize = 15 * SAMPLE_RATE as usize;

impl SilenceWatch {
    pub fn push(&mut self, samples: &[i16]) -> Option<SilenceAlert> {
        for &s in samples {
            if s.unsigned_abs() > 2 {
                self.silent = 0;
            } else {
                self.silent += 1;
            }
        }
        if self.silent >= ESCALATE_SAMPLES && !self.escalated {
            self.escalated = true;
            self.warned = true;
            return Some(SilenceAlert::Escalate);
        }
        if self.silent >= WARN_SAMPLES && !self.warned {
            self.warned = true;
            return Some(SilenceAlert::Warn);
        }
        None
    }
}

/// Limite de gravação: aviso aos 19 min, corte aos 20 min.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationAlert {
    AlmostFull,
    Full,
}

pub const MAX_RECORDING_SECS: usize = 20 * 60;
const WARN_RECORDING_SECS: usize = 19 * 60;

/// Buffer de um ditado com o limite de duração.
#[derive(Debug, Default)]
pub struct Recording {
    pub pcm: Vec<i16>,
    warned: bool,
}

impl Recording {
    pub fn with_preroll(preroll: Vec<i16>) -> Self {
        Self {
            pcm: preroll,
            warned: false,
        }
    }

    /// Acrescenta até o limite; devolve o aviso quando cruza 19 min ou chega a 20 min.
    pub fn push(&mut self, samples: &[i16]) -> Option<DurationAlert> {
        let max = MAX_RECORDING_SECS * SAMPLE_RATE as usize;
        let room = max.saturating_sub(self.pcm.len());
        self.pcm
            .extend_from_slice(&samples[..samples.len().min(room)]);
        if self.pcm.len() >= max {
            return Some(DurationAlert::Full);
        }
        if !self.warned && self.pcm.len() >= WARN_RECORDING_SECS * SAMPLE_RATE as usize {
            self.warned = true;
            return Some(DurationAlert::AlmostFull);
        }
        None
    }
}

/// Anel com os últimos N ms (pre-roll do "mic quente").
#[derive(Debug, Default)]
pub struct PreRoll {
    cap: usize,
    buf: std::collections::VecDeque<i16>,
}

impl PreRoll {
    pub fn new(ms: u32) -> Self {
        Self {
            cap: (SAMPLE_RATE as usize * ms as usize) / 1000,
            buf: std::collections::VecDeque::new(),
        }
    }

    pub fn push(&mut self, samples: &[i16]) {
        if self.cap == 0 {
            return;
        }
        self.buf.extend(samples);
        let extra = self.buf.len().saturating_sub(self.cap);
        self.buf.drain(..extra);
    }

    pub fn take(&mut self) -> Vec<i16> {
        self.buf.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, secs: f64) -> Vec<f32> {
        (0..(f64::from(rate) * secs) as usize)
            .map(|i| (0.5 * (2.0 * PI * freq * i as f64 / f64::from(rate)).sin()) as f32)
            .collect()
    }

    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn resample_48k_keeps_length_and_pitch() {
        let input = sine(440.0, 48_000, 1.0);
        let mut r = Resampler::to_16k(48_000);
        let mut out = Vec::new();
        // Em blocos irregulares, como chegam do driver.
        for chunk in input.chunks(480 + 7) {
            r.process(chunk, &mut out);
        }
        assert!((out.len() as i64 - 16_000).abs() < 64, "len {}", out.len());
        let zc = zero_crossings(&out[200..out.len() - 200]) as f64;
        let expected = 2.0 * 440.0 * (out.len() - 400) as f64 / 16_000.0;
        assert!((zc - expected).abs() < 4.0, "zc {zc} vs {expected}");
        let peak = out[200..out.len() - 200]
            .iter()
            .fold(0f32, |m, &x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.03, "peak {peak}");
    }

    #[test]
    fn resample_44k1_fractional_ratio() {
        let input = sine(1_000.0, 44_100, 0.5);
        let mut r = Resampler::to_16k(44_100);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert!((out.len() as i64 - 8_000).abs() < 64, "len {}", out.len());
        let zc = zero_crossings(&out[100..out.len() - 100]) as f64;
        let expected = 2.0 * 1_000.0 * (out.len() - 200) as f64 / 16_000.0;
        assert!((zc - expected).abs() < 4.0);
    }

    #[test]
    fn resample_removes_content_above_nyquist() {
        // 12 kHz não cabe em 16 kHz (Nyquist 8 kHz): tem de sumir, não virar alias.
        let input = sine(12_000.0, 48_000, 0.5);
        let mut r = Resampler::to_16k(48_000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let pcm: Vec<i16> = out[200..].iter().map(|&x| to_i16(x)).collect();
        assert!(rms(&pcm) < 0.01, "rms {}", rms(&pcm));
    }

    #[test]
    fn passthrough_at_16k() {
        let mut r = Resampler::to_16k(16_000);
        let mut out = Vec::new();
        r.process(&[0.1, 0.2], &mut out);
        assert_eq!(out, vec![0.1, 0.2]);
    }

    #[test]
    fn level_meter_emits_per_window() {
        let mut m = LevelMeter::new(4);
        let mut levels = Vec::new();
        m.push(&[i16::MAX; 10], |l| levels.push(l));
        assert_eq!(levels.len(), 2);
        assert!((levels[0] - 1.0).abs() < 1e-3);
        assert!(rms(&[0; 8]) == 0.0);
    }

    #[test]
    fn silence_watch_warns_then_escalates_once() {
        let mut w = SilenceWatch::default();
        let second = vec![0i16; SAMPLE_RATE as usize];
        let alerts: Vec<_> = (0..16).filter_map(|_| w.push(&second)).collect();
        assert_eq!(alerts, vec![SilenceAlert::Warn, SilenceAlert::Escalate]);
        // Qualquer sinal real zera a contagem.
        let mut w = SilenceWatch::default();
        for _ in 0..4 {
            w.push(&second);
        }
        w.push(&[500]);
        assert_eq!(w.push(&second), None);
    }

    #[test]
    fn recording_caps_at_twenty_minutes() {
        let mut rec = Recording::default();
        let minute = vec![1i16; 60 * SAMPLE_RATE as usize];
        let mut alerts = Vec::new();
        for _ in 0..21 {
            if let Some(a) = rec.push(&minute) {
                alerts.push(a);
            }
        }
        assert_eq!(alerts[0], DurationAlert::AlmostFull);
        assert_eq!(*alerts.last().unwrap(), DurationAlert::Full);
        assert_eq!(rec.pcm.len(), MAX_RECORDING_SECS * SAMPLE_RATE as usize);
    }

    #[test]
    fn preroll_keeps_only_last_window() {
        let mut p = PreRoll::new(1); // 16 amostras
        p.push(&(0..40).collect::<Vec<i16>>());
        let kept = p.take();
        assert_eq!(kept.len(), 16);
        assert_eq!(kept[0], 24);
        assert!(p.take().is_empty());
        let mut off = PreRoll::new(0);
        off.push(&[1, 2, 3]);
        assert!(off.take().is_empty());
    }
}
