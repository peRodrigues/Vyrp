//! Síntese dos sons do Vyrp (V19). Tudo gerado aqui, do zero: nenhum arquivo de áudio,
//! nada baixado ou copiado. Cada som é uma sequência de notas (seno com um toque de
//! harmônicos) com ataque curto e queda exponencial, sempre terminando em zero para não
//! estalar. Determinístico: o mesmo `SoundId` e a mesma taxa dão exatamente as mesmas amostras.

use std::f32::consts::TAU;

use super::SoundId;

/// Pico de cada som com ganho 1.0 (deixa folga para o volume e para somar sons).
pub const PEAK: f32 = 0.35;
const ATTACK_MS: f32 = 4.0;
const RELEASE_MS: f32 = 10.0;

/// Uma nota: frequência inicial/final (glissando se diferentes), duração e "brilho"
/// (peso dos harmônicos 2 e 3).
#[derive(Debug, Clone, Copy)]
struct Note {
    start_ms: f32,
    dur_ms: f32,
    from_hz: f32,
    to_hz: f32,
    amp: f32,
    /// Constante de tempo da queda; menor = som mais seco.
    decay_ms: f32,
    harmonics: [f32; 2],
}

impl Note {
    fn tone(start_ms: f32, dur_ms: f32, hz: f32, amp: f32, decay_ms: f32) -> Self {
        Self {
            start_ms,
            dur_ms,
            from_hz: hz,
            to_hz: hz,
            amp,
            decay_ms,
            harmonics: [0.22, 0.08],
        }
    }

    fn glide(
        start_ms: f32,
        dur_ms: f32,
        from_hz: f32,
        to_hz: f32,
        amp: f32,
        decay_ms: f32,
    ) -> Self {
        Self {
            from_hz,
            to_hz,
            ..Self::tone(start_ms, dur_ms, from_hz, amp, decay_ms)
        }
    }

    /// Timbre mais áspero (harmônicos ímpares fortes): para o erro.
    fn buzzy(mut self) -> Self {
        self.harmonics = [0.55, 0.35];
        self
    }
}

fn notes(id: SoundId) -> Vec<Note> {
    match id {
        // Sobe: "pode falar".
        SoundId::Start => vec![
            Note::tone(0.0, 80.0, 880.0, 0.9, 60.0),
            Note::tone(60.0, 130.0, 1318.5, 1.0, 70.0),
        ],
        // Desce: "recebi".
        SoundId::End => vec![
            Note::tone(0.0, 80.0, 1174.7, 0.9, 60.0),
            Note::tone(60.0, 130.0, 783.99, 1.0, 70.0),
        ],
        // Dois zumbidos graves: "algo deu errado".
        SoundId::Error => vec![
            Note::tone(0.0, 100.0, 233.1, 1.0, 90.0).buzzy(),
            Note::tone(150.0, 130.0, 196.0, 1.0, 110.0).buzzy(),
        ],
        // Um deslize curto e macio para baixo: "esquece".
        SoundId::Cancel => vec![Note::glide(0.0, 120.0, 700.0, 420.0, 0.9, 55.0)],
    }
}

fn total_ms(notes: &[Note]) -> f32 {
    notes
        .iter()
        .map(|n| n.start_ms + n.dur_ms)
        .fold(0.0, f32::max)
}

/// Duração do som, em milissegundos (independe da taxa).
pub fn duration_ms(id: SoundId) -> u32 {
    total_ms(&notes(id)).ceil() as u32
}

/// Gera as amostras (mono, f32 em -1..1) na taxa dada.
pub fn render(id: SoundId, sample_rate: u32) -> Vec<f32> {
    let notes = notes(id);
    let rate = sample_rate.max(8_000) as f32;
    let len = (total_ms(&notes) / 1000.0 * rate).ceil() as usize + 1;
    let mut out = vec![0.0f32; len];
    for note in &notes {
        let first = (note.start_ms / 1000.0 * rate) as usize;
        let count = (note.dur_ms / 1000.0 * rate) as usize;
        let attack = (ATTACK_MS / 1000.0 * rate).max(1.0);
        let release = (RELEASE_MS / 1000.0 * rate).max(1.0);
        let decay = (note.decay_ms / 1000.0 * rate).max(1.0);
        let mut phase = 0.0f32;
        for i in 0..count {
            let Some(slot) = out.get_mut(first + i) else {
                break;
            };
            let progress = i as f32 / count.max(1) as f32;
            let hz = note.from_hz + (note.to_hz - note.from_hz) * progress;
            phase = (phase + TAU * hz / rate) % TAU;
            let wave = phase.sin()
                + note.harmonics[0] * (2.0 * phase).sin()
                + note.harmonics[1] * (3.0 * phase).sin();
            let t = i as f32;
            let fade_in = (t / attack).min(1.0);
            let fade_out = ((count as f32 - t) / release).clamp(0.0, 1.0);
            let envelope = (-t / decay).exp() * fade_in * fade_out;
            *slot += wave * envelope * note.amp;
        }
    }
    // Normaliza para o pico combinado e termina exatamente em zero.
    let max = out.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    for v in &mut out {
        *v = *v / max * PEAK;
    }
    if let Some(last) = out.last_mut() {
        *last = 0.0;
    }
    out
}

/// Amostras em i16 (para gravar um WAV e ouvir fora do app).
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect()
}

/// WAV PCM 16-bit mono.
pub fn wav_bytes(id: SoundId, sample_rate: u32) -> Vec<u8> {
    let pcm = to_i16(&render(id, sample_rate));
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn zero_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    fn ms(n: f32) -> usize {
        (n / 1000.0 * RATE as f32) as usize
    }

    #[test]
    fn rendering_is_deterministic_and_finite() {
        for id in SoundId::ALL {
            let a = render(id, RATE);
            let b = render(id, RATE);
            assert_eq!(a, b, "{id:?}");
            assert!(a.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn lengths_match_the_declared_duration() {
        for id in SoundId::ALL {
            let samples = render(id, RATE);
            let expected = duration_ms(id) as usize * RATE as usize / 1000;
            assert!(
                samples.len().abs_diff(expected) <= RATE as usize / 100,
                "{id:?}: {} amostras, esperado ~{expected}",
                samples.len()
            );
            assert!(
                duration_ms(id) >= 100 && duration_ms(id) <= 400,
                "{id:?} deve ser curto"
            );
        }
    }

    #[test]
    fn peak_is_controlled_and_there_are_no_clicks() {
        for id in SoundId::ALL {
            let samples = render(id, RATE);
            let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - PEAK).abs() < 1e-3, "{id:?}: pico {peak}");
            // Começa e termina em silêncio.
            assert!(samples[0].abs() < 0.01, "{id:?} começa estalando");
            assert_eq!(*samples.last().unwrap(), 0.0);
            // Sem saltos bruscos entre amostras vizinhas.
            let max_step = samples
                .windows(2)
                .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
            assert!(max_step < 0.2, "{id:?}: salto de {max_step}");
        }
    }

    #[test]
    fn each_sound_is_different() {
        let all: Vec<Vec<f32>> = SoundId::ALL.iter().map(|id| render(*id, RATE)).collect();
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j]);
            }
        }
    }

    #[test]
    fn start_rises_and_end_falls() {
        let start = render(SoundId::Start, RATE);
        let (a, b) = (&start[..ms(60.0)], &start[ms(100.0)..ms(180.0)]);
        assert!(
            zero_crossings(b) * a.len() > zero_crossings(a) * b.len(),
            "início deve subir"
        );

        let end = render(SoundId::End, RATE);
        let (a, b) = (&end[..ms(60.0)], &end[ms(100.0)..ms(180.0)]);
        assert!(
            zero_crossings(b) * a.len() < zero_crossings(a) * b.len(),
            "fim deve descer"
        );
    }

    #[test]
    fn error_is_lower_than_the_start_chime() {
        let rate_of = |id| {
            let s = render(id, RATE);
            zero_crossings(&s) as f32 / s.len() as f32
        };
        assert!(rate_of(SoundId::Error) < rate_of(SoundId::Start));
    }

    #[test]
    fn works_at_other_sample_rates() {
        for rate in [22_050, 44_100, 96_000] {
            let s = render(SoundId::Start, rate);
            let expected = duration_ms(SoundId::Start) as usize * rate as usize / 1000;
            assert!(s.len().abs_diff(expected) <= rate as usize / 100);
        }
    }

    #[test]
    fn wav_export_has_a_valid_header() {
        let bytes = wav_bytes(SoundId::Cancel, 44_100);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 44 + data_len);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            44_100
        );
    }

    #[test]
    fn i16_conversion_clamps() {
        assert_eq!(
            to_i16(&[0.0, 1.0, -1.0, 2.0, -2.0]),
            vec![0, 32767, -32767, 32767, -32767]
        );
    }
}
