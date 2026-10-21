//! Mixer dos sons: soma vozes (sons em andamento) num bloco de saída. Puro, sem dispositivo:
//! o stream de áudio só chama [`Mixer::mix_into`] a cada bloco.

use std::sync::Arc;

/// Teto de sons simultâneos; passou disso o mais antigo sai (alguém apertando o atalho
/// sem parar não vira uma parede de som).
pub const MAX_VOICES: usize = 8;

#[derive(Debug, Clone)]
struct Voice {
    samples: Arc<[f32]>,
    pos: usize,
    gain: f32,
}

#[derive(Debug, Default)]
pub struct Mixer {
    voices: Vec<Voice>,
}

impl Mixer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Começa a tocar `samples` (mono) com o ganho dado.
    pub fn push(&mut self, samples: Arc<[f32]>, gain: f32) {
        if samples.is_empty() || gain <= 0.0 {
            return;
        }
        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            samples,
            pos: 0,
            gain,
        });
    }

    pub fn active(&self) -> usize {
        self.voices.len()
    }

    pub fn is_idle(&self) -> bool {
        self.voices.is_empty()
    }

    /// Escreve o próximo bloco intercalado (`channels` canais com o mesmo sinal mono).
    /// `out` é sobrescrito. Vozes que terminam saem do mixer.
    pub fn mix_into(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        for voice in &mut self.voices {
            let remaining = voice.samples.len() - voice.pos;
            let n = remaining.min(frames);
            for i in 0..n {
                let value = voice.samples[voice.pos + i] * voice.gain;
                for ch in 0..channels {
                    out[i * channels + ch] += value;
                }
            }
            voice.pos += n;
        }
        self.voices.retain(|v| v.pos < v.samples.len());
        for v in out.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[f32]) -> Arc<[f32]> {
        values.to_vec().into()
    }

    #[test]
    fn silence_when_idle() {
        let mut m = Mixer::new();
        let mut out = [1.0f32; 8];
        m.mix_into(&mut out, 2);
        assert_eq!(out, [0.0; 8]);
        assert!(m.is_idle());
    }

    #[test]
    fn a_voice_plays_across_blocks_and_then_leaves() {
        let mut m = Mixer::new();
        m.push(samples(&[0.1, 0.2, 0.3, 0.4, 0.5]), 1.0);
        let mut block = [0.0f32; 3];
        m.mix_into(&mut block, 1);
        assert_eq!(block, [0.1, 0.2, 0.3]);
        m.mix_into(&mut block, 1);
        assert_eq!(block, [0.4, 0.5, 0.0]);
        assert!(m.is_idle());
    }

    #[test]
    fn gain_scales_and_mono_is_copied_to_every_channel() {
        let mut m = Mixer::new();
        m.push(samples(&[0.5, -0.5]), 0.5);
        let mut out = [0.0f32; 6];
        m.mix_into(&mut out, 3);
        assert_eq!(out, [0.25, 0.25, 0.25, -0.25, -0.25, -0.25]);
    }

    #[test]
    fn overlapping_voices_sum_and_clip() {
        let mut m = Mixer::new();
        m.push(samples(&[0.4, 0.4]), 1.0);
        m.push(samples(&[0.3, 0.9]), 1.0);
        let mut out = [0.0f32; 2];
        m.mix_into(&mut out, 1);
        assert!((out[0] - 0.7).abs() < 1e-6);
        assert_eq!(out[1], 1.0, "soma 1.3 deve ser cortada em 1.0");
    }

    #[test]
    fn empty_or_silent_pushes_are_ignored_and_voices_are_capped() {
        let mut m = Mixer::new();
        m.push(samples(&[]), 1.0);
        m.push(samples(&[0.5]), 0.0);
        assert!(m.is_idle());
        for _ in 0..(MAX_VOICES + 5) {
            m.push(samples(&[0.1; 100]), 1.0);
        }
        assert_eq!(m.active(), MAX_VOICES);
    }

    #[test]
    fn odd_buffer_lengths_do_not_panic() {
        let mut m = Mixer::new();
        m.push(samples(&[0.1; 10]), 1.0);
        let mut out = [0.0f32; 7]; // 7 não é múltiplo de 2 canais
        m.mix_into(&mut out, 2);
        assert_eq!(&out[..6], &[0.1; 6]);
        assert_eq!(out[6], 0.0);
    }
}
