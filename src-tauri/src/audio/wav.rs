//! WAV PCM 16-bit mono (diagnóstico e histórico de áudio).

use std::path::Path;

pub fn encode(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // tamanho do bloco fmt
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // bytes/s
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes por quadro
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn write(path: &Path, pcm: &[i16], sample_rate: u32) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, encode(pcm, sample_rate))
}

/// Lê um WAV PCM 16-bit (mono ou estéreo) e devolve mono 16 kHz.
pub fn decode_to_16k_mono(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("não é um arquivo WAV".into());
    }
    let (mut channels, mut rate, mut bits) = (0u16, 0u32, 0u16);
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            let format = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
            if format != 1 && format != 0xFFFE {
                return Err(format!("WAV não-PCM (formato {format})"));
            }
        } else if id == b"data" {
            if bits != 16 || channels == 0 {
                return Err(format!(
                    "WAV precisa ser PCM 16-bit (veio {bits} bits, {channels} canais)"
                ));
            }
            let floats: Vec<f32> = body
                .chunks_exact(2)
                .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / f32::from(i16::MAX))
                .collect();
            let mut mono = Vec::new();
            super::dsp::downmix(&floats, usize::from(channels), &mut mono);
            let mut out = Vec::new();
            super::dsp::Resampler::to_16k(rate).process(&mono, &mut out);
            return Ok(out.into_iter().map(super::dsp::to_i16).collect());
        }
        pos += 8 + len + (len & 1);
    }
    Err("WAV sem bloco de dados".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_round_trips_16k_mono() {
        let pcm: Vec<i16> = (0..1_000)
            .map(|i| (i * 13 % 2_000) as i16 - 1_000)
            .collect();
        let back = decode_to_16k_mono(&encode(&pcm, 16_000)).unwrap();
        assert_eq!(back.len(), pcm.len());
        assert!(back.iter().zip(&pcm).all(|(a, b)| (a - b).abs() <= 1));
        assert!(decode_to_16k_mono(b"nada").is_err());
    }

    #[test]
    fn header_and_payload() {
        let bytes = encode(&[1, -1, 256], 16_000);
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            16_000
        );
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 1);
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), -1);
    }
}
