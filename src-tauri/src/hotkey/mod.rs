//! Atalhos globais: hook de teclado/mouse (V5) e máquina de gestos (V6).
//!
//! Fluxo: hook (thread `vyrp-hook`) → canal de [`KeyEvent`](crate::core::types::KeyEvent)
//! → thread `vyrp-gestures` (máquina de gestos) → callback com cada [`GestureEvent`].
//! Só teclas de atalho são logadas (teclas comuns de atalho, como Espaço, só com um
//! modificador pressionado); o resto da digitação nunca sai da thread de gestos.

pub mod gesture;
pub mod hook_logic;
pub mod keys;

#[cfg(windows)]
pub mod hook;

pub use gesture::{GestureConfig, GestureFsm};

use crate::core::types::GestureEvent;

/// Evento Tauri (diagnóstico) com cada gesto reconhecido.
pub const EVENT_GESTURE: &str = "gesture://event";

/// Intervalo da reconciliação de teclas presas quando não chegam eventos.
#[cfg(windows)]
const RECONCILE_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// Hook em execução + canal para trocar os atalhos sem reiniciar.
#[cfg(windows)]
pub struct HotkeyService {
    pub hook: hook::HookHandle,
    config_tx: std::sync::mpsc::Sender<GestureConfig>,
}

#[cfg(windows)]
impl HotkeyService {
    /// Aplica novos atalhos (ex.: depois de `set_settings`); inválidos são ignorados.
    pub fn update(&self, cfg: GestureConfig) {
        let _ = self.config_tx.send(cfg);
    }
}

/// Config válida, ou os padrões (com aviso) se a recebida tiver problemas.
#[cfg(windows)]
fn checked(cfg: GestureConfig) -> GestureConfig {
    let problems = cfg.validate();
    if problems.is_empty() {
        cfg
    } else {
        tracing::warn!(?problems, "atalhos inválidos; usando os padrões");
        GestureConfig::default()
    }
}

/// Liga o hook e a máquina de gestos; `on_gesture` roda na thread de gestos.
#[cfg(windows)]
pub fn start<F>(cfg: GestureConfig, on_gesture: F) -> Result<HotkeyService, String>
where
    F: Fn(GestureEvent) + Send + 'static,
{
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Instant;

    use crate::core::types::KeyEvent;
    use gesture::Output;

    let log_active = |cfg: &GestureConfig| {
        tracing::info!(
            ptt = %cfg.ptt.label(),
            lock = %cfg.lock.as_ref().map(|c| c.label()).unwrap_or_default(),
            "atalhos ativos"
        )
    };
    let cfg = checked(cfg);
    log_active(&cfg);

    let (tx, rx) = mpsc::channel::<KeyEvent>();
    let (config_tx, config_rx) = mpsc::channel::<GestureConfig>();
    let handle = hook::start(tx)?;
    std::thread::Builder::new()
        .name("vyrp-gestures".into())
        .spawn(move || {
            let mut fsm = GestureFsm::new(cfg);
            let ks = hook::AsyncKeyState;
            loop {
                if let Some(next) = config_rx.try_iter().last() {
                    let next = checked(next);
                    if &next != fsm.config() {
                        log_active(&next);
                        fsm = GestureFsm::new(next);
                    }
                }
                let outputs = match rx.recv_timeout(RECONCILE_EVERY) {
                    Ok(ev) => {
                        let repeat = ev.down && fsm.pressed().contains(&ev.vk);
                        if !repeat && fsm.config().worth_logging(ev.vk, fsm.pressed()) {
                            tracing::debug!(
                                target: "vyrp::keys",
                                vk = format_args!("0x{:02X}", ev.vk),
                                key = %keys::name(ev.vk),
                                down = ev.down,
                                injected = ev.injected,
                                "KeyEvent"
                            );
                        }
                        fsm.on_key(ev, &ks)
                    }
                    Err(RecvTimeoutError::Timeout) if !fsm.pressed().is_empty() => {
                        fsm.reconcile(&ks, Instant::now())
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                for out in outputs {
                    match out {
                        Output::Gesture(g) => {
                            tracing::info!(target: "vyrp::gestures", gesture = ?g, "gesto");
                            on_gesture(g);
                        }
                        Output::Mask => hook::send_mask_key(),
                        Output::Phantom => {
                            tracing::warn!(target: "vyrp::gestures", "atalho fantasma descartado")
                        }
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(HotkeyService {
        hook: handle,
        config_tx,
    })
}
