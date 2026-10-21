//! Mídia durante o ditado (V19): pausar a música, silenciar ou baixar o volume enquanto o
//! usuário fala, com restauração garantida.
//!
//! A lógica (o que mudar, o que restaurar, quando NÃO restaurar) é pura e testada com um
//! backend falso ([`MediaBackend`]). O backend real usa as sessões/endpoints de áudio do
//! Windows (Core Audio) e só existe no Windows.
//!
//! Restauração garantida em três camadas:
//! 1. [`MediaGuard::end`] desfaz exatamente o que o `begin` mudou;
//! 2. o `Drop` do guarda chama `end` (inclusive se a thread sair por pânico);
//! 3. um marcador em disco guarda o estado anterior enquanto algo está alterado; se o app
//!    morrer no meio do ditado, [`recover`] desfaz no próximo início.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// O que fazer com a mídia do sistema enquanto o ditado está ouvindo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaMode {
    /// Não mexe em nada.
    #[default]
    Off,
    /// Aperta play/pause se houver áudio tocando, e de novo ao fim (só se foi ele que pausou).
    Pause,
    /// Silencia o volume geral e desfaz ao fim.
    Mute,
    /// Baixa o volume geral (fator em [`MediaOptions::duck_factor`]) e desfaz ao fim.
    Duck,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaOptions {
    pub mode: MediaMode,
    /// Quanto do volume atual fica no modo `Duck` (0..1).
    pub duck_factor: f32,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self {
            mode: MediaMode::Off,
            duck_factor: 0.2,
        }
    }
}

/// Acesso ao áudio do sistema. Erros são texto para o log; nunca derrubam o ditado.
pub trait MediaBackend {
    fn is_muted(&mut self) -> Result<bool, String>;
    fn set_muted(&mut self, muted: bool) -> Result<(), String>;
    /// Volume geral, 0..1.
    fn volume(&mut self) -> Result<f32, String>;
    fn set_volume(&mut self, volume: f32) -> Result<(), String>;
    /// Há áudio saindo agora (medidor de pico do dispositivo padrão)?
    fn audio_playing(&mut self) -> Result<bool, String>;
    /// Tecla de mídia play/pause.
    fn send_play_pause(&mut self) -> Result<(), String>;
}

/// O que o guarda alterou (e portanto tem de desfazer).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    /// Silenciou; o volume já estava desmutado antes.
    Muted,
    /// Baixou o volume de `previous` para `ducked`.
    Ducked { previous: f32, ducked: f32 },
    /// Apertou play/pause para pausar.
    Paused,
}

/// Arquivo com a alteração em curso (só existe enquanto algo está alterado).
#[derive(Debug, Clone)]
pub struct Marker(PathBuf);

impl Marker {
    pub fn in_dir(dir: &Path) -> Self {
        Self(dir.join("media-restore.json"))
    }

    fn write(&self, change: Change) {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&change) {
            let _ = std::fs::write(&self.0, text);
        }
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.0);
    }

    fn read(&self) -> Option<Change> {
        serde_json::from_str(&std::fs::read_to_string(&self.0).ok()?).ok()
    }
}

/// Aplica e desfaz as alterações. Um guarda por ditado em curso; `begin` repetido é ignorado.
pub struct MediaGuard<B: MediaBackend> {
    backend: B,
    options: MediaOptions,
    marker: Option<Marker>,
    changed: Option<Change>,
}

/// Perto o bastante para considerar que o volume é o que nós deixamos.
const VOLUME_EPSILON: f32 = 0.015;

impl<B: MediaBackend> MediaGuard<B> {
    pub fn new(backend: B, options: MediaOptions, marker: Option<Marker>) -> Self {
        Self {
            backend,
            options,
            marker,
            changed: None,
        }
    }

    pub fn set_options(&mut self, options: MediaOptions) {
        self.options = options;
    }

    pub fn is_engaged(&self) -> bool {
        self.changed.is_some()
    }

    /// Liga a alteração do modo atual. `Off`, já alterado ou erro do backend: não faz nada
    /// (e o erro volta para o log).
    pub fn begin(&mut self) -> Result<(), String> {
        if self.changed.is_some() {
            return Ok(());
        }
        let change = match self.options.mode {
            MediaMode::Off => return Ok(()),
            MediaMode::Mute => {
                if self.backend.is_muted()? {
                    return Ok(()); // já estava mudo: nada a mexer, nada a restaurar
                }
                self.remember(Change::Muted);
                self.backend.set_muted(true)?;
                Change::Muted
            }
            MediaMode::Duck => {
                let previous = self.backend.volume()?;
                let ducked = (previous * self.options.duck_factor.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                let change = Change::Ducked { previous, ducked };
                self.remember(change);
                self.backend.set_volume(ducked)?;
                change
            }
            MediaMode::Pause => {
                if !self.backend.audio_playing()? {
                    return Ok(()); // nada tocando: apertar play/pause INICIARIA algo
                }
                self.backend.send_play_pause()?;
                Change::Paused
            }
        };
        self.changed = Some(change);
        Ok(())
    }

    fn remember(&mut self, change: Change) {
        if let Some(marker) = &self.marker {
            marker.write(change);
        }
    }

    /// Desfaz exatamente o que o `begin` fez. Seguro de chamar sem `begin` e várias vezes.
    pub fn end(&mut self) -> Result<(), String> {
        let Some(change) = self.changed.take() else {
            return Ok(());
        };
        let result = undo(&mut self.backend, change);
        if let Some(marker) = &self.marker {
            marker.clear();
        }
        result
    }
}

impl<B: MediaBackend> Drop for MediaGuard<B> {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

fn undo<B: MediaBackend>(backend: &mut B, change: Change) -> Result<(), String> {
    match change {
        Change::Muted => backend.set_muted(false),
        Change::Ducked { previous, ducked } => {
            // Se o usuário mexeu no volume durante o ditado, a escolha dele vale.
            if (backend.volume()? - ducked).abs() <= VOLUME_EPSILON {
                backend.set_volume(previous)
            } else {
                Ok(())
            }
        }
        Change::Paused => backend.send_play_pause(),
    }
}

/// Chamar no início do app: se o marcador existe, o app morreu com a mídia alterada.
/// Desfaz (mudo/volume; uma pausa não é desfeita: retomar música sozinho assusta mais).
pub fn recover<B: MediaBackend>(backend: &mut B, marker: &Marker) -> Result<bool, String> {
    let Some(change) = marker.read() else {
        marker.clear();
        return Ok(false);
    };
    let result = match change {
        Change::Paused => Ok(()),
        other => undo(backend, other),
    };
    marker.clear();
    result.map(|()| true)
}

// ---------------------------------------------------------------- controlador (thread)

enum Cmd {
    /// Liga depois do atraso (deixa o som de início tocar antes de silenciar).
    Begin {
        after: Duration,
    },
    /// Desfaz e avisa (para o som de fim só tocar com o volume de volta).
    End {
        done: Sender<()>,
    },
    Options(MediaOptions),
    Shutdown,
}

/// Controla a mídia numa thread própria (COM no Windows), sem bloquear quem chama.
pub struct MediaController {
    tx: Sender<Cmd>,
}

impl MediaController {
    /// `marker_dir`: pasta onde o marcador de restauração vive (ex.: AppData do app).
    pub fn start(options: MediaOptions, marker_dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("vyrp-media".into())
            .spawn(move || run(rx, options, marker_dir))
            .expect("thread de mídia");
        Self { tx }
    }

    pub fn set_options(&self, options: MediaOptions) {
        let _ = self.tx.send(Cmd::Options(options));
    }

    pub fn begin_after(&self, after: Duration) {
        let _ = self.tx.send(Cmd::Begin { after });
    }

    /// Desfaz e espera a confirmação por até `wait` (0 = não espera).
    pub fn end(&self, wait: Duration) {
        let (done, rx) = mpsc::channel();
        if self.tx.send(Cmd::End { done }).is_ok() && !wait.is_zero() {
            let _ = rx.recv_timeout(wait);
        }
    }
}

impl Drop for MediaController {
    fn drop(&mut self) {
        let (done, rx) = mpsc::channel();
        if self.tx.send(Cmd::End { done }).is_ok() {
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

fn run(rx: Receiver<Cmd>, options: MediaOptions, marker_dir: Option<PathBuf>) {
    let backend = match system::open() {
        Ok(backend) => backend,
        Err(err) => {
            tracing::warn!(%err, "controle de mídia indisponível");
            // Sem backend, só drena os comandos (End confirma para ninguém travar).
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Cmd::End { done } => {
                        let _ = done.send(());
                    }
                    Cmd::Shutdown => break,
                    _ => {}
                }
            }
            return;
        }
    };
    let marker = marker_dir.map(|dir| Marker::in_dir(&dir));
    let mut guard = MediaGuard::new(backend, options, marker.clone());
    if let Some(marker) = &marker {
        match recover(&mut guard.backend, marker) {
            Ok(true) => tracing::info!("mídia restaurada após encerramento anterior"),
            Ok(false) => {}
            Err(err) => tracing::warn!(%err, "não foi possível restaurar a mídia"),
        }
    }
    let mut pending: Option<Instant> = None;
    loop {
        let timeout = pending.map_or(Duration::from_secs(3600), |at| {
            at.saturating_duration_since(Instant::now())
        });
        match rx.recv_timeout(timeout) {
            Ok(Cmd::Begin { after }) => pending = Some(Instant::now() + after),
            Ok(Cmd::End { done }) => {
                pending = None;
                if let Err(err) = guard.end() {
                    tracing::warn!(%err, "falha ao restaurar a mídia");
                }
                let _ = done.send(());
            }
            Ok(Cmd::Options(next)) => guard.set_options(next),
            Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if pending.take().is_some()
                    && let Err(err) = guard.begin()
                {
                    tracing::warn!(%err, "falha ao alterar a mídia");
                }
            }
        }
    }
    let _ = guard.end();
}

// ---------------------------------------------------------------- backends do sistema

#[cfg(windows)]
pub mod system {
    //! Core Audio: endpoint de renderização padrão (mudo, volume, medidor de pico) e a
    //! tecla de mídia play/pause por SendInput.

    use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
    use windows::Win32::Media::Audio::{
        IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MEDIA_PLAY_PAUSE,
    };

    use super::MediaBackend;

    /// Abaixo disso o medidor de pico é só ruído do dispositivo.
    const PLAYING_PEAK: f32 = 0.002;

    pub struct WindowsMedia {
        com_started: bool,
    }

    pub fn open() -> Result<WindowsMedia, String> {
        // SAFETY: inicializa COM na thread que vai usar o backend; o Drop desfaz.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(format!("CoInitializeEx falhou: {hr:?}"));
        }
        Ok(WindowsMedia { com_started: true })
    }

    impl Drop for WindowsMedia {
        fn drop(&mut self) {
            if self.com_started {
                // SAFETY: par do CoInitializeEx bem-sucedido em `open`.
                unsafe { CoUninitialize() };
            }
        }
    }

    fn endpoint() -> Result<windows::Win32::Media::Audio::IMMDevice, String> {
        // SAFETY: COM já inicializado nesta thread; chamadas simples sem ponteiros do chamador.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| e.to_string())?;
            enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| e.to_string())
        }
    }

    fn volume_interface() -> Result<IAudioEndpointVolume, String> {
        let device = endpoint()?;
        // SAFETY: ativa a interface pedida no dispositivo recém-obtido.
        unsafe {
            device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())
        }
    }

    fn key_event(flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_MEDIA_PLAY_PAUSE,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    impl MediaBackend for WindowsMedia {
        fn is_muted(&mut self) -> Result<bool, String> {
            // SAFETY: interface COM válida durante a chamada.
            unsafe {
                volume_interface()?
                    .GetMute()
                    .map(|b| b.as_bool())
                    .map_err(|e| e.to_string())
            }
        }

        fn set_muted(&mut self, muted: bool) -> Result<(), String> {
            // SAFETY: contexto de evento nulo é permitido.
            unsafe {
                volume_interface()?
                    .SetMute(muted, std::ptr::null())
                    .map_err(|e| e.to_string())
            }
        }

        fn volume(&mut self) -> Result<f32, String> {
            // SAFETY: interface COM válida durante a chamada.
            unsafe {
                volume_interface()?
                    .GetMasterVolumeLevelScalar()
                    .map_err(|e| e.to_string())
            }
        }

        fn set_volume(&mut self, volume: f32) -> Result<(), String> {
            // SAFETY: contexto de evento nulo é permitido.
            unsafe {
                volume_interface()?
                    .SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null())
                    .map_err(|e| e.to_string())
            }
        }

        fn audio_playing(&mut self) -> Result<bool, String> {
            let device = endpoint()?;
            // SAFETY: ativa e lê o medidor do dispositivo recém-obtido.
            unsafe {
                let meter = device
                    .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
                    .map_err(|e| e.to_string())?;
                Ok(meter.GetPeakValue().map_err(|e| e.to_string())? > PLAYING_PEAK)
            }
        }

        fn send_play_pause(&mut self) -> Result<(), String> {
            let inputs = [key_event(0), key_event(KEYEVENTF_KEYUP)];
            // SAFETY: `inputs` vive durante a chamada e o tamanho é o de INPUT.
            let sent = unsafe {
                SendInput(
                    inputs.len() as u32,
                    inputs.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                )
            };
            if sent == inputs.len() as u32 {
                Ok(())
            } else {
                Err("SendInput não enviou a tecla de mídia".into())
            }
        }
    }
}

#[cfg(not(windows))]
pub mod system {
    use super::MediaBackend;

    pub struct NoMedia;

    pub fn open() -> Result<NoMedia, String> {
        Err("controle de mídia só existe no Windows".into())
    }

    impl MediaBackend for NoMedia {
        fn is_muted(&mut self) -> Result<bool, String> {
            Err("sem suporte".into())
        }
        fn set_muted(&mut self, _: bool) -> Result<(), String> {
            Err("sem suporte".into())
        }
        fn volume(&mut self) -> Result<f32, String> {
            Err("sem suporte".into())
        }
        fn set_volume(&mut self, _: f32) -> Result<(), String> {
            Err("sem suporte".into())
        }
        fn audio_playing(&mut self) -> Result<bool, String> {
            Err("sem suporte".into())
        }
        fn send_play_pause(&mut self) -> Result<(), String> {
            Err("sem suporte".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Backend falso que registra as chamadas e simula o estado do sistema.
    #[derive(Default)]
    struct State {
        muted: bool,
        volume: f32,
        playing: bool,
        calls: Vec<String>,
        fail_set_muted: bool,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<State>>);

    impl Fake {
        fn with(muted: bool, volume: f32, playing: bool) -> Self {
            let fake = Fake::default();
            *fake.0.borrow_mut() = State {
                muted,
                volume,
                playing,
                ..Default::default()
            };
            fake
        }
        fn calls(&self) -> Vec<String> {
            self.0.borrow().calls.clone()
        }
    }

    impl MediaBackend for Fake {
        fn is_muted(&mut self) -> Result<bool, String> {
            Ok(self.0.borrow().muted)
        }
        fn set_muted(&mut self, muted: bool) -> Result<(), String> {
            let mut s = self.0.borrow_mut();
            if s.fail_set_muted {
                return Err("falhou".into());
            }
            s.muted = muted;
            s.calls.push(format!("mute={muted}"));
            Ok(())
        }
        fn volume(&mut self) -> Result<f32, String> {
            Ok(self.0.borrow().volume)
        }
        fn set_volume(&mut self, volume: f32) -> Result<(), String> {
            let mut s = self.0.borrow_mut();
            s.volume = volume;
            s.calls.push(format!("volume={volume:.2}"));
            Ok(())
        }
        fn audio_playing(&mut self) -> Result<bool, String> {
            Ok(self.0.borrow().playing)
        }
        fn send_play_pause(&mut self) -> Result<(), String> {
            let mut s = self.0.borrow_mut();
            s.playing = !s.playing;
            s.calls.push("play_pause".into());
            Ok(())
        }
    }

    fn guard(fake: &Fake, mode: MediaMode) -> MediaGuard<Fake> {
        MediaGuard::new(
            fake.clone(),
            MediaOptions {
                mode,
                duck_factor: 0.25,
            },
            None,
        )
    }

    fn temp_marker() -> Marker {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        Marker::in_dir(&std::env::temp_dir().join(format!(
            "vyrp-media-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )))
    }

    #[test]
    fn off_never_touches_the_system() {
        let fake = Fake::with(false, 0.8, true);
        let mut g = guard(&fake, MediaMode::Off);
        g.begin().unwrap();
        g.end().unwrap();
        assert!(fake.calls().is_empty());
        assert!(!g.is_engaged());
    }

    #[test]
    fn mute_mutes_then_restores() {
        let fake = Fake::with(false, 0.8, false);
        let mut g = guard(&fake, MediaMode::Mute);
        g.begin().unwrap();
        assert!(fake.0.borrow().muted && g.is_engaged());
        g.end().unwrap();
        assert!(!fake.0.borrow().muted && !g.is_engaged());
        assert_eq!(fake.calls(), ["mute=true", "mute=false"]);
    }

    #[test]
    fn already_muted_is_left_alone_and_not_unmuted() {
        let fake = Fake::with(true, 0.8, false);
        let mut g = guard(&fake, MediaMode::Mute);
        g.begin().unwrap();
        g.end().unwrap();
        assert!(
            fake.0.borrow().muted,
            "o usuário já tinha silenciado: continua mudo"
        );
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn duck_lowers_and_restores_the_exact_volume() {
        let fake = Fake::with(false, 0.8, false);
        let mut g = guard(&fake, MediaMode::Duck);
        g.begin().unwrap();
        assert!((fake.0.borrow().volume - 0.2).abs() < 1e-6);
        g.end().unwrap();
        assert!((fake.0.borrow().volume - 0.8).abs() < 1e-6);
    }

    #[test]
    fn duck_respects_a_volume_changed_by_the_user_meanwhile() {
        let fake = Fake::with(false, 0.8, false);
        let mut g = guard(&fake, MediaMode::Duck);
        g.begin().unwrap();
        fake.0.borrow_mut().volume = 0.55; // o usuário mexeu no volume
        g.end().unwrap();
        assert!(
            (fake.0.borrow().volume - 0.55).abs() < 1e-6,
            "não sobrescreve a escolha dele"
        );
    }

    #[test]
    fn pause_only_when_something_is_playing_and_resumes_only_what_it_paused() {
        let fake = Fake::with(false, 0.8, true);
        let mut g = guard(&fake, MediaMode::Pause);
        g.begin().unwrap();
        assert!(!fake.0.borrow().playing);
        g.end().unwrap();
        assert!(fake.0.borrow().playing);
        assert_eq!(fake.calls(), ["play_pause", "play_pause"]);

        // Nada tocando: apertar play/pause iniciaria música. Não aperta nem ao fim.
        let silent = Fake::with(false, 0.8, false);
        let mut g = guard(&silent, MediaMode::Pause);
        g.begin().unwrap();
        g.end().unwrap();
        assert!(silent.calls().is_empty());
    }

    #[test]
    fn begin_twice_and_end_without_begin_are_harmless() {
        let fake = Fake::with(false, 0.8, false);
        let mut g = guard(&fake, MediaMode::Mute);
        g.end().unwrap();
        g.begin().unwrap();
        g.begin().unwrap();
        g.end().unwrap();
        g.end().unwrap();
        assert_eq!(fake.calls(), ["mute=true", "mute=false"]);
    }

    #[test]
    fn drop_restores_even_without_an_explicit_end() {
        let fake = Fake::with(false, 0.8, false);
        {
            let mut g = guard(&fake, MediaMode::Mute);
            g.begin().unwrap();
            assert!(fake.0.borrow().muted);
        }
        assert!(!fake.0.borrow().muted);
    }

    #[test]
    fn restores_after_a_panic_unwinds_the_guard() {
        let fake = Fake::with(false, 0.8, false);
        let inner = fake.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut g = guard(&inner, MediaMode::Mute);
            g.begin().unwrap();
            panic!("falha no meio do ditado");
        }));
        assert!(result.is_err());
        assert!(!fake.0.borrow().muted);
    }

    #[test]
    fn backend_failure_on_begin_leaves_nothing_to_undo() {
        let fake = Fake::with(false, 0.8, false);
        fake.0.borrow_mut().fail_set_muted = true;
        let mut g = guard(&fake, MediaMode::Mute);
        assert!(g.begin().is_err());
        assert!(!g.is_engaged());
        g.end().unwrap();
    }

    #[test]
    fn marker_exists_only_while_something_is_changed() {
        let fake = Fake::with(false, 0.8, false);
        let marker = temp_marker();
        let mut g = MediaGuard::new(
            fake.clone(),
            MediaOptions {
                mode: MediaMode::Mute,
                duck_factor: 0.2,
            },
            Some(marker.clone()),
        );
        assert_eq!(marker.read(), None);
        g.begin().unwrap();
        assert_eq!(marker.read(), Some(Change::Muted));
        g.end().unwrap();
        assert_eq!(marker.read(), None);
    }

    #[test]
    fn recover_undoes_a_crashed_session() {
        // Simula: o app morreu com o volume baixado; o marcador ficou no disco.
        let marker = temp_marker();
        marker.write(Change::Ducked {
            previous: 0.7,
            ducked: 0.14,
        });
        let mut fake = Fake::with(false, 0.14, false);
        assert_eq!(recover(&mut fake, &marker), Ok(true));
        assert!((fake.0.borrow().volume - 0.7).abs() < 1e-6);
        assert_eq!(marker.read(), None);
        // Segunda vez: nada a fazer.
        assert_eq!(recover(&mut fake, &marker), Ok(false));

        marker.write(Change::Muted);
        let mut muted = Fake::with(true, 0.5, false);
        assert_eq!(recover(&mut muted, &marker), Ok(true));
        assert!(!muted.0.borrow().muted);

        // Pausa não é retomada sozinha.
        marker.write(Change::Paused);
        let mut paused = Fake::with(false, 0.5, false);
        assert_eq!(recover(&mut paused, &marker), Ok(true));
        assert!(paused.calls().is_empty());
    }

    #[test]
    fn recover_with_a_corrupt_marker_just_clears_it() {
        let marker = temp_marker();
        std::fs::create_dir_all(marker.0.parent().unwrap()).unwrap();
        std::fs::write(&marker.0, "isto não é json").unwrap();
        let mut fake = Fake::with(false, 0.5, false);
        assert_eq!(recover(&mut fake, &marker), Ok(false));
        assert!(!marker.0.exists());
    }

    #[test]
    fn modes_serialize_in_snake_case_and_default_to_off() {
        assert_eq!(MediaMode::default(), MediaMode::Off);
        assert_eq!(serde_json::to_string(&MediaMode::Duck).unwrap(), "\"duck\"");
        assert_eq!(MediaOptions::default().mode, MediaMode::Off);
    }
}
