//! Transformação explícita da seleção. Contexto só em memória; histórico isolado.
use crate::core::{
    types::{ActionId, DictationState, EVENT_STATE, ErrorCode, GestureEvent, SttOptions},
    ui_error::UiError,
};
#[cfg(windows)]
use crate::insert::selection::{self, Selection};
use crate::llm::{
    LlmCleaner, LlmConfig,
    transform::{MAX_INSTRUCTION, error},
};
use crate::settings::{Settings, SettingsStore};
use crate::storage::{
    Db, now_ms,
    transformations::{SavedPrompt, TransformStatus, Transformation},
};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Listener, Manager};

pub const EVENT: &str = "transform://state";
#[cfg(all(windows, feature = "dev-tools"))]
pub mod diagnostic;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Capturing,
    Listening,
    Processing,
    Inserting,
}
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub phase: Phase,
    pub error: Option<UiError>,
    pub entry_id: Option<String>,
}
#[derive(Default)]
struct Live {
    status: Status,
    cancelled: bool,
    generation: u64,
    #[cfg(windows)]
    selection: Option<Selection>,
}
#[derive(Default)]
pub struct Transform {
    live: Mutex<Live>,
}

pub fn active(app: &AppHandle) -> bool {
    app.try_state::<Arc<Transform>>()
        .is_some_and(|s| s.live.lock().unwrap().status.phase != Phase::Idle)
}

pub fn init(app: &AppHandle) {
    app.manage(Arc::new(Transform::default()));
    purge(app);
    let handle = app.clone();
    app.listen(crate::settings::EVENT_CHANGED, move |_| purge(&handle));
}

fn purge(app: &AppHandle) {
    if let Some(db) = app.try_state::<Db>() {
        let retention = app.state::<SettingsStore>().get().history_retention;
        if db.transformations().purge(retention, now_ms()).is_err() {
            tracing::warn!("retenção de transformações indisponível");
        }
    }
}

pub fn handle_gesture(app: &AppHandle, gesture: GestureEvent) -> bool {
    let Some(service) = app.try_state::<Arc<Transform>>() else {
        return false;
    };
    match gesture {
        GestureEvent::Action(ActionId::Transform) => {
            service.start(app, None);
            true
        }
        GestureEvent::Action(ActionId::TransformPrompt(slot)) => {
            service.start(app, Some(slot));
            true
        }
        GestureEvent::Cancel if active(app) => {
            service.cancel(app);
            true
        }
        _ => active(app),
    }
}

impl Transform {
    fn notify(&self, app: &AppHandle) {
        let status = self.live.lock().unwrap().status.clone();
        let state = match status.phase {
            Phase::Idle if status.error.is_some() => DictationState::Error {
                code: ErrorCode::InsertFailed,
            },
            Phase::Idle => DictationState::Idle,
            Phase::Listening => DictationState::Listening { locked: true },
            Phase::Inserting => DictationState::Inserting,
            _ => DictationState::Processing,
        };
        let _ = app.emit(EVENT, &status);
        let _ = app.emit(EVENT_STATE, state);
    }

    fn complete(&self, app: &AppHandle, result: Result<Option<String>, UiError>) {
        let mut live = self.live.lock().unwrap();
        live.status.phase = Phase::Idle;
        #[cfg(windows)]
        {
            live.selection = None;
        }
        match result {
            Ok(id) => {
                live.status.error = None;
                live.status.entry_id = id;
            }
            Err(e) => {
                live.status.error = Some(e);
            }
        }
        drop(live);
        self.notify(app);
    }

    fn cancel(&self, app: &AppHandle) {
        let mut live = self.live.lock().unwrap();
        live.cancelled = true;
        if live.status.phase == Phase::Listening {
            app.state::<crate::audio::AudioService>().cancel();
            if let Some(fx) = app.try_state::<crate::sound::Fx>() {
                fx.on_cancel();
            }
            live.status.phase = Phase::Idle;
            #[cfg(windows)]
            {
                live.selection = None;
            }
        }
        drop(live);
        self.notify(app);
    }

    #[cfg(windows)]
    fn start(self: &Arc<Self>, app: &AppHandle, slot: Option<u8>) {
        let mut live = self.live.lock().unwrap();
        if live.status.phase == Phase::Listening && slot.is_none() {
            let Some(selected) = live.selection.take() else {
                return;
            };
            live.status.phase = Phase::Processing;
            drop(live);
            self.notify(app);
            let me = self.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                let result = (|| {
                    let clip = app
                        .state::<crate::audio::AudioService>()
                        .end()
                        .map_err(|_| error("transform.audio"))?;
                    if let Some(fx) = app.try_state::<crate::sound::Fx>() {
                        fx.on_end();
                    }
                    let settings = app.state::<SettingsStore>().get();
                    selected
                        .permitted(&settings.privacy)
                        .map_err(|_| error("transform.selection"))?;
                    if me.live.lock().unwrap().cancelled {
                        return Err(error("transform.cancelled"));
                    }
                    let engine = crate::factory::build_stt(&settings, &app);
                    let transcript = tauri::async_runtime::block_on(engine.transcribe(
                        &clip,
                        &SttOptions {
                            languages: settings.dictation_languages.clone(),
                            vocabulary: Vec::new(),
                        },
                    ))
                    .map_err(|_| error("transform.audio"))?;
                    me.execute(&app, selected, transcript.text, None)
                })();
                me.complete(&app, result);
            });
            return;
        }
        if live.status.phase != Phase::Idle {
            return;
        }
        if app
            .try_state::<Arc<crate::dictation::Orchestrator>>()
            .is_some_and(|s| s.is_busy())
        {
            live.status.error = Some(error("transform.busy"));
            drop(live);
            self.notify(app);
            return;
        }
        live.cancelled = false;
        live.generation += 1;
        crate::learn::new_dictation();
        let generation = live.generation;
        live.status = Status {
            phase: Phase::Capturing,
            ..Default::default()
        };
        drop(live);
        self.notify(app);
        let me = self.clone();
        let app = app.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let settings = app.state::<SettingsStore>().get();
                if settings.llm_provider.is_none() {
                    return Err(error("transform.provider"));
                }
                let instruction = match slot {
                    Some(slot) if (1..=9).contains(&slot) => Some(
                        app.try_state::<Db>()
                            .ok_or_else(|| error("transform.storage"))?
                            .transformations()
                            .prompts()
                            .map_err(|_| error("transform.storage"))?
                            .into_iter()
                            .find(|p| p.slot == slot && !p.instruction.trim().is_empty())
                            .ok_or_else(|| error("transform.prompt"))?
                            .instruction,
                    ),
                    Some(_) => return Err(error("transform.prompt")),
                    None => None,
                };
                let selected = selection::selected(settings.privacy.clone(), None)
                    .map_err(|_| error("transform.selection"))?;
                if let Some(instruction) = instruction {
                    return me.execute(&app, selected, instruction, slot).map(Some);
                }
                let mut live = me.live.lock().unwrap();
                if live.cancelled {
                    return Err(error("transform.cancelled"));
                }
                let audio = app.state::<crate::audio::AudioService>();
                if audio.owner() == Some(crate::audio::AudioOwner::Dictation) {
                    return Err(error("transform.busy"));
                }
                audio.cancel();
                audio.begin();
                let _ = app.emit(
                    crate::dictation::EVENT_MONITOR,
                    serde_json::json!({"active":false}),
                );
                if let Some(fx) = app.try_state::<crate::sound::Fx>() {
                    fx.on_start();
                }
                live.selection = Some(selected);
                live.status.phase = Phase::Listening;
                drop(live);
                me.notify(&app);
                // No unbounded microphone session when a shortcut-up is lost.
                let timer = me.clone();
                let handle = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(60));
                    let live = timer.live.lock().unwrap();
                    let expired =
                        live.generation == generation && live.status.phase == Phase::Listening;
                    drop(live);
                    if expired {
                        timer.cancel(&handle);
                    }
                });
                Ok(None)
            })();
            match result {
                Ok(None) => {}
                Ok(Some(id)) => me.complete(&app, Ok(id)),
                Err(e) => me.complete(&app, Err(e)),
            }
        });
    }

    #[cfg(not(windows))]
    fn start(self: &Arc<Self>, app: &AppHandle, _slot: Option<u8>) {
        self.complete(app, Err(error("transform.selection")));
    }

    #[cfg(windows)]
    fn execute(
        &self,
        app: &AppHandle,
        selected: Selection,
        instruction: String,
        slot: Option<u8>,
    ) -> Result<Option<String>, UiError> {
        let settings = app.state::<SettingsStore>().get();
        selected
            .permitted(&settings.privacy)
            .map_err(|_| error("transform.selection"))?;
        {
            let mut live = self.live.lock().unwrap();
            if live.cancelled {
                return Err(error("transform.cancelled"));
            }
            live.status.phase = Phase::Processing;
        }
        self.notify(app);
        let cleaner = cleaner(&settings, app)?;
        let output =
            tauri::async_runtime::block_on(cleaner.transform(&selected.text, &instruction))?;
        // Cancellation and insertion are serialized: once Inserting starts there is no retry.
        let mut live = self.live.lock().unwrap();
        if live.cancelled {
            return Err(error("transform.cancelled"));
        }
        live.status.phase = Phase::Inserting;
        let _ = app.emit(EVENT, live.status.clone());
        let _ = app.emit(EVENT_STATE, DictationState::Inserting);
        let privacy = app.state::<SettingsStore>().get().privacy;
        let outcome = selection::replace(selected.clone(), output.clone(), privacy);
        let entry = Transformation {
            id: selected.id.clone(),
            created_at: now_ms(),
            original_text: selected.text,
            instruction,
            transformed_text: output,
            prompt_slot: slot,
            status: if outcome.ok {
                TransformStatus::Inserted
            } else {
                TransformStatus::InsertFailed
            },
            provider: cleaner.kind().name().into(),
        };
        let retention = app.state::<SettingsStore>().get().history_retention;
        app.try_state::<Db>()
            .ok_or_else(|| error("transform.storage"))?
            .transformations()
            .insert(&entry, retention)
            .map_err(|_| error("transform.storage"))?;
        if !outcome.ok {
            return Err(error("transform.insert"));
        }
        Ok(Some(entry.id))
    }
}

fn cleaner(settings: &Settings, app: &AppHandle) -> Result<LlmCleaner, UiError> {
    let kind = settings
        .llm_provider
        .ok_or_else(|| error("transform.provider"))?;
    Ok(LlmCleaner::new(
        kind,
        Arc::new(crate::stt::cloud::AppKeys(app.clone())),
        LlmConfig {
            timeout: Duration::from_secs(20),
            model: settings.llm_model.clone().filter(|v| !v.trim().is_empty()),
            url: settings
                .llm_url
                .as_ref()
                .filter(|v| !v.trim().is_empty())
                .map(|v| crate::factory::resolve_llm_url(kind, v)),
        },
    ))
}

#[tauri::command]
pub fn transform_status(app: AppHandle) -> Status {
    app.state::<Arc<Transform>>()
        .live
        .lock()
        .unwrap()
        .status
        .clone()
}
#[tauri::command]
pub fn transform_cancel(app: AppHandle) {
    app.state::<Arc<Transform>>().cancel(&app);
}
#[tauri::command]
pub fn transform_prompts(db: tauri::State<'_, Db>) -> Result<Vec<SavedPrompt>, UiError> {
    db.transformations()
        .prompts()
        .map_err(|_| error("transform.storage"))
}
#[tauri::command]
pub fn transform_save_prompt(db: tauri::State<'_, Db>, prompt: SavedPrompt) -> Result<(), UiError> {
    if !(1..=9).contains(&prompt.slot)
        || prompt.title.chars().count() > 100
        || prompt.instruction.chars().count() > MAX_INSTRUCTION
        || prompt.instruction.contains('\0')
    {
        return Err(error("transform.prompt"));
    }
    db.transformations()
        .save_prompt(&prompt)
        .map_err(|_| error("transform.storage"))
}
#[tauri::command]
pub fn transform_history(app: AppHandle, offset: u32) -> Result<Vec<Transformation>, UiError> {
    purge(&app);
    app.try_state::<Db>()
        .ok_or_else(|| error("transform.storage"))?
        .transformations()
        .list(30, offset)
        .map_err(|_| error("transform.storage"))
}
#[tauri::command]
pub fn transform_delete(db: tauri::State<'_, Db>, id: Option<String>) -> Result<(), UiError> {
    match id {
        Some(id) => db.transformations().delete(&id),
        None => db.transformations().clear(),
    }
    .map_err(|_| error("transform.storage"))
}
#[tauri::command]
pub async fn transform_undo(app: AppHandle, id: String) -> Result<(), UiError> {
    #[cfg(windows)]
    {
        if active(&app) || app.state::<Arc<crate::dictation::Orchestrator>>().is_busy() {
            return Err(error("transform.busy"));
        }
        let key = id.clone();
        let outcome = tauri::async_runtime::spawn_blocking(move || selection::undo(key))
            .await
            .map_err(|_| error("transform.undo"))?;
        if !outcome.ok {
            return Err(error("transform.undo"));
        }
        app.state::<Db>()
            .transformations()
            .mark_undone(&id)
            .map_err(|_| error("transform.storage"))
    }
    #[cfg(not(windows))]
    {
        let _ = (app, id);
        Err(error("transform.undo"))
    }
}
