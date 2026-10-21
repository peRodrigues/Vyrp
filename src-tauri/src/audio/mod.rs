//! Captura de áudio (V7): cpal/WASAPI → mono 16 kHz i16, num stream pré-aquecido.
//!
//! Uma thread `vyrp-audio` é dona do stream (no Windows o `cpal::Stream` não pode
//! mudar de thread). O callback do driver só faz downmix + reamostragem e manda o PCM
//! para essa thread, que grava o ditado, mede nível, vigia silêncio e duração e varre
//! os microfones. Padrão: stream **construído e pausado** (abre rápido ao apertar, sem
//! capturar nada antes); "mic quente" com pre-roll é opcional e desligado.

pub mod device;
pub mod dsp;
pub mod wav;

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use serde::{Deserialize, Serialize};

use crate::core::types::{AudioClip, Result, SAMPLE_RATE, VyrpError};
use dsp::{
    DurationAlert, LevelMeter, PreRoll, Recording, Resampler, SilenceAlert, SilenceWatch, downmix,
    to_i16,
};

/// Nível para o overlay a ~30 Hz.
const LEVEL_WINDOW: usize = SAMPLE_RATE as usize / 30;
/// Varredura de microfones quando ocioso.
const SCAN_EVERY: Duration = Duration::from_secs(3);
/// Quanto esperar pelos últimos blocos do driver depois de pausar.
const TAIL_WAIT: Duration = Duration::from_millis(60);

/// Configuração de captura (vem do Settings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Microfone escolhido pelo usuário (nome do SO); `None` = automático.
    pub preferred: Option<String>,
    /// Ordem de preferência quando o escolhido não está presente.
    pub ranking: Vec<String>,
    /// Mantém o mic aberto entre ditados (pre-roll). Desligado por padrão: privacidade.
    pub warm_mic: bool,
    pub preroll_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioWarning {
    /// Silêncio digital contínuo (5 s; `escalated` aos 15 s).
    NoAudio {
        escalated: bool,
    },
    /// 19 min de gravação.
    AlmostFull,
    /// 20 min: o resto não é gravado.
    Full,
    MicDisconnected {
        name: String,
    },
    NewMic {
        name: String,
    },
    StreamError {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// RMS 0..=1, só durante a gravação.
    Level(f32),
    Warning(AudioWarning),
    /// Stream aberto (pré-aquecido) num microfone.
    Ready {
        device: String,
        rate: u32,
        channels: u16,
    },
}

enum Msg {
    Begin,
    End(SyncSender<Result<AudioClip>>),
    Cancel,
    Configure(AudioConfig),
    Devices(SyncSender<Vec<String>>),
    Pcm(Vec<i16>),
    StreamError { disconnected: bool, message: String },
}

/// Quem abriu a gravação em curso. O ditado sempre tem prioridade: o medidor de nível do
/// hub/onboarding não começa por cima de um ditado e não pode cancelá-lo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOwner {
    Dictation,
    Monitor,
}

/// Controle da captura; barato de clonar.
#[derive(Clone)]
pub struct AudioService {
    tx: Sender<Msg>,
    owner: Arc<Mutex<Option<AudioOwner>>>,
}

impl AudioService {
    pub fn start<F>(cfg: AudioConfig, on_event: F) -> Self
    where
        F: Fn(AudioEvent) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("vyrp-audio".into())
            .spawn(move || Worker::new(cfg, worker_tx, Box::new(on_event)).run(rx))
            .expect("thread de áudio");
        Self {
            tx,
            owner: Arc::new(Mutex::new(None)),
        }
    }

    /// Dono da gravação em curso (`None` = ninguém gravando).
    pub fn owner(&self) -> Option<AudioOwner> {
        *self.owner.lock().unwrap()
    }

    /// Começa a gravar o ditado (PttStart/LockStart).
    pub fn begin(&self) {
        *self.owner.lock().unwrap() = Some(AudioOwner::Dictation);
        let _ = self.tx.send(Msg::Begin);
    }

    /// Medidor de nível (hub/onboarding): não começa por cima de um ditado.
    /// Devolve `false` quando o microfone está com o ditado.
    pub fn begin_monitor(&self) -> bool {
        let mut owner = self.owner.lock().unwrap();
        if *owner == Some(AudioOwner::Dictation) {
            return false;
        }
        *owner = Some(AudioOwner::Monitor);
        let _ = self.tx.send(Msg::Begin);
        true
    }

    /// Para o medidor, mas só se a gravação for dele: nunca cancela um ditado.
    pub fn cancel_monitor(&self) {
        let mut owner = self.owner.lock().unwrap();
        if *owner == Some(AudioOwner::Monitor) {
            *owner = None;
            let _ = self.tx.send(Msg::Cancel);
        }
    }

    /// Termina e devolve o áudio do ditado.
    pub fn end(&self) -> Result<AudioClip> {
        *self.owner.lock().unwrap() = None;
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Msg::End(reply))
            .map_err(|_| VyrpError::Audio("thread de áudio encerrada".into()))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|_| VyrpError::Timeout("fim da gravação".into()))?
    }

    /// Descarta a gravação em curso, seja de quem for.
    pub fn cancel(&self) {
        *self.owner.lock().unwrap() = None;
        let _ = self.tx.send(Msg::Cancel);
    }

    pub fn configure(&self, cfg: AudioConfig) {
        let _ = self.tx.send(Msg::Configure(cfg));
    }

    /// Nomes dos microfones presentes.
    pub fn devices(&self) -> Vec<String> {
        let (reply, rx) = mpsc::sync_channel(1);
        if self.tx.send(Msg::Devices(reply)).is_err() {
            return Vec::new();
        }
        rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default()
    }
}

struct Session {
    rec: Recording,
    silence: SilenceWatch,
    meter: LevelMeter,
    started: Instant,
    full: bool,
}

struct Worker {
    cfg: AudioConfig,
    tx: Sender<Msg>,
    on_event: Box<dyn Fn(AudioEvent) + Send>,
    host: cpal::Host,
    stream: Option<cpal::Stream>,
    device: Option<String>,
    session: Option<Session>,
    /// Gravação perdida por desconexão: o `End` devolve erro.
    aborted: bool,
    preroll: PreRoll,
    known: Vec<String>,
    last_scan: Instant,
    deferred: VecDeque<Msg>,
}

impl Worker {
    fn new(cfg: AudioConfig, tx: Sender<Msg>, on_event: Box<dyn Fn(AudioEvent) + Send>) -> Self {
        let preroll = PreRoll::new(if cfg.warm_mic { cfg.preroll_ms } else { 0 });
        Self {
            cfg,
            tx,
            on_event,
            host: cpal::default_host(),
            stream: None,
            device: None,
            session: None,
            aborted: false,
            preroll,
            known: Vec::new(),
            last_scan: Instant::now(),
            deferred: VecDeque::new(),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        self.known = self.device_names();
        self.open();
        loop {
            let msg = match self.deferred.pop_front() {
                Some(m) => m,
                None => match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        self.idle_scan();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                },
            };
            match msg {
                Msg::Begin => self.begin(),
                Msg::End(reply) => {
                    let clip = self.end(&rx);
                    let _ = reply.send(clip);
                }
                Msg::Cancel => {
                    self.session = None;
                    self.pause_if_cold();
                }
                Msg::Configure(cfg) => self.configure(cfg),
                Msg::Devices(reply) => {
                    let _ = reply.send(self.device_names());
                }
                Msg::Pcm(pcm) => self.on_pcm(&pcm),
                Msg::StreamError {
                    disconnected,
                    message,
                } => self.on_stream_error(disconnected, message),
            }
            if self.session.is_none() && self.last_scan.elapsed() >= SCAN_EVERY {
                self.idle_scan();
            }
        }
    }

    fn emit(&self, ev: AudioEvent) {
        (self.on_event)(ev);
    }

    fn device_names(&self) -> Vec<String> {
        self.host
            .input_devices()
            .map(|devices| devices.filter_map(|d| device_name(&d)).collect())
            .unwrap_or_default()
    }

    /// Constrói o stream no microfone escolhido; pausado, salvo com "mic quente".
    fn open(&mut self) {
        self.stream = None;
        let names = self.device_names();
        let default = self
            .host
            .default_input_device()
            .and_then(|d| device_name(&d));
        let Some(name) = device::choose(
            &names,
            default.as_deref(),
            self.cfg.preferred.as_deref(),
            &self.cfg.ranking,
        ) else {
            self.device = None;
            tracing::warn!("nenhum microfone encontrado");
            return;
        };
        let found = self
            .host
            .input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| device_name(d).as_deref() == Some(name.as_str())));
        let Some(dev) = found else { return };
        match build_stream(&dev, self.tx.clone()) {
            Ok((stream, rate, channels)) => {
                if self.cfg.warm_mic
                    && let Err(err) = stream.play()
                {
                    tracing::warn!(%err, "mic quente não iniciou");
                }
                tracing::info!(device = %name, rate, channels, warm = self.cfg.warm_mic, "microfone pronto");
                self.emit(AudioEvent::Ready {
                    device: name.clone(),
                    rate,
                    channels,
                });
                self.stream = Some(stream);
                self.device = Some(name);
            }
            Err(message) => {
                tracing::error!(device = %name, %message, "falha ao abrir o microfone");
                self.emit(AudioEvent::Warning(AudioWarning::StreamError { message }));
            }
        }
    }

    fn begin(&mut self) {
        if self.session.is_some() {
            return;
        }
        if self.stream.is_none() {
            self.open();
        }
        let Some(stream) = &self.stream else {
            self.aborted = true;
            return;
        };
        if !self.cfg.warm_mic
            && let Err(err) = stream.play()
        {
            tracing::error!(%err, "o microfone não iniciou");
            self.aborted = true;
            return;
        }
        self.aborted = false;
        self.session = Some(Session {
            rec: Recording::with_preroll(self.preroll.take()),
            silence: SilenceWatch::default(),
            meter: LevelMeter::new(LEVEL_WINDOW),
            started: Instant::now(),
            full: false,
        });
        tracing::debug!(device = ?self.device, "gravação iniciada");
    }

    fn end(&mut self, rx: &Receiver<Msg>) -> Result<AudioClip> {
        self.pause_if_cold();
        // Recolhe os últimos blocos que o driver já entregou; o resto da fila espera.
        let deadline = Instant::now() + TAIL_WAIT;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Msg::Pcm(pcm)) => self.on_pcm(&pcm),
                Ok(other) => self.deferred.push_back(other),
                Err(_) => break,
            }
        }
        let Some(session) = self.session.take() else {
            let reason = if self.aborted {
                "microfone indisponível ou desconectado durante o ditado"
            } else {
                "nenhuma gravação em andamento"
            };
            self.aborted = false;
            return Err(VyrpError::Audio(reason.into()));
        };
        let clip = AudioClip::new(session.rec.pcm);
        tracing::debug!(
            ms = clip.duration_ms(),
            wall_ms = session.started.elapsed().as_millis() as u64,
            "gravação encerrada"
        );
        Ok(clip)
    }

    fn pause_if_cold(&self) {
        if !self.cfg.warm_mic
            && let Some(stream) = &self.stream
        {
            let _ = stream.pause();
        }
    }

    fn on_pcm(&mut self, pcm: &[i16]) {
        let Some(session) = self.session.as_mut() else {
            self.preroll.push(pcm);
            return;
        };
        let mut levels = Vec::new();
        session.meter.push(pcm, |l| levels.push(l));
        let silence = session.silence.push(pcm);
        let duration = if session.full {
            None
        } else {
            session.rec.push(pcm)
        };
        if duration == Some(DurationAlert::Full) {
            session.full = true;
        }
        for level in levels {
            self.emit(AudioEvent::Level(level));
        }
        if let Some(alert) = silence {
            self.emit(AudioEvent::Warning(AudioWarning::NoAudio {
                escalated: alert == SilenceAlert::Escalate,
            }));
        }
        match duration {
            Some(DurationAlert::AlmostFull) => {
                self.emit(AudioEvent::Warning(AudioWarning::AlmostFull))
            }
            Some(DurationAlert::Full) => self.emit(AudioEvent::Warning(AudioWarning::Full)),
            None => {}
        }
    }

    fn on_stream_error(&mut self, disconnected: bool, message: String) {
        if !disconnected {
            tracing::warn!(%message, "erro no stream de áudio");
            return;
        }
        let name = self.device.take().unwrap_or_default();
        tracing::warn!(device = %name, "microfone desconectado");
        self.stream = None;
        if self.session.take().is_some() {
            self.aborted = true;
        }
        self.emit(AudioEvent::Warning(AudioWarning::MicDisconnected { name }));
    }

    fn configure(&mut self, cfg: AudioConfig) {
        if cfg == self.cfg {
            return;
        }
        let reopen = cfg.preferred != self.cfg.preferred
            || cfg.ranking != self.cfg.ranking
            || cfg.warm_mic != self.cfg.warm_mic;
        self.preroll = PreRoll::new(if cfg.warm_mic { cfg.preroll_ms } else { 0 });
        self.cfg = cfg;
        if reopen && self.session.is_none() {
            self.open();
        }
    }

    /// Ocioso: avisa microfone novo, reabre se o atual sumiu ou se não havia nenhum.
    fn idle_scan(&mut self) {
        if self.session.is_some() {
            return;
        }
        self.last_scan = Instant::now();
        let names = self.device_names();
        for name in device::newly_connected(&self.known, &names) {
            tracing::info!(device = %name, "microfone novo");
            self.emit(AudioEvent::Warning(AudioWarning::NewMic { name }));
        }
        let current_gone = self.device.as_ref().is_some_and(|d| !names.contains(d));
        if current_gone {
            let name = self.device.take().unwrap_or_default();
            self.stream = None;
            self.emit(AudioEvent::Warning(AudioWarning::MicDisconnected { name }));
        }
        let changed = names != self.known;
        self.known = names;
        if self.stream.is_none() || (changed && self.cfg.preferred.is_none()) {
            // Sem stream, ou mudou a lista e a escolha é automática: reavalia o melhor mic.
            self.open();
        }
    }
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

fn build_stream(
    device: &cpal::Device,
    tx: Sender<Msg>,
) -> std::result::Result<(cpal::Stream, u32, u16), String> {
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let rate = supported.sample_rate();
    let channels = supported.channels();
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_typed::<f32>(device, config, tx),
        cpal::SampleFormat::I16 => build_typed::<i16>(device, config, tx),
        cpal::SampleFormat::I32 => build_typed::<i32>(device, config, tx),
        cpal::SampleFormat::U16 => build_typed::<u16>(device, config, tx),
        other => return Err(format!("formato de amostra não suportado: {other}")),
    }?;
    Ok((stream, rate, channels))
}

fn build_typed<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    tx: Sender<Msg>,
) -> std::result::Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut resampler = Resampler::to_16k(config.sample_rate);
    let (mut floats, mut mono, mut out) = (Vec::new(), Vec::new(), Vec::new());
    let data_tx = tx.clone();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                floats.clear();
                floats.extend(data.iter().map(|&s| s.to_sample::<f32>()));
                mono.clear();
                downmix(&floats, channels, &mut mono);
                out.clear();
                resampler.process(&mono, &mut out);
                if !out.is_empty() {
                    let pcm = out.iter().map(|&x| to_i16(x)).collect();
                    let _ = data_tx.send(Msg::Pcm(pcm));
                }
            },
            move |err: cpal::Error| {
                // Xrun no primeiro bloco após `play()` é normal no WASAPI: não é erro.
                if matches!(err.kind(), cpal::ErrorKind::Xrun) {
                    tracing::debug!(%err, "xrun no stream de áudio");
                    return;
                }
                let disconnected = matches!(err.kind(), cpal::ErrorKind::DeviceNotAvailable);
                let _ = tx.send(Msg::StreamError {
                    disconnected,
                    message: err.to_string(),
                });
            },
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serviço sem thread de áudio: só registra as mensagens enviadas.
    fn service() -> (AudioService, Receiver<Msg>) {
        let (tx, rx) = mpsc::channel();
        (
            AudioService {
                tx,
                owner: Arc::new(Mutex::new(None)),
            },
            rx,
        )
    }

    fn sent(rx: &Receiver<Msg>) -> Vec<&'static str> {
        rx.try_iter()
            .map(|m| match m {
                Msg::Begin => "begin",
                Msg::Cancel => "cancel",
                _ => "outro",
            })
            .collect()
    }

    #[test]
    fn monitor_never_starts_over_a_dictation() {
        let (audio, rx) = service();
        audio.begin();
        assert!(!audio.begin_monitor());
        assert_eq!(audio.owner(), Some(AudioOwner::Dictation));
        assert_eq!(sent(&rx), vec!["begin"]);
    }

    #[test]
    fn monitor_stop_does_not_cancel_a_dictation() {
        let (audio, rx) = service();
        assert!(audio.begin_monitor());
        // O ditado começa: cancela o medidor e assume o microfone.
        audio.cancel();
        audio.begin();
        // O hub ainda acha que o medidor está ligado e manda parar: nada acontece.
        audio.cancel_monitor();
        assert_eq!(audio.owner(), Some(AudioOwner::Dictation));
        assert_eq!(sent(&rx), vec!["begin", "cancel", "begin"]);
    }

    #[test]
    fn monitor_stop_cancels_its_own_session() {
        let (audio, rx) = service();
        assert!(audio.begin_monitor());
        audio.cancel_monitor();
        assert_eq!(audio.owner(), None);
        audio.cancel_monitor();
        assert_eq!(sent(&rx), vec!["begin", "cancel"]);
    }
}
