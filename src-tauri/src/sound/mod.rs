//! Sons e mídia do ditado (V19).
//!
//! - [`SoundPlayer::play`]: toca início/fim/erro/cancelar sem bloquear quem chama. Os sons
//!   são sintetizados aqui ([`synth`]) e pré-renderizados na taxa do dispositivo; um stream
//!   de saída fica aberto, então tocar é só empurrar uma voz para o mixer (latência de
//!   um bloco de áudio).
//! - [`media`]: pausar/silenciar/baixar a mídia do sistema durante o ditado, com
//!   restauração garantida.
//! - [`Fx`]: junta os dois na ordem certa para o orquestrador (V11): `on_start`,
//!   `on_end`, `on_error`, `on_cancel`.
//!
//! O módulo é autônomo (não usa nada do resto do app): para ligar, `pub mod sound;` em
//! `lib.rs`. Volume e liga/desliga vêm de [`FxParams`]; veja os campos sugeridos para o
//! Settings em [`FxParams`].

pub mod media;
pub mod mixer;
pub mod synth;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use serde::{Deserialize, Serialize};

pub use media::{MediaController, MediaMode, MediaOptions};
use mixer::Mixer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundId {
    /// Começou a ouvir.
    Start,
    /// Terminou de ouvir (processando/colando).
    End,
    /// Algo falhou.
    Error,
    /// O usuário cancelou.
    Cancel,
}

impl SoundId {
    pub const ALL: [SoundId; 4] = [
        SoundId::Start,
        SoundId::End,
        SoundId::Error,
        SoundId::Cancel,
    ];
}

/// Parâmetros de som. **Campos sugeridos para o Settings** (hoje já existe `sounds: bool`):
/// `sounds: bool` → `enabled`, `sound_volume: f32` (0..1, padrão 0.6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundParams {
    pub enabled: bool,
    /// 0..1; vira ganho por uma curva quadrática (o ouvido é logarítmico).
    pub volume: f32,
}

impl Default for SoundParams {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 0.6,
        }
    }
}

/// Ganho a aplicar, ou `None` se o som não deve tocar (desligado ou volume zero).
pub fn gain_for(params: SoundParams) -> Option<f32> {
    if !params.enabled {
        return None;
    }
    let volume = params.volume.clamp(0.0, 1.0);
    let gain = volume * volume;
    (gain > 0.0005).then_some(gain)
}

// ---------------------------------------------------------------- player

enum Cmd {
    Play { id: SoundId, gain: f32 },
    Shutdown,
}

/// Toca os sons numa thread própria; barato de clonar a referência (`Arc`) e de chamar.
pub struct SoundPlayer {
    tx: Sender<Cmd>,
    params: Mutex<SoundParams>,
    /// Último `play` que não tocou por falta de dispositivo (para não encher o log).
    warned: AtomicBool,
}

impl SoundPlayer {
    pub fn start(params: SoundParams) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("vyrp-sound".into())
            .spawn(move || run(rx))
            .expect("thread de som");
        Self {
            tx,
            params: Mutex::new(params),
            warned: AtomicBool::new(false),
        }
    }

    pub fn set_params(&self, params: SoundParams) {
        *self.params.lock().unwrap() = params;
    }

    pub fn params(&self) -> SoundParams {
        *self.params.lock().unwrap()
    }

    /// Toca o som e volta na hora. Sem efeito se desligado, volume zero ou sem dispositivo.
    pub fn play(&self, id: SoundId) {
        let Some(gain) = gain_for(self.params()) else {
            return;
        };
        if self.tx.send(Cmd::Play { id, gain }).is_err()
            && !self.warned.swap(true, Ordering::Relaxed)
        {
            tracing::warn!("thread de som encerrada");
        }
    }
}

impl Drop for SoundPlayer {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

/// Saída aberta: stream + mixer compartilhado + sons já renderizados na taxa do dispositivo.
struct Output {
    _stream: cpal::Stream,
    mixer: Arc<Mutex<Mixer>>,
    bank: HashMap<SoundId, Arc<[f32]>>,
    broken: Arc<AtomicBool>,
}

/// Espera mínima entre tentativas de reabrir o dispositivo.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);

fn run(rx: Receiver<Cmd>) {
    let mut output = open_output()
        .map_err(|e| tracing::warn!(error = %e, "sem saída de áudio"))
        .ok();
    let mut last_attempt = Instant::now();
    while let Ok(cmd) = rx.recv() {
        let Cmd::Play { id, gain } = cmd else {
            break;
        };
        let needs_reopen = output
            .as_ref()
            .is_none_or(|o| o.broken.load(Ordering::Relaxed));
        if needs_reopen && last_attempt.elapsed() >= REOPEN_BACKOFF {
            last_attempt = Instant::now();
            output = open_output()
                .map_err(|e| tracing::warn!(error = %e, "sem saída de áudio"))
                .ok();
        }
        if let Some(out) = &output
            && let Some(samples) = out.bank.get(&id)
        {
            out.mixer.lock().unwrap().push(samples.clone(), gain);
        }
    }
}

fn open_output() -> Result<Output, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("nenhum dispositivo de saída")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    let config: cpal::StreamConfig = supported.config();

    let mixer = Arc::new(Mutex::new(Mixer::new()));
    let broken = Arc::new(AtomicBool::new(false));
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, config, channels, &mixer, &broken),
        cpal::SampleFormat::I16 => build::<i16>(&device, config, channels, &mixer, &broken),
        cpal::SampleFormat::U16 => build::<u16>(&device, config, channels, &mixer, &broken),
        other => return Err(format!("formato de amostra não suportado: {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;

    let bank = SoundId::ALL
        .iter()
        .map(|&id| (id, Arc::from(synth::render(id, rate))))
        .collect();
    Ok(Output {
        _stream: stream,
        mixer,
        bank,
        broken,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mixer: &Arc<Mutex<Mixer>>,
    broken: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let mixer = mixer.clone();
    let broken = broken.clone();
    let mut block: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                block.resize(data.len(), 0.0);
                match mixer.lock() {
                    Ok(mut m) => m.mix_into(&mut block, channels),
                    Err(_) => block.fill(0.0),
                }
                for (out, value) in data.iter_mut().zip(&block) {
                    *out = T::from_sample(*value);
                }
            },
            move |err: cpal::Error| {
                if matches!(err.kind(), cpal::ErrorKind::Xrun) {
                    return;
                }
                tracing::warn!(%err, "erro no stream de saída");
                broken.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- Fx: sons + mídia

/// Tudo que o usuário configura sobre som e mídia.
///
/// **Campos sugeridos para o Settings**: `sounds: bool` (já existe), `sound_volume: f32`,
/// `media_mode: MediaMode` (`off`/`pause`/`mute`/`duck`; padrão `off`), `media_duck_factor: f32`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FxParams {
    pub sound: SoundParams,
    pub media: MediaOptions,
}

/// Tempo para o som de início tocar antes de a mídia ser silenciada.
const START_SOUND_HEAD_MS: u64 = 230;
/// Quanto esperamos a mídia voltar antes de tocar o som de fim (ele precisa ser ouvido).
const RESTORE_WAIT: Duration = Duration::from_millis(400);

pub struct Fx {
    player: SoundPlayer,
    media: MediaController,
}

impl Fx {
    /// `marker_dir`: pasta do app para o marcador de restauração da mídia (AppData).
    pub fn start(params: FxParams, marker_dir: Option<PathBuf>) -> Self {
        Self {
            player: SoundPlayer::start(params.sound),
            media: MediaController::start(params.media, marker_dir),
        }
    }

    pub fn set_params(&self, params: FxParams) {
        self.player.set_params(params.sound);
        self.media.set_options(params.media);
    }

    /// PttStart / LockStart: toca o som e, passado o som, mexe na mídia.
    pub fn on_start(&self) {
        self.player.play(SoundId::Start);
        self.media
            .begin_after(Duration::from_millis(START_SOUND_HEAD_MS));
    }

    /// PttEnd / LockEnd: devolve a mídia e só então toca o som de fim.
    pub fn on_end(&self) {
        self.media.end(RESTORE_WAIT);
        self.player.play(SoundId::End);
    }

    pub fn on_error(&self) {
        self.media.end(RESTORE_WAIT);
        self.player.play(SoundId::Error);
    }

    pub fn on_cancel(&self) {
        self.media.end(RESTORE_WAIT);
        self.player.play(SoundId::Cancel);
    }

    pub fn player(&self) -> &SoundPlayer {
        &self.player
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_or_silent_params_never_play() {
        assert_eq!(
            gain_for(SoundParams {
                enabled: false,
                volume: 1.0
            }),
            None
        );
        assert_eq!(
            gain_for(SoundParams {
                enabled: true,
                volume: 0.0
            }),
            None
        );
        assert_eq!(
            gain_for(SoundParams {
                enabled: true,
                volume: 0.01
            }),
            None
        );
    }

    #[test]
    fn gain_follows_a_quadratic_curve_and_clamps() {
        let gain = |volume| {
            gain_for(SoundParams {
                enabled: true,
                volume,
            })
            .unwrap()
        };
        assert!((gain(1.0) - 1.0).abs() < 1e-6);
        assert!((gain(0.5) - 0.25).abs() < 1e-6);
        assert!((gain(7.0) - 1.0).abs() < 1e-6);
        assert_eq!(
            gain_for(SoundParams {
                enabled: true,
                volume: -3.0
            }),
            None
        );
    }

    #[test]
    fn defaults_are_on_at_a_moderate_volume() {
        let p = SoundParams::default();
        assert!(p.enabled && (p.volume - 0.6).abs() < 1e-6);
        assert_eq!(FxParams::default().media.mode, MediaMode::Off);
    }

    #[test]
    fn params_deserialize_with_missing_fields() {
        let p: SoundParams = serde_json::from_str(r#"{"volume": 0.3}"#).unwrap();
        assert!(p.enabled && (p.volume - 0.3).abs() < 1e-6);
        assert_eq!(
            serde_json::to_string(&SoundId::Cancel).unwrap(),
            "\"cancel\""
        );
    }

    #[test]
    fn player_calls_never_block_even_without_a_device() {
        // Sem dispositivo (CI) a thread só registra o aviso; play() tem de voltar logo.
        let player = SoundPlayer::start(SoundParams::default());
        let started = Instant::now();
        for _ in 0..50 {
            player.play(SoundId::Start);
        }
        assert!(started.elapsed() < Duration::from_millis(200));
        player.set_params(SoundParams {
            enabled: false,
            volume: 1.0,
        });
        assert!(!player.params().enabled);
    }
}
