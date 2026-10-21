//! Contratos entre módulos (spec `docs/re/wispr-flow-spec.md` §5.2).
//!
//! Quem mudar um tipo daqui muda a interface entre cards: avise o Cérebro.

use std::ops::Range;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Taxa de amostragem única do pipeline (mono, PCM i16).
pub const SAMPLE_RATE: u32 = 16_000;

/// Nome do evento Tauri com o estado do ditado (payload: [`DictationState`]).
pub const EVENT_STATE: &str = "dictation://state";
/// Nome do evento Tauri com o nível de áudio (payload: [`AudioLevel`]).
pub const EVENT_LEVEL: &str = "dictation://level";

// ---------------------------------------------------------------- erros

#[derive(Debug, thiserror::Error)]
pub enum VyrpError {
    #[error("áudio: {0}")]
    Audio(String),
    #[error("speech-to-text: {0}")]
    Stt(String),
    #[error("LLM: {0}")]
    Llm(String),
    #[error("inserção: {0}")]
    Insert(String),
    #[error("contexto: {0}")]
    Context(String),
    #[error("tempo esgotado: {0}")]
    Timeout(String),
    /// Falha de um provedor (STT/LLM) com código estável para a tela; `kind` só prefixa o log.
    #[error("{kind}: {error}")]
    Provider {
        kind: &'static str,
        error: crate::core::ui_error::UiError,
    },
    #[error("cancelado")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl VyrpError {
    /// A versão para a tela: o código do provedor, ou só o texto quando não há tradução.
    pub fn ui(&self) -> crate::core::ui_error::UiError {
        match self {
            VyrpError::Provider { error, .. } => error.clone(),
            other => crate::core::ui_error::UiError::raw(other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, VyrpError>;

// ---------------------------------------------------------------- teclado (hook → FSM)

/// Evento cru do hook de teclado/mouse (V5 → V6). `vk` = virtual-key code do Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u16,
    pub down: bool,
    /// Gerado por SendInput (inclusive pelo próprio Vyrp): a FSM ignora.
    pub injected: bool,
    pub t: Instant,
}

/// Ações que um atalho pode disparar além do ditado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    PasteLast,
    CopyLast,
    Transform,
    TransformPrompt(u8),
    OpenHub,
}

/// Gesto reconhecido pela máquina de gestos (V6 → V11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "action", rename_all = "snake_case")]
pub enum GestureEvent {
    PttStart,
    PttEnd,
    LockStart,
    LockEnd,
    Cancel,
    Action(ActionId),
}

// ---------------------------------------------------------------- estado do ditado

/// Código de erro estável, usado pela UI e pelas notificações.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    MicUnavailable,
    NoAudio,
    SttFailed,
    LlmFailed,
    InsertFailed,
    Timeout,
    /// Ditado/colagem recusado pela lista de privacidade (app ou site bloqueado).
    Blocked,
}

/// Estado emitido em [`EVENT_STATE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DictationState {
    Idle,
    Arming,
    Listening { locked: bool },
    Processing,
    Inserting,
    Done,
    Error { code: ErrorCode },
}

/// Payload de [`EVENT_LEVEL`]: RMS normalizado 0..=1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioLevel {
    pub rms: f32,
}

// ---------------------------------------------------------------- áudio e STT

/// Áudio de um ditado: mono, [`SAMPLE_RATE`], com os trechos de fala do VAD (índices de amostra).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioClip {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    pub speech_segments: Vec<Range<usize>>,
}

impl AudioClip {
    pub fn new(pcm: Vec<i16>) -> Self {
        Self {
            pcm,
            sample_rate: SAMPLE_RATE,
            speech_segments: Vec::new(),
        }
    }

    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.pcm.len() as u64 * 1000 / u64::from(self.sample_rate)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SttOptions {
    /// Códigos ISO 639-1; vazio = autodetecção.
    pub languages: Vec<String>,
    /// Vocabulário do dicionário (vai no prompt inicial do whisper / da nuvem).
    pub vocabulary: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SttTimings {
    pub load_ms: u64,
    pub decode_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    pub timings: SttTimings,
}

/// Motor de STT (local whisper no V9, nuvem no V10).
#[async_trait]
pub trait SttEngine: Send + Sync {
    async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript>;
}

// ---------------------------------------------------------------- contexto do app em foco

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Work,
    Email,
    Personal,
    #[default]
    Other,
}

/// Contexto capturado pelo V12 (nunca persistido por padrão).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppContext {
    pub exe: String,
    pub title: String,
    pub url: Option<String>,
    pub category: AppCategory,
    pub editable: bool,
    pub before: String,
    pub selection: String,
    pub after: String,
    /// Foco num campo de senha (UIA `IsPassword`). Com `true`, nada do campo é lido e a
    /// privacidade bloqueia ditado e colagem.
    #[serde(default)]
    pub password: bool,
}

// ---------------------------------------------------------------- limpeza por LLM

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupLevel {
    None,
    #[default]
    Light,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritingStyle {
    #[default]
    Formal,
    Casual,
    VeryCasual,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CleanRequest {
    pub raw: String,
    pub level: CleanupLevel,
    pub style: WritingStyle,
    pub context: AppContext,
    pub vocabulary: Vec<String>,
}

/// Provedor de limpeza (Claude / OpenAI / Ollama no V13).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn clean(&self, req: &CleanRequest) -> Result<String>;
}

// ---------------------------------------------------------------- inserção

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertMethod {
    #[default]
    Auto,
    Paste,
    ShiftInsert,
    Type,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertRequest {
    pub text: String,
    pub html: Option<String>,
    pub method: InsertMethod,
    pub press_enter: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertOutcome {
    pub ok: bool,
    pub method: InsertMethod,
    /// `None` = não foi possível verificar (sem UIA no alvo).
    pub verified: Option<bool>,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------- latência

/// Marcos de um ditado (gravados no histórico pelo V11/V22).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LatencyMarks {
    pub t_press: Option<Instant>,
    pub t_first_audio: Option<Instant>,
    pub t_release: Option<Instant>,
    pub t_stt_done: Option<Instant>,
    pub t_llm_done: Option<Instant>,
    pub t_inserted: Option<Instant>,
}

impl LatencyMarks {
    /// Caminho crítico: soltar a tecla → texto inserido.
    pub fn release_to_inserted_ms(&self) -> Option<u128> {
        Some(self.t_inserted?.duration_since(self.t_release?).as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dictation_state_serializes_with_tag() {
        let json = serde_json::to_value(DictationState::Listening { locked: true }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"state": "listening", "locked": true})
        );
        let err = serde_json::to_value(DictationState::Error {
            code: ErrorCode::NoAudio,
        })
        .unwrap();
        assert_eq!(
            err,
            serde_json::json!({"state": "error", "code": "no_audio"})
        );
    }

    #[test]
    fn gesture_event_round_trips() {
        for ev in [
            GestureEvent::PttStart,
            GestureEvent::Cancel,
            GestureEvent::Action(ActionId::TransformPrompt(3)),
        ] {
            let json = serde_json::to_string(&ev).unwrap();
            assert_eq!(serde_json::from_str::<GestureEvent>(&json).unwrap(), ev);
        }
    }

    #[test]
    fn audio_clip_duration() {
        let clip = AudioClip::new(vec![0; SAMPLE_RATE as usize / 2]);
        assert_eq!(clip.duration_ms(), 500);
        assert_eq!(clip.sample_rate, SAMPLE_RATE);
    }

    #[test]
    fn latency_release_to_inserted() {
        let t0 = Instant::now();
        let marks = LatencyMarks {
            t_release: Some(t0),
            t_inserted: Some(t0 + Duration::from_millis(420)),
            ..Default::default()
        };
        assert_eq!(marks.release_to_inserted_ms(), Some(420));
        assert_eq!(LatencyMarks::default().release_to_inserted_ms(), None);
    }
}
