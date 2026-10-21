//! Overlay (V16): janela interativa (arrastar/ocultar) e simulador dos estados do ditado.
//!
//! A janela já nasce no scaffold; a UI mora em `src/overlay`. Enquanto o orquestrador (V11)
//! não existe, o simulador percorre o `DictationState` inteiro emitindo `dictation://state`
//! e `dictation://level` como ele fará. Dispare com `--diag-overlay` ou pelo command
//! `overlay_demo`.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::core::types::{AudioLevel, DictationState, EVENT_LEVEL, EVENT_STATE, ErrorCode};

const FLAG: &str = "--diag-overlay";
/// Variável opcional: milissegundos de cada passo (padrão em [`default_hold`]).
const HOLD_ENV: &str = "VYRP_DIAG_STEP_MS";
const LEVEL_TICK_MS: u64 = 33;

/// Um passo do simulador: o estado a emitir e quanto tempo ele dura.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub state: DictationState,
    /// Peso da duração (1 = passo padrão).
    pub weight: u32,
    /// Emite níveis de áudio durante o passo.
    pub speaking: bool,
}

/// Sequência completa: repouso → preparando → ouvindo (PTT) → ouvindo (travado) →
/// processando → colando → pronto → um passo para cada erro → repouso.
pub fn demo_script() -> Vec<Step> {
    let step = |state, weight, speaking| Step {
        state,
        weight,
        speaking,
    };
    let mut steps = vec![
        step(DictationState::Idle, 1, false),
        step(DictationState::Arming, 1, false),
        step(DictationState::Listening { locked: false }, 3, true),
        step(DictationState::Listening { locked: true }, 3, true),
        step(DictationState::Processing, 2, false),
        step(DictationState::Inserting, 1, false),
        step(DictationState::Done, 1, false),
    ];
    for code in [
        ErrorCode::MicUnavailable,
        ErrorCode::NoAudio,
        ErrorCode::SttFailed,
        ErrorCode::LlmFailed,
        ErrorCode::InsertFailed,
        ErrorCode::Timeout,
        ErrorCode::Blocked,
    ] {
        steps.push(step(DictationState::Error { code }, 1, false));
    }
    steps.push(step(DictationState::Idle, 1, false));
    steps
}

/// Nível de áudio simulado (0..=1) no instante `t_ms`: fala com pausas, sem aleatoriedade.
pub fn demo_level(t_ms: u64) -> f32 {
    let t = t_ms as f32 / 1000.0;
    let syllables = (t * 9.0).sin().abs();
    let words = ((t * 1.6).sin() * 0.5 + 0.5).powf(0.6);
    (0.02 + 0.22 * syllables * words).clamp(0.0, 1.0)
}

fn default_hold() -> Duration {
    let ms = std::env::var(HOLD_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1_500)
        .clamp(200, 30_000);
    Duration::from_millis(ms)
}

/// Roda o roteiro uma vez, numa thread própria. Seguro de chamar mais de uma vez.
pub fn run_demo(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let hold = default_hold();
        for step in demo_script() {
            let _ = app.emit(EVENT_STATE, step.state);
            let total = hold * step.weight;
            if !step.speaking {
                std::thread::sleep(total);
                continue;
            }
            let mut elapsed = 0u64;
            while elapsed < total.as_millis() as u64 {
                let rms = demo_level(elapsed);
                let _ = app.emit(EVENT_LEVEL, AudioLevel { rms });
                std::thread::sleep(Duration::from_millis(LEVEL_TICK_MS));
                elapsed += LEVEL_TICK_MS;
            }
        }
    });
}

#[tauri::command]
pub fn overlay_demo(app: AppHandle) {
    run_demo(&app);
}

/// Deixa o overlay clicável (arrastar e ocultar). Ele não rouba foco: a janela é
/// `focusable: false`. Com `--diag-overlay`, roda o simulador depois de a UI subir.
pub fn init(app: &AppHandle) {
    if let Some(overlay) = app.get_webview_window(crate::OVERLAY)
        && let Err(err) = overlay.set_ignore_cursor_events(false)
    {
        tracing::warn!(error = %err, "overlay continua sem receber cliques");
    }
    if std::env::args().any(|a| a == FLAG) {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            run_demo(&app);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_covers_every_state_and_error_code() {
        let script = demo_script();
        let has = |f: fn(&DictationState) -> bool| script.iter().any(|s| f(&s.state));
        assert!(has(|s| matches!(s, DictationState::Idle)));
        assert!(has(|s| matches!(s, DictationState::Arming)));
        assert!(has(|s| matches!(
            s,
            DictationState::Listening { locked: false }
        )));
        assert!(has(|s| matches!(
            s,
            DictationState::Listening { locked: true }
        )));
        assert!(has(|s| matches!(s, DictationState::Processing)));
        assert!(has(|s| matches!(s, DictationState::Inserting)));
        assert!(has(|s| matches!(s, DictationState::Done)));
        let errors = script
            .iter()
            .filter(|s| matches!(s.state, DictationState::Error { .. }))
            .count();
        assert_eq!(errors, 7);
    }

    #[test]
    fn script_starts_and_ends_at_rest_and_only_listening_speaks() {
        let script = demo_script();
        assert_eq!(script.first().unwrap().state, DictationState::Idle);
        assert_eq!(script.last().unwrap().state, DictationState::Idle);
        for step in &script {
            assert_eq!(
                step.speaking,
                matches!(step.state, DictationState::Listening { .. })
            );
        }
    }

    #[test]
    fn demo_level_stays_in_range_and_moves() {
        let values: Vec<f32> = (0..400).map(|i| demo_level(i * 33)).collect();
        assert!(values.iter().all(|v| (0.0..=1.0).contains(v)));
        let (min, max) = values
            .iter()
            .fold((1.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        assert!(max - min > 0.1, "o nível precisa variar para a waveform");
    }

    #[test]
    fn states_serialize_the_way_the_ui_expects() {
        let json = serde_json::to_value(DictationState::Error {
            code: ErrorCode::InsertFailed,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"state": "error", "code": "insert_failed"})
        );
    }
}
