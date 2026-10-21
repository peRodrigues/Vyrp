//! VAD de reserva por energia (Rust puro): usado enquanto o modelo Silero não está
//! disponível. Quadros de 30 ms; fala = energia acima do piso de ruído adaptativo.

use std::ops::Range;

use super::{VadConfig, smooth};
use crate::core::types::SAMPLE_RATE;

const FRAME: usize = SAMPLE_RATE as usize * 30 / 1000;
/// Fala precisa estar este tanto acima do ruído de fundo...
const MARGIN_DB: f32 = 12.0;
/// ...e nunca abaixo deste nível absoluto (silêncio de sala com ganho alto).
const FLOOR_DBFS: f32 = -48.0;

fn dbfs(frame: &[i16]) -> f32 {
    let rms = crate::audio::dsp::rms(frame).max(1e-6);
    20.0 * rms.log10()
}

/// Trechos de fala (índices de amostra), já suavizados pelas regras de [`VadConfig`].
pub fn segments(pcm: &[i16], cfg: &VadConfig) -> Vec<Range<usize>> {
    let energies: Vec<f32> = pcm.chunks(FRAME).map(dbfs).collect();
    if energies.is_empty() {
        return Vec::new();
    }
    // Piso de ruído = percentil 10 da energia dos quadros.
    let mut sorted = energies.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let noise = sorted[sorted.len() / 10];
    let threshold = (noise + MARGIN_DB).max(FLOOR_DBFS);
    let raw: Vec<Range<usize>> = energies
        .iter()
        .enumerate()
        .filter(|(_, e)| **e > threshold)
        .map(|(i, _)| i * FRAME..((i + 1) * FRAME).min(pcm.len()))
        .collect();
    smooth(raw, pcm.len(), cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize, amp: f32) -> Vec<i16> {
        (0..SAMPLE_RATE as usize * ms / 1000)
            .map(|i| (amp * 32_767.0 * (i as f32 * 0.12).sin()) as i16)
            .collect()
    }

    fn noise(ms: usize, amp: i16) -> Vec<i16> {
        // Ruído determinístico pequeno (LCG), como o chiado de um mic em sala silenciosa.
        let mut x: u32 = 12_345;
        (0..SAMPLE_RATE as usize * ms / 1000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (x >> 16) as i16 % amp.max(1)
            })
            .collect()
    }

    #[test]
    fn quiet_room_has_no_speech() {
        let pcm = noise(5_000, 20);
        assert!(segments(&pcm, &VadConfig::default()).is_empty());
        assert!(segments(&[], &VadConfig::default()).is_empty());
    }

    #[test]
    fn finds_speech_burst_with_padding() {
        let mut pcm = noise(1_000, 20);
        pcm.extend(tone(800, 0.3));
        pcm.extend(noise(1_500, 20));
        let segs = segments(&pcm, &VadConfig::default());
        assert_eq!(segs.len(), 1, "{segs:?}");
        let s = &segs[0];
        let ms = |i: usize| i * 1000 / SAMPLE_RATE as usize;
        assert!(
            (ms(s.start) as i64 - 1_000).abs() <= 260,
            "início {}",
            ms(s.start)
        );
        assert!((ms(s.end) as i64 - 1_800).abs() <= 260, "fim {}", ms(s.end));
    }

    #[test]
    fn short_click_is_not_speech() {
        let mut pcm = noise(1_000, 20);
        pcm.extend(tone(60, 0.5));
        pcm.extend(noise(1_000, 20));
        assert!(segments(&pcm, &VadConfig::default()).is_empty());
    }
}
