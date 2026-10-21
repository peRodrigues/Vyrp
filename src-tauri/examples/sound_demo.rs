//! Toca os sons do Vyrp para ouvir (V19).
//!
//! ```text
//! cargo run --example sound_demo                     # início, fim, erro e cancelar
//! cargo run --example sound_demo -- --volume 0.3     # mais baixo (0..1)
//! cargo run --example sound_demo -- --wav pasta      # grava os 4 sons em WAV
//! cargo run --example sound_demo -- --media mute     # silencia 3 s e restaura (pause|mute|duck)
//! cargo run --example sound_demo -- --fx             # sequência completa do ditado (Fx)
//! ```
//!
//! O módulo `sound` é incluído por caminho, então o exemplo roda mesmo antes de
//! `pub mod sound;` entrar no `lib.rs`.

#[path = "../src/sound/mod.rs"]
#[allow(dead_code, unused_imports)]
mod sound;

use std::time::Duration;

use sound::media::{MediaBackend, MediaGuard, MediaMode, MediaOptions};
use sound::{Fx, FxParams, SoundId, SoundParams, SoundPlayer};

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let volume = arg_value(&args, "--volume")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.6);

    if let Some(dir) = arg_value(&args, "--wav") {
        std::fs::create_dir_all(&dir).expect("criar a pasta");
        for id in SoundId::ALL {
            let path = format!("{dir}/{id:?}.wav").to_lowercase();
            std::fs::write(&path, sound::synth::wav_bytes(id, 44_100)).expect("gravar o WAV");
            println!("{path} ({} ms)", sound::synth::duration_ms(id));
        }
        return;
    }

    if let Some(mode) = arg_value(&args, "--media") {
        media_demo(&mode);
        return;
    }

    let params = SoundParams {
        enabled: true,
        volume,
    };

    if args.iter().any(|a| a == "--fx") {
        let fx = Fx::start(
            FxParams {
                sound: params,
                ..Default::default()
            },
            None,
        );
        std::thread::sleep(Duration::from_millis(400));
        println!("início");
        fx.on_start();
        std::thread::sleep(Duration::from_millis(1_500));
        println!("fim");
        fx.on_end();
        std::thread::sleep(Duration::from_millis(900));
        println!("erro");
        fx.on_error();
        std::thread::sleep(Duration::from_millis(900));
        println!("cancelar");
        fx.on_cancel();
        std::thread::sleep(Duration::from_millis(900));
        return;
    }

    let player = SoundPlayer::start(params);
    std::thread::sleep(Duration::from_millis(400)); // abre o dispositivo
    println!("volume {volume}");
    for id in SoundId::ALL {
        println!("{id:?}");
        let started = std::time::Instant::now();
        player.play(id);
        println!("  play() voltou em {:?}", started.elapsed());
        std::thread::sleep(Duration::from_millis(
            u64::from(sound::synth::duration_ms(id)) + 700,
        ));
    }
}

fn media_demo(mode: &str) {
    let mode = match mode {
        "pause" => MediaMode::Pause,
        "mute" => MediaMode::Mute,
        "duck" => MediaMode::Duck,
        other => {
            eprintln!("modo desconhecido: {other} (use pause, mute ou duck)");
            return;
        }
    };
    let mut backend = match sound::media::system::open() {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!("sem controle de mídia: {err}");
            return;
        }
    };
    println!(
        "antes: mudo={:?} volume={:?} tocando={:?}",
        backend.is_muted(),
        backend.volume(),
        backend.audio_playing()
    );
    let mut guard = MediaGuard::new(
        backend,
        MediaOptions {
            mode,
            duck_factor: 0.2,
        },
        None,
    );
    println!("alterando ({mode:?}) por 3 s…");
    println!("begin: {:?}", guard.begin());
    std::thread::sleep(Duration::from_secs(3));
    println!("end: {:?}", guard.end());
    drop(guard);
    if let Ok(mut after) = sound::media::system::open() {
        println!(
            "depois: mudo={:?} volume={:?}",
            after.is_muted(),
            after.volume()
        );
    }
}
