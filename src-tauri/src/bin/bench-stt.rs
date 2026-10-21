//! Benchmark do STT local (spec V9): `cargo run --release --features dev-tools --bin bench-stt -- <modelo.bin> <audio.wav> [idioma] [execuções]`.
//! Mostra o system_info do whisper.cpp (SIMD/GPU ativos), a carga e cada transcrição.

use std::time::Instant;

use vyrp_lib::audio::wav;
use vyrp_lib::core::types::{AudioClip, SttOptions};
use vyrp_lib::stt::whisper::{WhisperEngine, backend};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(model), Some(audio)) = (args.next(), args.next()) else {
        eprintln!("uso: bench-stt <modelo.bin> <audio.wav> [idioma=pt] [execuções=3]");
        std::process::exit(2);
    };
    let lang = args.next().unwrap_or_else(|| "pt".into());
    let runs: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(3);

    let bytes = std::fs::read(&audio).expect("ler o WAV");
    let clip = AudioClip::new(wav::decode_to_16k_mono(&bytes).expect("WAV PCM 16-bit"));
    println!("backend compilado: {}", backend());
    println!("system_info: {}", whisper_rs::print_system_info());
    println!("áudio: {} ms", clip.duration_ms());

    let engine = WhisperEngine::new(|_| {});
    let load_ms = engine
        .load_blocking("bench", model.as_ref())
        .expect("carregar o modelo");
    println!("carga + aquecimento: {load_ms} ms");

    let opts = SttOptions {
        languages: vec![lang],
        vocabulary: Vec::new(),
    };
    for run in 1..=runs {
        let t = Instant::now();
        let out = engine
            .transcribe_blocking(&clip, &opts)
            .expect("transcrever");
        let ms = t.elapsed().as_millis();
        println!(
            "execução {run}: {ms} ms (RTF {:.2}) [{}] {}",
            ms as f64 / clip.duration_ms() as f64,
            out.language.unwrap_or_default(),
            out.text
        );
    }
}
