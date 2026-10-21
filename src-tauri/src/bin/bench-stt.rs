//! Benchmark do STT local (spec V9): `cargo run --release --features dev-tools --bin bench-stt -- <modelo.bin> <audio.wav> [idioma] [execuções]`.
//! Mostra o system_info do whisper.cpp (SIMD/GPU ativos), a carga e cada transcrição.
//!
//! Modo incremental (V9b): `bench-stt <modelo.bin> <audio.wav> <idioma> --incremental <silero.bin>`.
//! Compara soltar→texto do caminho de hoje (VAD + clipe inteiro) com o incremental, que
//! recebe o WAV em tempo real (blocos de 20 ms, como o microfone) e decodifica os trechos
//! fechados durante a "fala"; o relógio do soltar começa quando o último bloco chega.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vyrp_lib::audio::wav;
use vyrp_lib::core::types::{AudioClip, SttOptions};
use vyrp_lib::stt::incremental::{Incremental, SegmentDecoder};
use vyrp_lib::stt::whisper::{WhisperEngine, backend};
use vyrp_lib::vad::{Screened, SileroVad, VadConfig, VadService, screen};

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
    let mut args = std::env::args().skip(1);
    let (Some(model), Some(audio)) = (args.next(), args.next()) else {
        eprintln!(
            "uso: bench-stt <modelo.bin> <audio.wav> [idioma=pt] [execuções=3 | --incremental <silero.bin>]"
        );
        std::process::exit(2);
    };
    let lang = args.next().unwrap_or_else(|| "pt".into());
    let mode = args.next();
    let incremental = (mode.as_deref() == Some("--incremental")).then(|| args.next());
    let runs: usize = mode.and_then(|v| v.parse().ok()).unwrap_or(3);

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
    if let Some(silero) = incremental {
        let silero = silero.expect("caminho do modelo Silero");
        compare(engine, &clip, &opts, &silero);
        return;
    }
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

fn compare(engine: Arc<WhisperEngine>, clip: &AudioClip, opts: &SttOptions, silero: &str) {
    let vad = VadService::new(VadConfig::default());
    vad.set_silero(SileroVad::load(silero.as_ref(), VadConfig::default()).expect("Silero"));

    // Hoje: ao soltar, VAD + whisper no clipe inteiro.
    let t = Instant::now();
    let whole = match screen(&vad, clip).0 {
        Screened::NoSpeech { .. } => String::new(),
        Screened::Speech(compacted) => {
            engine
                .transcribe_blocking(&compacted, opts)
                .expect("transcrever")
                .text
        }
    };
    let before_ms = t.elapsed().as_millis();
    println!("ANTES  soltar→texto: {before_ms} ms\n  {whole}");

    // Incremental: o áudio chega em tempo real; soltar = depois do último bloco.
    let decoder = Arc::clone(&engine) as Arc<dyn SegmentDecoder>;
    let (session, tap) = Incremental::start(decoder, Arc::clone(&vad), opts.clone());
    let block = 320; // 20 ms a 16 kHz
    let start = Instant::now();
    for (i, chunk) in clip.pcm.chunks(block).enumerate() {
        let due = start + Duration::from_millis(20 * i as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        if tap.send(chunk.to_vec()).is_err() {
            println!("  (a decodificação incremental parou no bloco {i}; ver o log)");
            break;
        }
    }
    drop(tap);
    let t = Instant::now();
    let text = match session.finish(clip) {
        Some(result) => result.expect("transcrever").text,
        None => {
            println!("  (nada adiantado: caminho normal)");
            whole.clone()
        }
    };
    let after_ms = t.elapsed().as_millis();
    println!("DEPOIS soltar→texto: {after_ms} ms\n  {text}");
    println!(
        "RESUMO áudio={} ms antes={before_ms} ms depois={after_ms} ms",
        clip.duration_ms()
    );

    // Cancel: o whisper tem de parar no meio (o abort é pedido 300 ms depois de começar).
    let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&abort);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let t = Instant::now();
    let cancelled = engine.transcribe_continuing(
        clip,
        opts,
        &vyrp_lib::stt::whisper::Continuation {
            previous: String::new(),
            abort: Some(abort),
        },
    );
    println!(
        "CANCEL parou em {} ms: {:?}",
        t.elapsed().as_millis(),
        cancelled.map(|t| t.text.len())
    );
}
