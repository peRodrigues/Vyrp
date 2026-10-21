//! Settings tipados em JSON + chaves de API no Windows Credential Manager.
//!
//! Card V3 (spec `docs/re/wispr-flow-spec.md` §6). O JSON fica em
//! `<AppData>/settings.json`; as chaves de API ficam só no Credential Manager
//! (módulo [`secrets`]) e nunca passam por aqui nem voltam à UI.

pub mod secrets;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::AudioConfig;
use crate::core::types::{CleanupLevel, InsertMethod, WritingStyle};
use crate::hotkey::GestureConfig;
use crate::post::{VoiceCommand, default_voice_commands};

/// Versão atual do formato em disco. Some 1 a cada mudança incompatível e
/// acrescente o passo em [`migrate`].
pub const CURRENT_VERSION: u32 = 2;
/// Evento Tauri emitido após cada `set_settings` (payload: [`Settings`]).
pub const EVENT_CHANGED: &str = "settings://changed";
const FILE_NAME: &str = "settings.json";

/// Política de dados do histórico (spec §1.9): guardar, apagar em 24 h ou nunca gravar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRetention {
    #[default]
    Keep,
    Day,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// Idioma da interface (código BCP 47, ex.: "pt-BR").
    pub ui_language: String,
    /// Idiomas de ditado, o primeiro é o principal. Vazio = detectar.
    pub dictation_languages: Vec<String>,
    /// Nome do microfone escolhido; `None` = padrão do sistema.
    pub microphone: Option<String>,
    /// Ordem de preferência dos microfones quando o escolhido não está presente.
    pub mic_ranking: Vec<String>,
    /// Mantém o microfone aberto entre ditados para ter pre-roll. Desligado por
    /// padrão (privacidade; spec §5.1).
    pub warm_mic: bool,
    /// Áudio guardado antes de o atalho ser apertado (só com `warm_mic`).
    pub preroll_ms: u32,
    /// Atalhos (PTT, trava, cancelar, ações) e tempos dos gestos (V6).
    pub gestures: GestureConfig,
    /// Modelo whisper local ativo (id do catálogo em `stt::catalog`).
    pub stt_model: String,
    pub cleanup: CleanupLevel,
    pub style: WritingStyle,
    pub insert_method: InsertMethod,
    /// Quanto tempo o histórico de ditados fica guardado.
    pub history_retention: HistoryRetention,
    /// Comandos de voz ("nova linha"...). Lista vazia desliga todos.
    pub voice_commands: Vec<VoiceCommand>,
    /// Sons de início/fim do ditado.
    pub sounds: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            ui_language: "pt-BR".into(),
            dictation_languages: vec!["pt".into()],
            microphone: None,
            mic_ranking: Vec::new(),
            warm_mic: false,
            preroll_ms: 300,
            gestures: GestureConfig::default(),
            stt_model: "small-q5_1".into(),
            cleanup: CleanupLevel::default(),
            style: WritingStyle::default(),
            insert_method: InsertMethod::default(),
            history_retention: HistoryRetention::default(),
            voice_commands: default_voice_commands(),
            sounds: true,
        }
    }
}

impl Settings {
    /// Valida e normaliza um valor vindo da UI. Erro = recusar a gravação.
    pub fn sanitized(mut self) -> Result<Self, String> {
        let problems = self.gestures.validate();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        self.gestures.clamp_timings();
        if crate::stt::catalog::find(&self.stt_model).is_none() {
            return Err(format!(
                "modelo de transcrição desconhecido: {}",
                self.stt_model
            ));
        }
        self.version = CURRENT_VERSION;
        self.preroll_ms = self.preroll_ms.clamp(0, 2_000);
        self.dictation_languages.dedup();
        Ok(self)
    }
}

/// Leva um JSON de qualquer versão antiga até [`CURRENT_VERSION`] e o lê.
/// Versão maior que a conhecida é erro: não sobrescrevemos o que não entendemos.
pub fn migrate(mut value: Value) -> Result<Settings, String> {
    let obj = value
        .as_object_mut()
        .ok_or("o arquivo de settings não é um objeto JSON")?;
    let mut version = obj.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version > CURRENT_VERSION {
        return Err(format!(
            "settings na versão {version}, mais nova que a {CURRENT_VERSION} deste Vyrp"
        ));
    }
    if version == 0 {
        // v0: um único idioma em `language`; v1: lista em `dictation_languages`.
        if let Some(Value::String(lang)) = obj.remove("language") {
            obj.insert(
                "dictation_languages".into(),
                Value::Array(vec![lang.into()]),
            );
        }
        version = 1;
    }
    if version == 1 {
        // v1: atalho solto (`hotkey`, `hold_min_ms`, `double_tap_ms`);
        // v2: tudo em `gestures` (GestureConfig, validado pela máquina de gestos).
        let mut gestures =
            serde_json::to_value(GestureConfig::default()).map_err(|e| e.to_string())?;
        for (old, new) in [
            ("hotkey", "ptt"),
            ("hold_min_ms", "min_hold_ms"),
            ("double_tap_ms", "double_tap_ms"),
        ] {
            if let Some(v) = obj.remove(old) {
                gestures[new] = v;
            }
        }
        obj.entry("gestures").or_insert(gestures);
        version = 2;
    }
    obj.insert("version".into(), version.into());
    serde_json::from_value(value).map_err(|e| e.to_string())
}

impl Settings {
    /// Parte do Settings que a captura de áudio usa.
    pub fn audio(&self) -> AudioConfig {
        AudioConfig {
            preferred: self.microphone.clone(),
            ranking: self.mic_ranking.clone(),
            warm_mic: self.warm_mic,
            preroll_ms: self.preroll_ms,
        }
    }
}

/// Settings em memória + persistência atômica em disco.
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    /// Lê `path`. Arquivo ausente = defaults. Arquivo ilegível ou de versão
    /// futura é guardado como `.bak` (nada é perdido) e os defaults entram.
    pub fn load(path: PathBuf) -> Self {
        let settings = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Value>(&text)
                .map_err(|e| e.to_string())
                .and_then(migrate)
                .unwrap_or_else(|err| {
                    tracing::warn!(%err, "settings inválidos; usando defaults e guardando .bak");
                    let _ = std::fs::rename(&path, path.with_extension("json.bak"));
                    Settings::default()
                }),
            Err(_) => Settings::default(),
        };
        Self {
            path,
            current: Mutex::new(settings),
        }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    /// Valida, grava em disco (tmp + rename) e só então troca o valor em memória.
    pub fn set(&self, next: Settings) -> Result<Settings, String> {
        let next = next.sanitized()?;
        write_atomic(&self.path, &next).map_err(|e| e.to_string())?;
        *self.current.lock().unwrap() = next.clone();
        Ok(next)
    }
}

fn write_atomic(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, path)
}

/// Registra o store e os segredos como estado do Tauri. Chamar no `setup`.
pub fn init(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    app.manage(SettingsStore::load(dir.join(FILE_NAME)));
    app.manage(secrets::Secrets::system());
    Ok(())
}

#[tauri::command]
pub fn get_settings(store: State<'_, SettingsStore>) -> Settings {
    store.get()
}

#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    store: State<'_, SettingsStore>,
    settings: Settings,
) -> Result<Settings, String> {
    let saved = store.set(settings)?;
    let _ = app.emit(EVENT_CHANGED, &saved);
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_path() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vyrp-settings-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        dir.join(FILE_NAME)
    }

    #[test]
    fn defaults_are_valid_and_current() {
        let d = Settings::default();
        assert_eq!(d.version, CURRENT_VERSION);
        assert_eq!(d.clone().sanitized().unwrap(), d);
    }

    #[test]
    fn empty_object_yields_defaults() {
        assert_eq!(migrate(json!({"version": 2})).unwrap(), Settings::default());
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_fields() {
        let s = migrate(json!({"version": 2, "sounds": false})).unwrap();
        assert!(!s.sounds);
        assert_eq!(s.gestures, Settings::default().gestures);
    }

    #[test]
    fn v0_single_language_migrates_to_list() {
        let s = migrate(json!({"language": "en", "sounds": false})).unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert_eq!(s.dictation_languages, vec!["en".to_string()]);
        assert!(!s.sounds);
    }

    #[test]
    fn retention_defaults_to_keep_and_reads_old_files() {
        assert_eq!(
            Settings::default().history_retention,
            HistoryRetention::Keep
        );
        let s = migrate(json!({"version": 2, "history_retention": "day"})).unwrap();
        assert_eq!(s.history_retention, HistoryRetention::Day);
    }

    #[test]
    fn newer_version_is_refused() {
        assert!(migrate(json!({"version": CURRENT_VERSION + 1})).is_err());
    }

    #[test]
    fn invalid_hotkeys_are_refused() {
        use crate::hotkey::gesture::Chord;
        let with = |ptt: Vec<u16>| Settings {
            gestures: GestureConfig {
                ptt: Chord(ptt),
                ..GestureConfig::default()
            },
            ..Settings::default()
        };
        assert!(with(vec![]).sanitized().is_err());
        assert!(with(vec![0x11, 0x10, 0x12, 0x41]).sanitized().is_err());
        assert!(with(vec![0x5B, 0x5B]).sanitized().is_err());
        assert!(with(vec![0x41]).sanitized().is_err(), "sem modificador");
        assert!(with(vec![0xA3, 0xA5]).sanitized().is_ok());
    }

    #[test]
    fn v1_hotkey_fields_migrate_into_gestures() {
        let s = migrate(json!({
            "version": 1,
            "hotkey": [0xA3, 0xA5],
            "hold_min_ms": 200,
            "double_tap_ms": 400,
            "sounds": false
        }))
        .unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert_eq!(s.gestures.ptt.0, vec![0xA3, 0xA5]);
        assert_eq!(s.gestures.min_hold_ms, 200);
        assert_eq!(s.gestures.double_tap_ms, 400);
        assert_eq!(s.gestures.lock, GestureConfig::default().lock);
        assert!(!s.sounds);
        assert!(!s.warm_mic, "mic quente continua desligado");
    }

    #[test]
    fn numbers_are_clamped() {
        let s = Settings {
            gestures: GestureConfig {
                double_tap_ms: 5,
                ..GestureConfig::default()
            },
            preroll_ms: 99_999,
            ..Settings::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(s.gestures.double_tap_ms, 100);
        assert_eq!(s.preroll_ms, 2_000);
    }

    #[test]
    fn store_persists_across_loads() {
        let path = temp_path();
        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get(), Settings::default());
        store
            .set(Settings {
                sounds: false,
                ..Settings::default()
            })
            .unwrap();
        assert!(!SettingsStore::load(path).get().sounds);
    }

    #[test]
    fn corrupt_file_is_kept_as_bak_and_defaults_load() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ não é json").unwrap();
        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get(), Settings::default());
        assert!(path.with_extension("json.bak").exists());
    }

    #[test]
    fn rejected_set_keeps_previous_value() {
        let store = SettingsStore::load(temp_path());
        let bad = Settings {
            gestures: GestureConfig {
                ptt: crate::hotkey::gesture::Chord(vec![]),
                ..GestureConfig::default()
            },
            ..Settings::default()
        };
        assert!(store.set(bad).is_err());
        assert_eq!(store.get(), Settings::default());
    }
}
