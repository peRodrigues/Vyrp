//! Commands só do hub (V17a): microfones, padrões, catálogo de modelos e histórico para a UI.
//!
//! Settings e chaves de API têm os próprios commands em `settings`; o dicionário, em
//! `storage::dictionary_commands`. Aqui ficam as leituras que a tela precisa e que não
//! pertencem a nenhum desses módulos.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::audio::AudioService;
use crate::settings::Settings;
use crate::storage::{Db, HistoryEntry};

// ---------------------------------------------------------------- histórico

/// Linha do histórico para a UI: sem o blob de áudio, com o texto "final" já escolhido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub created_at: i64,
    pub raw_text: String,
    /// O que o usuário recebeu: editado > colado > limpo > cru.
    pub final_text: String,
    pub app: Option<String>,
    pub language: Option<String>,
    pub duration_ms: i64,
    pub word_count: i64,
    pub latency_total_ms: Option<i64>,
    pub status: String,
    pub reverted_ai: bool,
    pub has_audio: bool,
}

impl From<HistoryEntry> for HistoryItem {
    fn from(e: HistoryEntry) -> Self {
        let final_text = e
            .edited_text
            .or(e.inserted_text)
            .or(e.clean_text)
            .unwrap_or_else(|| e.raw_text.clone());
        Self {
            id: e.id,
            created_at: e.created_at,
            raw_text: e.raw_text,
            final_text,
            app: e.app,
            language: e.language,
            duration_ms: e.duration_ms,
            word_count: e.word_count,
            latency_total_ms: e.latency_total_ms,
            status: e.status,
            reverted_ai: e.reverted_ai,
            has_audio: e.audio.is_some(),
        }
    }
}

/// Teto por página, para a UI não pedir o banco inteiro de uma vez.
const MAX_PAGE: u32 = 200;

pub fn history_page(
    db: &Db,
    search: Option<&str>,
    limit: u32,
    offset: u32,
) -> crate::storage::Result<Vec<HistoryItem>> {
    Ok(db
        .history()
        .list(search, limit.clamp(1, MAX_PAGE), offset)?
        .into_iter()
        .map(HistoryItem::from)
        .collect())
}

#[tauri::command]
pub fn history_list(
    db: State<'_, Db>,
    search: Option<String>,
    limit: u32,
    offset: u32,
) -> Result<Vec<HistoryItem>, String> {
    history_page(&db, search.as_deref(), limit, offset).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_delete(db: State<'_, Db>, id: String) -> Result<bool, String> {
    db.history().delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_clear(db: State<'_, Db>) -> Result<u64, String> {
    db.history().clear().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- microfones e padrões

#[tauri::command]
pub fn list_microphones(audio: State<'_, AudioService>) -> Vec<String> {
    audio.devices()
}

/// Settings de fábrica, para os botões "restaurar padrão" da tela.
#[tauri::command]
pub fn settings_defaults() -> Settings {
    Settings::default()
}

// ---------------------------------------------------------------- modelos de fala

/// Modelo de voz local. O download chega com o V9 (STT local); por ora é só catálogo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttModelInfo {
    pub id: String,
    pub name: String,
    pub size_mb: u32,
    /// Resumo de uso, em português.
    pub note: String,
    pub installed: bool,
    /// `false` até o V9 existir: a UI mostra o botão desligado.
    pub downloadable: bool,
}

pub fn model_catalog() -> Vec<SttModelInfo> {
    let entry = |id: &str, name: &str, size_mb: u32, note: &str| SttModelInfo {
        id: id.into(),
        name: name.into(),
        size_mb,
        note: note.into(),
        installed: false,
        downloadable: false,
    };
    vec![
        entry("tiny", "Minúsculo", 75, "Mais rápido, erra mais."),
        entry("base", "Básico", 142, "Bom para máquinas modestas."),
        entry(
            "small",
            "Pequeno",
            466,
            "Equilíbrio entre velocidade e acerto.",
        ),
        entry(
            "medium",
            "Médio",
            1_500,
            "Mais preciso; pede boa CPU ou GPU.",
        ),
    ]
}

#[tauri::command]
pub fn stt_models() -> Vec<SttModelInfo> {
    model_catalog()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_text_prefers_edited_then_inserted_then_clean_then_raw() {
        let mut e = HistoryEntry::new("cru", 1);
        assert_eq!(HistoryItem::from(e.clone()).final_text, "cru");
        e.clean_text = Some("limpo".into());
        assert_eq!(HistoryItem::from(e.clone()).final_text, "limpo");
        e.inserted_text = Some("colado".into());
        assert_eq!(HistoryItem::from(e.clone()).final_text, "colado");
        e.edited_text = Some("editado".into());
        let item = HistoryItem::from(e);
        assert_eq!(item.final_text, "editado");
        assert_eq!(item.raw_text, "cru");
    }

    #[test]
    fn audio_blob_never_reaches_the_ui() {
        let mut e = HistoryEntry::new("oi", 1);
        e.audio = Some(vec![1; 1024]);
        let item = HistoryItem::from(e);
        assert!(item.has_audio);
        assert!(!serde_json::to_string(&item).unwrap().contains("audio\":["));
    }

    #[test]
    fn page_is_newest_first_filtered_and_capped() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..3 {
            db.history()
                .insert(&HistoryEntry::new(format!("ditado {i}"), i))
                .unwrap();
        }
        let all = history_page(&db, None, 50, 0).unwrap();
        assert_eq!(all[0].raw_text, "ditado 2");
        assert_eq!(history_page(&db, Some("ditado 1"), 50, 0).unwrap().len(), 1);
        // limit 0 vira 1; limite enorme é cortado em MAX_PAGE.
        assert_eq!(history_page(&db, None, 0, 0).unwrap().len(), 1);
        assert_eq!(history_page(&db, None, 9_999, 0).unwrap().len(), 3);
    }

    #[test]
    fn catalog_is_ordered_by_size_and_not_downloadable_yet() {
        let models = model_catalog();
        assert!(models.windows(2).all(|w| w[0].size_mb < w[1].size_mb));
        assert!(models.iter().all(|m| !m.installed && !m.downloadable));
    }

    #[test]
    fn defaults_command_matches_settings_default() {
        assert_eq!(settings_defaults(), Settings::default());
    }
}
