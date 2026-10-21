//! Speech-to-text atrás de [`SttEngine`](crate::core::types::SttEngine): whisper local
//! (V9) e, depois, nuvem (V10).
//!
//! Contrato com a UI (hub):
//! - commands `model_list`, `model_download { id }`, `model_status`, `model_select { id }`;
//! - eventos `model://progress` ([`DownloadProgress`]), `model://done` / `model://error`
//!   ([`DownloadResult`]) e `stt://status` ([`whisper::SttStatus`]).
//!
//! O modelo ativo é `settings.stt_model`; ele carrega no boot (se instalado), quando o
//! download dele termina e quando o Settings muda.

pub mod catalog;
pub mod cloud;
pub mod models;
pub mod whisper;

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager, State};

use crate::settings::{self, SettingsStore};
use models::ModelStore;
use whisper::{SttStatus, WhisperEngine};

pub const EVENT_MODEL_PROGRESS: &str = "model://progress";
pub const EVENT_MODEL_DONE: &str = "model://done";
pub const EVENT_MODEL_ERROR: &str = "model://error";
pub const EVENT_STT_STATUS: &str = "stt://status";

/// Estado do Tauri: o motor local (o mesmo `Arc` serve ao orquestrador como `SttEngine`).
pub struct LocalStt(pub Arc<WhisperEngine>);

#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub size_bytes: u64,
    pub installed: bool,
    pub downloading: bool,
    /// É o `settings.stt_model`.
    pub active: bool,
    /// Sugestão para este binário (GPU ou CPU).
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub id: String,
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadResult {
    pub id: String,
    pub error: Option<String>,
}

/// Registra pasta de modelos e motor; carrega o modelo do Settings se já estiver baixado.
pub fn init(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?.join("models");
    app.manage(ModelStore::new(dir));
    let emitter = app.clone();
    let engine = WhisperEngine::new(move |status| {
        let _ = emitter.emit(EVENT_STT_STATUS, status);
    });
    app.manage(LocalStt(engine));
    ensure_loaded(app);

    let handle = app.clone();
    app.listen(settings::EVENT_CHANGED, move |_| ensure_loaded(&handle));
    Ok(())
}

/// Carrega `settings.stt_model` se ele estiver instalado e ainda não for o atual.
fn ensure_loaded(app: &AppHandle) {
    let wanted = app.state::<SettingsStore>().get().stt_model;
    let engine = &app.state::<LocalStt>().0;
    if engine.current_model().as_deref() == Some(wanted.as_str()) {
        return;
    }
    let Some(spec) = catalog::find(&wanted) else {
        tracing::warn!(model = %wanted, "modelo desconhecido no settings");
        return;
    };
    let store = app.state::<ModelStore>();
    if store.is_installed(spec) {
        engine.load(spec.id, store.path(spec));
    } else {
        tracing::info!(model = %wanted, "modelo de transcrição não instalado");
    }
}

#[tauri::command]
pub fn model_list(
    store: State<'_, ModelStore>,
    settings: State<'_, SettingsStore>,
) -> Vec<ModelEntry> {
    let active = settings.get().stt_model;
    let gpu = whisper::backend() != "cpu";
    catalog::CATALOG
        .iter()
        .map(|spec| ModelEntry {
            id: spec.id,
            label: spec.label,
            size_bytes: spec.size_bytes,
            installed: store.is_installed(spec),
            downloading: store.is_downloading(spec),
            active: spec.id == active,
            recommended: if gpu {
                spec.recommended_gpu
            } else {
                spec.recommended_cpu
            },
        })
        .collect()
}

/// Baixa e verifica o modelo; progresso em `model://progress`, fim em `model://done` ou
/// `model://error`. Se for o modelo ativo, já carrega.
#[tauri::command]
pub async fn model_download(app: AppHandle, id: String) -> Result<(), String> {
    let spec = catalog::find(&id).ok_or_else(|| format!("modelo desconhecido: {id}"))?;
    let store = app.state::<ModelStore>();
    if store.is_installed(spec) {
        let _ = app.emit(EVENT_MODEL_DONE, DownloadResult { id, error: None });
        return Ok(());
    }
    tracing::info!(model = %id, bytes = spec.size_bytes, "baixando modelo");
    let emitter = app.clone();
    let progress_id = id.clone();
    let result = store
        .download(spec, move |downloaded, total| {
            let _ = emitter.emit(
                EVENT_MODEL_PROGRESS,
                DownloadProgress {
                    id: progress_id.clone(),
                    downloaded,
                    total,
                },
            );
        })
        .await;
    match result {
        Ok(_) => {
            tracing::info!(model = %id, "modelo baixado e verificado (SHA-256)");
            let _ = app.emit(EVENT_MODEL_DONE, DownloadResult { id, error: None });
            ensure_loaded(&app);
            Ok(())
        }
        Err(err) => {
            tracing::error!(model = %id, %err, "download do modelo falhou");
            let _ = app.emit(
                EVENT_MODEL_ERROR,
                DownloadResult {
                    id,
                    error: Some(err.clone()),
                },
            );
            Err(err)
        }
    }
}

#[tauri::command]
pub fn model_status(stt: State<'_, LocalStt>) -> SttStatus {
    stt.0.status()
}

/// Troca o modelo ativo (grava no Settings; carrega se já estiver instalado).
#[tauri::command]
pub fn model_select(
    app: AppHandle,
    settings: State<'_, SettingsStore>,
    id: String,
) -> Result<SttStatus, String> {
    catalog::find(&id).ok_or_else(|| format!("modelo desconhecido: {id}"))?;
    let mut next = settings.get();
    next.stt_model = id;
    let saved = settings.set(next)?;
    let _ = app.emit(settings::EVENT_CHANGED, &saved);
    ensure_loaded(&app);
    Ok(app.state::<LocalStt>().0.status())
}
