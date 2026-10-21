//! Orquestrador do ditado (V11): gestos → áudio → STT (VAD + política) → pós-processo →
//! LLM opcional → inserção → histórico, com os estados para o overlay e os sons.
//!
//! - [`decide`]: o que cada gesto faz, dado o estado (puro).
//! - [`run_pipeline`]: do clipe ao texto colado, com dependências injetadas (testável com
//!   motores falsos) e os marcos de latência.
//! - [`Orchestrator`]: liga tudo ao app (estado do Tauri, threads, eventos, Fx).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioService;
use crate::context::ContextSource;
use crate::core::types::{
    ActionId, AppContext, AudioClip, CleanRequest, DictationState, EVENT_STATE, ErrorCode,
    GestureEvent, InsertOutcome, InsertRequest, LatencyMarks, LlmProvider, SttEngine, SttOptions,
};
use crate::core::ui_error::UiError;
use crate::insert::Inserter;
use crate::post::{PostOptions, PostProcessor};
use crate::settings::{Settings, SettingsStore};
use crate::storage::history::HistoryEntry;
use crate::storage::{Db, now_ms};

/// Quanto o estado final (pronto/erro) fica no overlay antes de voltar ao repouso.
const RESULT_LINGER: Duration = Duration::from_millis(1_500);
/// Teto do vocabulário enviado ao STT/LLM (o whisper ainda corta o prompt por tamanho).
const MAX_VOCABULARY: usize = 200;
/// Evento para o hub: o medidor de nível foi desligado porque um ditado começou.
pub const EVENT_MONITOR: &str = "audio://monitor";

/// Resultado do ditado de teste do hub/onboarding (gravou e transcreveu, sem colar).
#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub transcript: crate::core::types::Transcript,
    pub audio_ms: u64,
}

// ---------------------------------------------------------------- decisão (pura)

/// O que fazer com um gesto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Start {
        locked: bool,
    },
    /// PTT em curso vira mãos livres.
    Lock,
    Finish,
    Cancel,
    Action(ActionId),
    Ignore,
}

/// `listening`: `None` = ocioso; `Some(locked)` = ouvindo.
pub fn decide(listening: Option<bool>, gesture: GestureEvent) -> Step {
    match (listening, gesture) {
        (None, GestureEvent::PttStart) => Step::Start { locked: false },
        (None, GestureEvent::LockStart) => Step::Start { locked: true },
        (Some(false), GestureEvent::LockStart) => Step::Lock,
        (Some(_), GestureEvent::PttEnd | GestureEvent::LockEnd) => Step::Finish,
        (Some(_), GestureEvent::Cancel) => Step::Cancel,
        (None, GestureEvent::Action(a)) => Step::Action(a),
        _ => Step::Ignore,
    }
}

// ---------------------------------------------------------------- pipeline

/// Dependências do pipeline (no app: fábrica do Settings + estado do Tauri).
pub struct Deps<'a> {
    pub stt: &'a dyn SttEngine,
    pub post: &'a PostProcessor,
    pub llm: Option<&'a dyn LlmProvider>,
    pub inserter: &'a dyn Inserter,
    pub stt_name: &'a str,
    pub llm_name: Option<&'a str>,
}

pub struct Input {
    pub clip: AudioClip,
    pub context: AppContext,
    pub settings: Settings,
    pub vocabulary: Vec<String>,
    /// "ptt" ou "lock".
    pub command: &'static str,
    pub marks: LatencyMarks,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub state: DictationState,
    /// `None` quando não há o que guardar (sem fala).
    pub entry: Option<HistoryEntry>,
    pub marks: LatencyMarks,
    pub outcome: Option<InsertOutcome>,
}

fn ms_between(a: Option<Instant>, b: Option<Instant>) -> Option<i64> {
    Some(b?.saturating_duration_since(a?).as_millis() as i64)
}

/// Do clipe ao texto colado. `emit` recebe cada mudança de estado para o overlay.
pub async fn run_pipeline(deps: &Deps<'_>, input: Input, emit: &dyn Fn(DictationState)) -> Report {
    let Input {
        clip,
        context,
        settings,
        vocabulary,
        command,
        mut marks,
    } = input;
    emit(DictationState::Processing);
    let mut entry = HistoryEntry::new("", now_ms());
    entry.command = Some(command.into());
    entry.duration_ms = clip.duration_ms() as i64;
    entry.audio = settings
        .privacy
        .save_audio
        .then(|| crate::audio::wav::encode(&clip.pcm, clip.sample_rate));
    entry.engine = Some(deps.stt_name.into());
    entry.app = (!context.exe.is_empty()).then(|| context.exe.clone());
    entry.url = context.url.clone();
    entry.category = Some(format!("{:?}", context.category).to_lowercase());

    let opts = SttOptions {
        languages: settings.dictation_languages.clone(),
        vocabulary: vocabulary.clone(),
    };
    let transcript = deps.stt.transcribe(&clip, &opts).await;
    marks.t_stt_done = Some(Instant::now());
    entry.latency_stt_ms = ms_between(marks.t_release, marks.t_stt_done);
    let transcript = match transcript {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(error = %err, "ditado: transcrição falhou");
            entry.status = "stt_failed".into();
            let state = DictationState::Error {
                code: ErrorCode::SttFailed,
            };
            emit(state);
            return Report {
                state,
                entry: Some(entry),
                marks,
                outcome: None,
            };
        }
    };
    let raw = transcript.text.trim().to_string();
    if raw.is_empty() {
        tracing::info!("ditado: nada a colar (sem fala)");
        emit(DictationState::Done);
        return Report {
            state: DictationState::Done,
            entry: None,
            marks,
            outcome: None,
        };
    }
    entry.raw_text = raw.clone();
    entry.language = transcript.language.clone();

    let preceding = (!context.before.is_empty()).then(|| context.before.clone());
    let post = deps.post.process(
        &raw,
        &PostOptions {
            preceding,
            ..PostOptions::default()
        },
    );

    if post.undo_last {
        emit(DictationState::Inserting);
        let outcome = deps.inserter.undo_last();
        marks.t_inserted = Some(Instant::now());
        entry.status = "undo".into();
        let state = if outcome.ok {
            DictationState::Done
        } else {
            DictationState::Error {
                code: ErrorCode::InsertFailed,
            }
        };
        emit(state);
        return Report {
            state,
            entry: Some(entry),
            marks,
            outcome: Some(outcome),
        };
    }

    // Snippet com HTML não passa pelo LLM: a expansão é exatamente a que o usuário salvou.
    let mut text = post.text.clone();
    if let (Some(llm), None) = (deps.llm, &post.html) {
        let req = CleanRequest {
            raw: post.text.clone(),
            level: settings.cleanup,
            style: settings.style,
            context: context.clone(),
            vocabulary,
        };
        let cleaned = crate::llm::clean_with_fallback(llm, &req).await;
        marks.t_llm_done = Some(Instant::now());
        entry.latency_llm_ms = ms_between(marks.t_stt_done, marks.t_llm_done);
        if cleaned.used_llm {
            entry.llm_provider = deps.llm_name.map(str::to_string);
            entry.clean_text = Some(cleaned.text.clone());
        } else if let Some(reason) = &cleaned.fallback_reason {
            tracing::warn!(%reason, "ditado: limpeza por LLM caiu no texto sem LLM");
            entry.fallback_level = 1;
        }
        text = cleaned.text;
    }
    if entry.clean_text.is_none() && text != raw {
        entry.clean_text = Some(text.clone());
    }
    entry.style = Some(format!("{:?}", settings.style).to_lowercase());
    entry.word_count = text.split_whitespace().count() as i64;

    emit(DictationState::Inserting);
    let request = InsertRequest {
        text: text.clone(),
        html: post.html.clone(),
        method: settings.insert_method,
        press_enter: post.press_enter,
    };
    let outcome = deps.inserter.insert(&request);
    marks.t_inserted = Some(Instant::now());
    entry.latency_total_ms = marks.release_to_inserted_ms().map(|v| v as i64);
    entry.inserted_text = outcome.ok.then(|| text.clone());
    let blocked = outcome.reason.as_deref() == Some(crate::privacy::BLOCKED_REASON);
    if !outcome.ok && !blocked {
        // O overlay diz "Copiado. Cole manualmente.": que seja verdade (o inseridor
        // restaura o clipboard anterior quando a colagem falha).
        let copied = deps.inserter.copy_text(&request);
        tracing::info!(copied = copied.ok, reason = ?copied.reason, "ditado: texto copiado após falha ao colar");
    }
    entry.status = if outcome.ok {
        "inserted"
    } else if blocked {
        "blocked"
    } else {
        "insert_failed"
    }
    .into();
    let state = if outcome.ok {
        DictationState::Done
    } else if blocked {
        DictationState::Error {
            code: ErrorCode::Blocked,
        }
    } else {
        tracing::warn!(reason = ?outcome.reason, "ditado: inserção falhou");
        DictationState::Error {
            code: ErrorCode::InsertFailed,
        }
    };
    emit(state);
    Report {
        state,
        entry: Some(entry),
        marks,
        outcome: Some(outcome),
    }
}

// ---------------------------------------------------------------- orquestrador do app

struct Session {
    locked: bool,
    /// Ditado de teste do hub: gestos não mexem nele e nada é colado.
    test: bool,
    marks: LatencyMarks,
    /// Preenchido em paralelo à fala (a captura não atrasa o início).
    context: Arc<Mutex<Option<AppContext>>>,
    generation: u64,
}

pub struct Orchestrator {
    app: AppHandle,
    context: Arc<dyn ContextSource>,
    inserter: Arc<dyn Inserter>,
    live: Mutex<Option<Session>>,
    generation: Mutex<u64>,
}

impl Orchestrator {
    pub fn new(
        app: AppHandle,
        context: Arc<dyn ContextSource>,
        inserter: Arc<dyn Inserter>,
    ) -> Arc<Self> {
        Arc::new(Self {
            app,
            context,
            inserter,
            live: Mutex::new(None),
            generation: Mutex::new(0),
        })
    }

    fn emit(&self, state: DictationState) {
        let _ = self.app.emit(EVENT_STATE, state);
    }

    fn fx(&self) -> Option<tauri::State<'_, crate::sound::Fx>> {
        self.app.try_state::<crate::sound::Fx>()
    }

    fn next_generation(&self) -> u64 {
        let mut g = self.generation.lock().unwrap();
        *g += 1;
        *g
    }

    /// Entrada dos gestos (thread de gestos): só decide e despacha; nada pesado aqui.
    pub fn on_gesture(self: &Arc<Self>, gesture: GestureEvent) {
        if self.live.lock().unwrap().as_ref().is_some_and(|s| s.test) {
            tracing::debug!(?gesture, "gesto ignorado: ditado de teste do hub em curso");
            return;
        }
        let listening = self.live.lock().unwrap().as_ref().map(|s| s.locked);
        match decide(listening, gesture) {
            Step::Start { locked } => self.start(locked),
            Step::Lock => {
                if let Some(s) = self.live.lock().unwrap().as_mut() {
                    s.locked = true;
                }
                self.emit(DictationState::Listening { locked: true });
            }
            Step::Finish => self.finish(),
            Step::Cancel => self.cancel(),
            Step::Action(action) => self.action(action),
            Step::Ignore => {}
        }
    }

    /// Ações de atalho fora do ditado. Rodam numa thread própria: colar espera os
    /// modificadores soltarem, e a thread de gestos não pode bloquear.
    fn action(&self, action: ActionId) {
        let run: fn() -> InsertOutcome = match action {
            ActionId::PasteLast => crate::insert::paste_last,
            ActionId::CopyLast => crate::insert::copy_last,
            other => {
                tracing::info!(action = ?other, "ação de atalho ainda não ligada");
                return;
            }
        };
        std::thread::spawn(move || {
            let outcome = run();
            tracing::info!(?action, ok = outcome.ok, reason = ?outcome.reason, "ação de atalho");
        });
    }

    fn start(self: &Arc<Self>, locked: bool) {
        self.begin_session(locked, false);
    }

    fn begin_session(self: &Arc<Self>, locked: bool, test: bool) {
        let generation = self.next_generation();
        let slot = Arc::new(Mutex::new(None));
        let (source, fill) = (Arc::clone(&self.context), Arc::clone(&slot));
        std::thread::spawn(move || *fill.lock().unwrap() = Some(source.capture()));
        let audio = self.app.state::<AudioService>();
        // Um medidor de nível do hub/onboarding (V20) pode estar com a gravação aberta:
        // o ditado tem prioridade, começa limpo e o hub é avisado de que o medidor caiu.
        if audio.owner() == Some(crate::audio::AudioOwner::Monitor) {
            let _ = self
                .app
                .emit(EVENT_MONITOR, serde_json::json!({ "active": false }));
        }
        audio.cancel();
        audio.begin();
        if let Some(fx) = self.fx() {
            fx.on_start();
        }
        *self.live.lock().unwrap() = Some(Session {
            locked,
            test,
            marks: LatencyMarks {
                t_press: Some(Instant::now()),
                ..LatencyMarks::default()
            },
            context: slot,
            generation,
        });
        self.emit(DictationState::Listening { locked });
        tracing::info!(locked, test, "ditado: ouvindo");
    }

    /// Ditado de teste do hub/onboarding: começa a gravar (como um PTT), sem colar no fim.
    pub fn test_start(self: &Arc<Self>) -> Result<(), UiError> {
        if self.live.lock().unwrap().is_some() {
            return Err(UiError::new(
                "test.already_running",
                "já há um ditado em andamento",
            ));
        }
        self.begin_session(false, true);
        Ok(())
    }

    /// Termina o ditado de teste: VAD + STT do Settings (com o vocabulário), sem inserir
    /// nem gravar histórico. Bloqueia até a transcrição (chamar fora da thread de UI).
    pub fn test_stop(&self) -> Result<TestOutcome, UiError> {
        let session = {
            let mut live = self.live.lock().unwrap();
            match live.as_ref() {
                Some(s) if s.test => live.take().unwrap(),
                _ => {
                    return Err(UiError::new(
                        "test.not_running",
                        "nenhum ditado de teste em andamento",
                    ));
                }
            }
        };
        let audio = self.app.state::<AudioService>();
        let clip = audio.end().map_err(|e| e.ui());
        if let Some(fx) = self.fx() {
            fx.on_end();
        }
        let clip = clip?;
        if clip.pcm.is_empty() {
            self.finish_state(DictationState::Idle, session.generation);
            return Err(UiError::new(
                "test.no_audio",
                "Nenhum áudio foi captado. Confira o microfone.",
            ));
        }
        self.emit(DictationState::Processing);
        self.wait_local_model(Duration::from_secs(60));
        let settings = self.app.state::<SettingsStore>().get();
        let opts = SttOptions {
            languages: settings.dictation_languages.clone(),
            vocabulary: self.vocabulary(),
        };
        let stt = crate::factory::build_stt(&settings, &self.app);
        let audio_ms = clip.duration_ms();
        let result = tauri::async_runtime::block_on(stt.transcribe(&clip, &opts));
        let state = if result.is_ok() {
            DictationState::Done
        } else {
            DictationState::Error {
                code: ErrorCode::SttFailed,
            }
        };
        self.finish_state(state, session.generation);
        result
            .map(|transcript| TestOutcome {
                transcript,
                audio_ms,
            })
            .map_err(|e| e.ui())
    }

    /// Vocabulário do dicionário (cache do pós-processo), com teto.
    fn vocabulary(&self) -> Vec<String> {
        let mut words = self
            .app
            .try_state::<PostProcessor>()
            .map(|p| p.vocabulary())
            .unwrap_or_default();
        words.truncate(MAX_VOCABULARY);
        words
    }

    fn wait_local_model(&self, max: Duration) {
        use crate::stt::whisper::LoadState;
        let Some(local) = self.app.try_state::<crate::stt::LocalStt>() else {
            return;
        };
        let started = Instant::now();
        while local.0.status().state == LoadState::Loading && started.elapsed() < max {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn cancel(&self) {
        if self.live.lock().unwrap().take().is_none() {
            return;
        }
        self.app.state::<AudioService>().cancel();
        if let Some(fx) = self.fx() {
            fx.on_cancel();
        }
        self.emit(DictationState::Idle);
        tracing::info!("ditado: cancelado");
    }

    fn finish(self: &Arc<Self>) {
        let Some(mut session) = self.live.lock().unwrap().take() else {
            return;
        };
        if session.test {
            // Não deveria acontecer (gestos são ignorados no teste): devolve a sessão.
            *self.live.lock().unwrap() = Some(session);
            return;
        }
        session.marks.t_release = Some(Instant::now());
        let clip = self.app.state::<AudioService>().end();
        if let Some(fx) = self.fx() {
            fx.on_end();
        }
        let clip = match clip {
            Ok(clip) => clip,
            Err(err) => {
                tracing::error!(error = %err, "ditado: sem áudio");
                if let Some(fx) = self.fx() {
                    fx.on_error();
                }
                self.finish_state(
                    DictationState::Error {
                        code: ErrorCode::MicUnavailable,
                    },
                    session.generation,
                );
                return;
            }
        };
        let command = if session.locked { "lock" } else { "ptt" };
        let context = session.context.lock().unwrap().clone().unwrap_or_default();
        self.process(clip, context, command, session.marks, session.generation);
    }

    /// Diagnóstico: roda o pipeline com um clipe pronto, como se fosse um PTT.
    pub fn dictate_clip(self: &Arc<Self>, clip: AudioClip) {
        let generation = self.next_generation();
        let now = Instant::now();
        let marks = LatencyMarks {
            t_press: Some(now),
            t_release: Some(now),
            ..LatencyMarks::default()
        };
        let context = self.context.capture();
        self.process(clip, context, "ptt", marks, generation);
    }

    fn process(
        self: &Arc<Self>,
        clip: AudioClip,
        context: AppContext,
        command: &'static str,
        marks: LatencyMarks,
        generation: u64,
    ) {
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("vyrp-dictation".into())
            .spawn(move || {
                let settings = me.app.state::<SettingsStore>().get();
                if let Some(blocked) = crate::privacy::check(&settings.privacy, &context) {
                    // App/site bloqueado: o clipe é descartado sem transcrever (nada sai do micro).
                    tracing::info!(%blocked, "ditado: descartado pela lista de privacidade");
                    if let Some(fx) = me.fx() {
                        fx.on_error();
                    }
                    me.finish_state(
                        DictationState::Error {
                            code: ErrorCode::Blocked,
                        },
                        generation,
                    );
                    return;
                }
                let stt = crate::factory::build_stt(&settings, &me.app);
                let llm = crate::factory::build_llm(&settings, &me.app);
                let post = me.app.state::<PostProcessor>();
                let stt_name = match settings.stt_policy {
                    crate::stt::cloud::SttPolicy::LocalOnly => settings.stt_model.clone(),
                    policy => format!("{policy:?}/{:?}", settings.stt_cloud).to_lowercase(),
                };
                let llm_name = settings.llm_provider.map(|k| k.name().to_string());
                let deps = Deps {
                    stt: stt.as_ref(),
                    post: &post,
                    llm: llm.as_deref(),
                    inserter: me.inserter.as_ref(),
                    stt_name: &stt_name,
                    llm_name: llm_name.as_deref(),
                };
                let input = Input {
                    clip,
                    context,
                    settings: settings.clone(),
                    vocabulary: me.vocabulary(),
                    command,
                    marks,
                };
                let emit = |state: DictationState| me.emit(state);
                let report = tauri::async_runtime::block_on(run_pipeline(&deps, input, &emit));
                me.after(report, &settings, generation);
            })
            .expect("thread do ditado");
    }

    fn after(&self, report: Report, settings: &Settings, generation: u64) {
        let m = &report.marks;
        tracing::info!(
            state = ?report.state,
            press_to_release_ms = ms_between(m.t_press, m.t_release),
            release_to_stt_ms = ms_between(m.t_release, m.t_stt_done),
            stt_to_llm_ms = ms_between(m.t_stt_done, m.t_llm_done),
            release_to_inserted_ms = ms_between(m.t_release, m.t_inserted),
            outcome = ?report.outcome,
            "ditado: concluído (marcos de latência)"
        );
        if matches!(report.state, DictationState::Error { .. })
            && let Some(fx) = self.fx()
        {
            fx.on_error();
        }
        if let (Some(entry), Some(db)) = (&report.entry, self.app.try_state::<Db>())
            && let Err(err) = db.history().record(entry, settings.history_retention)
        {
            tracing::error!(error = %err, "ditado: histórico não gravado");
        }
        self.finish_state(report.state, generation);
    }

    /// Mostra o resultado e volta ao repouso, se nenhum ditado novo começou.
    fn finish_state(&self, state: DictationState, generation: u64) {
        self.emit(state);
        let app = self.app.clone();
        let current = *self.generation.lock().unwrap();
        if current != generation {
            return;
        }
        std::thread::spawn(move || {
            std::thread::sleep(RESULT_LINGER);
            if let Some(orch) = app.try_state::<Arc<Orchestrator>>()
                && *orch.generation.lock().unwrap() == generation
                && orch.live.lock().unwrap().is_none()
            {
                let _ = app.emit(EVENT_STATE, DictationState::Idle);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{InsertMethod, Result, Transcript, VyrpError};
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn decide_table() {
        use GestureEvent::*;
        assert_eq!(decide(None, PttStart), Step::Start { locked: false });
        assert_eq!(decide(None, LockStart), Step::Start { locked: true });
        assert_eq!(decide(Some(false), LockStart), Step::Lock);
        assert_eq!(decide(Some(false), PttEnd), Step::Finish);
        assert_eq!(decide(Some(true), LockEnd), Step::Finish);
        assert_eq!(decide(Some(true), Cancel), Step::Cancel);
        assert_eq!(decide(None, Cancel), Step::Ignore);
        assert_eq!(decide(None, PttEnd), Step::Ignore);
        assert_eq!(decide(Some(false), PttStart), Step::Ignore);
        assert_eq!(
            decide(None, Action(ActionId::PasteLast)),
            Step::Action(ActionId::PasteLast)
        );
    }

    struct FakeStt(&'static str);
    #[async_trait]
    impl SttEngine for FakeStt {
        async fn transcribe(&self, _: &AudioClip, _: &SttOptions) -> Result<Transcript> {
            if self.0 == "erro" {
                return Err(VyrpError::Stt("falso".into()));
            }
            Ok(Transcript {
                text: self.0.into(),
                language: Some("pt".into()),
                ..Transcript::default()
            })
        }
    }

    struct FakeLlm;
    #[async_trait]
    impl LlmProvider for FakeLlm {
        async fn clean(&self, req: &CleanRequest) -> Result<String> {
            Ok(format!("{} (limpo)", req.raw))
        }
    }

    /// `.0` = colados (e desfazer), `.1` = copiados.
    #[derive(Default)]
    struct FakeInserter(StdMutex<Vec<String>>, StdMutex<Vec<String>>);
    impl Inserter for FakeInserter {
        fn copy_text(&self, req: &InsertRequest) -> InsertOutcome {
            self.1.lock().unwrap().push(req.text.clone());
            InsertOutcome {
                ok: true,
                method: InsertMethod::Paste,
                verified: None,
                reason: None,
            }
        }
        fn insert(&self, req: &InsertRequest) -> InsertOutcome {
            self.0.lock().unwrap().push(req.text.clone());
            InsertOutcome {
                ok: true,
                method: InsertMethod::Paste,
                verified: Some(true),
                reason: None,
            }
        }
        fn undo_last(&self) -> InsertOutcome {
            self.0.lock().unwrap().push("<undo>".into());
            InsertOutcome {
                ok: true,
                method: InsertMethod::Paste,
                verified: None,
                reason: None,
            }
        }
    }

    fn input() -> Input {
        let now = Instant::now();
        Input {
            clip: AudioClip::new(vec![0; 16_000]),
            context: AppContext::default(),
            settings: Settings::default(),
            vocabulary: vec![],
            command: "ptt",
            marks: LatencyMarks {
                t_press: Some(now),
                t_release: Some(now),
                ..LatencyMarks::default()
            },
        }
    }

    fn run(stt: &str, llm: bool) -> (Report, Vec<String>, Vec<DictationState>) {
        let stt = FakeStt(Box::leak(stt.to_string().into_boxed_str()));
        let post = PostProcessor::new();
        let inserter = FakeInserter::default();
        let fake_llm = FakeLlm;
        let deps = Deps {
            stt: &stt,
            post: &post,
            llm: llm.then_some(&fake_llm as &dyn LlmProvider),
            inserter: &inserter,
            stt_name: "falso",
            llm_name: Some("falso"),
        };
        let states = StdMutex::new(Vec::new());
        let emit = |s: DictationState| states.lock().unwrap().push(s);
        let report = tauri::async_runtime::block_on(run_pipeline(&deps, input(), &emit));
        let pasted = inserter.0.lock().unwrap().clone();
        (report, pasted, states.into_inner().unwrap())
    }

    #[test]
    fn end_to_end_with_fakes() {
        let (report, pasted, states) = run("olá mundo", false);
        assert_eq!(pasted, vec!["Olá mundo".to_string()]);
        assert_eq!(
            states,
            vec![
                DictationState::Processing,
                DictationState::Inserting,
                DictationState::Done
            ]
        );
        let entry = report.entry.unwrap();
        assert_eq!(entry.raw_text, "olá mundo");
        assert_eq!(entry.inserted_text.as_deref(), Some("Olá mundo"));
        assert_eq!(entry.status, "inserted");
        assert!(entry.latency_total_ms.is_some());
        assert!(report.marks.t_stt_done.is_some() && report.marks.t_inserted.is_some());
    }

    #[test]
    fn llm_cleans_when_configured() {
        let (report, pasted, _) = run("olá mundo", true);
        assert_eq!(pasted, vec!["Olá mundo (limpo)".to_string()]);
        let entry = report.entry.unwrap();
        assert_eq!(entry.clean_text.as_deref(), Some("Olá mundo (limpo)"));
        assert_eq!(entry.llm_provider.as_deref(), Some("falso"));
        assert!(entry.latency_llm_ms.is_some());
    }

    // ---- V21: privacidade

    struct FocusedOn(&'static str);
    impl crate::context::ContextSource for FocusedOn {
        fn capture(&self) -> AppContext {
            AppContext {
                exe: self.0.into(),
                ..AppContext::default()
            }
        }
    }

    #[test]
    fn paste_into_a_blocked_app_is_refused_with_its_own_code() {
        let stt = FakeStt("olá mundo");
        let post = PostProcessor::new();
        let inner = Arc::new(FakeInserter::default());
        // O foco foi para o gerenciador de senhas entre a fala e a colagem.
        let guarded = crate::privacy::GuardedInserter::new(
            inner.clone(),
            Arc::new(FocusedOn("1Password.exe")),
            Arc::new(crate::privacy::Privacy::default),
        );
        let deps = Deps {
            stt: &stt,
            post: &post,
            llm: None,
            inserter: &guarded,
            stt_name: "falso",
            llm_name: None,
        };
        let states = StdMutex::new(Vec::new());
        let emit = |s: DictationState| states.lock().unwrap().push(s);
        let report = tauri::async_runtime::block_on(run_pipeline(&deps, input(), &emit));
        assert!(inner.0.lock().unwrap().is_empty(), "nada foi colado");
        assert_eq!(
            report.state,
            DictationState::Error {
                code: ErrorCode::Blocked
            }
        );
        let entry = report.entry.unwrap();
        assert_eq!(entry.status, "blocked");
        assert_eq!(entry.inserted_text, None);
        assert_eq!(
            entry.raw_text, "olá mundo",
            "o texto fica no histórico para copiar"
        );
    }

    #[test]
    fn history_audio_is_saved_only_when_the_user_turns_it_on() {
        let stt = FakeStt("olá");
        let post = PostProcessor::new();
        let inserter = FakeInserter::default();
        let deps = Deps {
            stt: &stt,
            post: &post,
            llm: None,
            inserter: &inserter,
            stt_name: "falso",
            llm_name: None,
        };
        let emit = |_: DictationState| {};
        let off = tauri::async_runtime::block_on(run_pipeline(&deps, input(), &emit));
        assert_eq!(off.entry.unwrap().audio, None);

        let mut with_audio = input();
        with_audio.settings.privacy.save_audio = true;
        let on = tauri::async_runtime::block_on(run_pipeline(&deps, with_audio, &emit));
        let wav = on.entry.unwrap().audio.expect("áudio guardado");
        assert_eq!(&wav[..4], b"RIFF");
    }

    #[test]
    fn screen_context_never_reaches_the_history_entry() {
        let stt = FakeStt("olá");
        let post = PostProcessor::new();
        let inserter = FakeInserter::default();
        let deps = Deps {
            stt: &stt,
            post: &post,
            llm: None,
            inserter: &inserter,
            stt_name: "falso",
            llm_name: None,
        };
        let mut with_context = input();
        with_context.context = AppContext {
            exe: "notepad.exe".into(),
            title: "TITULO-SECRETO".into(),
            before: "TEXTO-ANTES-SECRETO".into(),
            selection: "SELECAO-SECRETA".into(),
            after: "TEXTO-DEPOIS-SECRETO".into(),
            ..AppContext::default()
        };
        let emit = |_: DictationState| {};
        let report = tauri::async_runtime::block_on(run_pipeline(&deps, with_context, &emit));
        let json = serde_json::to_string(&report.entry.unwrap()).unwrap();
        for secret in ["SECRETO", "SECRETA", "ANTES", "DEPOIS"] {
            assert!(!json.contains(secret), "vazou {secret}: {json}");
        }
    }

    #[test]
    fn empty_transcript_pastes_nothing_and_keeps_no_history() {
        let (report, pasted, states) = run("   ", false);
        assert!(pasted.is_empty());
        assert!(report.entry.is_none());
        assert_eq!(states.last(), Some(&DictationState::Done));
    }

    #[test]
    fn stt_error_is_reported() {
        let (report, pasted, _) = run("erro", false);
        assert!(pasted.is_empty());
        assert_eq!(
            report.state,
            DictationState::Error {
                code: ErrorCode::SttFailed
            }
        );
        assert_eq!(report.entry.unwrap().status, "stt_failed");
    }

    /// STT/LLM que guardam o vocabulário recebido.
    #[derive(Default)]
    struct SeesVocabulary(StdMutex<Vec<Vec<String>>>);
    #[async_trait]
    impl SttEngine for SeesVocabulary {
        async fn transcribe(&self, _: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
            self.0.lock().unwrap().push(opts.vocabulary.clone());
            Ok(Transcript {
                text: "teste do vyrp".into(),
                ..Transcript::default()
            })
        }
    }
    #[async_trait]
    impl LlmProvider for SeesVocabulary {
        async fn clean(&self, req: &CleanRequest) -> Result<String> {
            self.0.lock().unwrap().push(req.vocabulary.clone());
            Ok(req.raw.clone())
        }
    }

    #[test]
    fn vocabulary_reaches_stt_and_llm() {
        let stt = SeesVocabulary::default();
        let llm = SeesVocabulary::default();
        let post = PostProcessor::new();
        let inserter = FakeInserter::default();
        let deps = Deps {
            stt: &stt,
            post: &post,
            llm: Some(&llm as &dyn LlmProvider),
            inserter: &inserter,
            stt_name: "falso",
            llm_name: Some("falso"),
        };
        let mut input = input();
        input.vocabulary = vec!["Vyrp".into(), "Tauri".into()];
        tauri::async_runtime::block_on(run_pipeline(&deps, input, &|_| {}));
        let want = vec![vec!["Vyrp".to_string(), "Tauri".to_string()]];
        assert_eq!(*stt.0.lock().unwrap(), want);
        assert_eq!(*llm.0.lock().unwrap(), want);
    }

    #[test]
    fn vocabulary_cache_follows_dictionary_changes() {
        use crate::storage::dictionary::NewDictionaryEntry;
        let db = Db::open_in_memory().unwrap();
        let post = PostProcessor::new();
        post.reload_from_db(&db).unwrap();
        assert!(post.vocabulary().is_empty());
        let word = |phrase: &str| NewDictionaryEntry {
            phrase: phrase.into(),
            manual_entry: true,
            ..Default::default()
        };
        let vyrp = db.dictionary().upsert(&word("Vyrp"), 1).unwrap();
        db.dictionary().upsert(&word("Tauri"), 2).unwrap();
        // Sem o gancho, o cache continua o antigo (é cache).
        assert!(post.vocabulary().is_empty());
        post.reload_from_db(&db).unwrap();
        let mut got = post.vocabulary();
        got.sort();
        assert_eq!(got, vec!["Tauri".to_string(), "Vyrp".to_string()]);
        db.dictionary().soft_delete(vyrp.id).unwrap();
        post.reload_from_db(&db).unwrap();
        assert_eq!(post.vocabulary(), vec!["Tauri".to_string()]);
    }

    /// Cola sempre falhando (ex.: foco mudou), mas copia.
    #[derive(Default)]
    struct FailingInserter(StdMutex<Vec<String>>);
    impl Inserter for FailingInserter {
        fn insert(&self, req: &InsertRequest) -> InsertOutcome {
            InsertOutcome {
                ok: false,
                method: req.method,
                verified: None,
                reason: Some("foco mudou".into()),
            }
        }
        fn undo_last(&self) -> InsertOutcome {
            self.insert(&InsertRequest::default())
        }
        fn copy_text(&self, req: &InsertRequest) -> InsertOutcome {
            self.0.lock().unwrap().push(req.text.clone());
            InsertOutcome {
                ok: true,
                method: req.method,
                verified: None,
                reason: None,
            }
        }
    }

    #[test]
    fn failed_paste_leaves_text_in_clipboard() {
        let stt = FakeStt("olá mundo");
        let post = PostProcessor::new();
        let inserter = FailingInserter::default();
        let deps = Deps {
            stt: &stt,
            post: &post,
            llm: None,
            inserter: &inserter,
            stt_name: "falso",
            llm_name: None,
        };
        let report = tauri::async_runtime::block_on(run_pipeline(&deps, input(), &|_| {}));
        assert_eq!(
            report.state,
            DictationState::Error {
                code: ErrorCode::InsertFailed
            }
        );
        assert_eq!(inserter.0.lock().unwrap().as_slice(), ["Olá mundo"]);
    }

    #[test]
    fn spoken_undo_calls_undo_last() {
        let (report, pasted, _) = run("apagar isso", false);
        assert_eq!(pasted, vec!["<undo>".to_string()]);
        assert_eq!(report.entry.unwrap().status, "undo");
    }
}
