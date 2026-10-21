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
use crate::core::ui_error::UiError;
use crate::hotkey::GestureConfig;
use crate::llm::LlmKind;
use crate::post::{VoiceCommand, default_voice_commands};
use crate::privacy::Privacy;
use crate::sound::{FxParams, MediaMode, MediaOptions, SoundParams};
use crate::stt::cloud::{CloudProvider, SttPolicy};

/// Versão atual do formato em disco. Some 1 a cada mudança incompatível e
/// acrescente o passo em [`migrate`].
pub const CURRENT_VERSION: u32 = 5;
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
    /// Onde transcrever: só local (padrão), só nuvem, ou nuvem com reserva local.
    pub stt_policy: SttPolicy,
    /// Provedor de STT na nuvem (a chave fica no Credential Manager).
    pub stt_cloud: CloudProvider,
    /// Limpeza por LLM: `None` = desligada (padrão; só o pós-processo local).
    pub llm_provider: Option<LlmKind>,
    /// Modelo do provedor de LLM; `None` = o padrão dele.
    pub llm_model: Option<String>,
    /// Endereço da API do LLM; `None` = o padrão do provedor. Vale para o Ollama
    /// (outra porta/máquina) e para endpoints compatíveis com a API da OpenAI.
    pub llm_url: Option<String>,
    /// O onboarding de primeira execução já foi concluído (ou pulado).
    pub onboarding_done: bool,
    /// Volume dos sons de início/fim/erro/cancelar (0..1).
    pub sound_volume: f32,
    /// O que fazer com a mídia do sistema enquanto ouve: off, pause, mute ou duck.
    pub media_mode: MediaMode,
    /// No modo duck, quanto do volume atual fica (0..1).
    pub media_duck_factor: f32,
    /// Abrir o Vyrp ao entrar no Windows (sobe só no tray, sem abrir o hub). V23.
    pub autostart: bool,
    /// Auto-aprendizado do dicionário (V24): desligado, sugerir no hub ou automático.
    pub dictionary_learning: crate::learn::LearnMode,
    /// Lista de privacidade (apps/sites bloqueados) e áudio no histórico. Campo aditivo:
    /// arquivos antigos caem nos padrões, sem migração.
    pub privacy: Privacy,
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
            ui_language: "auto".into(),
            dictation_languages: vec!["pt".into()],
            microphone: None,
            mic_ranking: Vec::new(),
            warm_mic: false,
            preroll_ms: 300,
            gestures: GestureConfig::default(),
            stt_model: "small-q5_1".into(),
            stt_policy: SttPolicy::default(),
            stt_cloud: CloudProvider::Groq,
            llm_provider: None,
            llm_model: None,
            llm_url: None,
            onboarding_done: false,
            sound_volume: 0.6,
            media_mode: MediaMode::Off,
            media_duck_factor: 0.2,
            autostart: false,
            dictionary_learning: crate::learn::LearnMode::default(),
            privacy: Privacy::default(),
            cleanup: CleanupLevel::default(),
            style: WritingStyle::default(),
            insert_method: InsertMethod::default(),
            history_retention: HistoryRetention::default(),
            voice_commands: default_voice_commands(),
            sounds: true,
        }
    }
}

/// Endereço do LLM vindo da UI: em branco = padrão (`None`); senão precisa ser http(s) e
/// sem espaços. Barras finais saem.
fn normalize_llm_url(url: Option<String>) -> Result<Option<String>, UiError> {
    let Some(raw) = url else {
        return Ok(None);
    };
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    let valid = (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && trimmed.len() > "https://".len()
        && !trimmed.contains(char::is_whitespace);
    if valid {
        Ok(Some(trimmed.to_string()))
    } else {
        Err(UiError::new(
            "settings.llm_url_invalid",
            format!("endereço do LLM inválido: {raw} (use http:// ou https://, sem espaços)"),
        )
        .with("url", raw))
    }
}

impl Settings {
    /// Valida e normaliza um valor vindo da UI. Erro = recusar a gravação.
    pub fn sanitized(mut self) -> Result<Self, UiError> {
        // Um problema por vez: a tela mostra o primeiro; corrigido, aparece o próximo.
        if let Some(first) = self.gestures.validate_ui().into_iter().next() {
            return Err(first);
        }
        self.gestures.clamp_timings();
        self.sound_volume = self.sound_volume.clamp(0.0, 1.0);
        self.media_duck_factor = self.media_duck_factor.clamp(0.0, 1.0);
        if crate::stt::catalog::find(&self.stt_model).is_none() {
            return Err(UiError::new(
                "settings.unknown_model",
                format!("modelo de transcrição desconhecido: {}", self.stt_model),
            )
            .with("id", &self.stt_model));
        }
        self.llm_url = normalize_llm_url(self.llm_url.take())?;
        self.privacy = self.privacy.clone().normalized();
        if !matches!(self.ui_language.as_str(), "auto" | "pt-BR" | "en") {
            self.ui_language = "auto".into();
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
    if version == 2 {
        // v3: provedores (stt_policy, stt_cloud, llm_provider, llm_model). Campos novos
        // entram com os padrões: tudo local, LLM desligado.
        version = 3;
    }
    if version == 3 {
        // v4: som e mídia (sound_volume, media_mode, media_duck_factor), com os padrões.
        version = 4;
    }
    if version == 4 {
        // v5: llm_url (None = padrão do provedor) e onboarding_done (false). O hub migra
        // sozinho o "onboarding concluído" que antes vivia no localStorage.
        version = 5;
    }
    obj.insert("version".into(), version.into());
    serde_json::from_value(value).map_err(|e| e.to_string())
}

impl Settings {
    /// Sons e mídia do ditado (V19).
    pub fn fx(&self) -> FxParams {
        FxParams {
            sound: SoundParams {
                enabled: self.sounds,
                volume: self.sound_volume,
            },
            media: MediaOptions {
                mode: self.media_mode,
                duck_factor: self.media_duck_factor,
            },
        }
    }

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

/// Tira o BOM UTF-8 do começo: alguns editores do Windows o gravam, e o serde o recusa.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
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
            Ok(text) => serde_json::from_str::<Value>(strip_bom(&text))
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
    pub fn set(&self, next: Settings) -> Result<Settings, UiError> {
        let next = next.sanitized()?;
        write_atomic(&self.path, &next).map_err(|e| UiError::raw(e.to_string()))?;
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
) -> Result<Settings, UiError> {
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
    fn v2_file_gets_local_only_providers() {
        let s = migrate(json!({"version": 2, "sounds": false, "stt_model": "base"})).unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert_eq!(s.stt_policy, SttPolicy::LocalOnly);
        assert_eq!(s.stt_cloud, CloudProvider::Groq);
        assert_eq!(s.llm_provider, None);
        assert_eq!(s.stt_model, "base");
        let full = migrate(json!({
            "version": 3,
            "stt_policy": "cloud_with_fallback",
            "stt_cloud": "open_ai",
            "llm_provider": "anthropic",
            "llm_model": "claude-haiku-5-5"
        }))
        .unwrap();
        assert_eq!(full.stt_policy, SttPolicy::CloudWithFallback);
        assert_eq!(full.stt_cloud, CloudProvider::OpenAi);
        assert_eq!(full.llm_provider, Some(LlmKind::Anthropic));
    }

    #[test]
    fn a_utf8_bom_at_the_start_of_the_file_is_ignored() {
        let path = temp_path();
        let body = serde_json::to_string_pretty(&json!({
            "version": CURRENT_VERSION,
            "ui_language": "en",
        }))
        .unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("\u{feff}{body}")).unwrap();
        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get().ui_language, "en");
        assert!(!path.with_extension("json.bak").exists());
        // Ao gravar, o arquivo sai sem BOM.
        store.set(store.get()).unwrap();
        let written = std::fs::read(&path).unwrap();
        assert!(!written.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert_eq!(strip_bom("\u{feff}{}"), "{}");
        assert_eq!(strip_bom("{}"), "{}");
    }

    #[test]
    fn validation_errors_reach_the_ui_as_codes() {
        let bad_url = Settings {
            llm_url: Some("ftp://x".into()),
            ..Settings::default()
        }
        .sanitized()
        .unwrap_err();
        assert_eq!(bad_url.code, "settings.llm_url_invalid");
        assert_eq!(bad_url.params["url"], "ftp://x");
        let model = Settings {
            stt_model: "nao-existe".into(),
            ..Settings::default()
        }
        .sanitized()
        .unwrap_err();
        assert_eq!(model.code, "settings.unknown_model");
        assert_eq!(model.params["id"], "nao-existe");
        let mut shortcuts = Settings::default();
        shortcuts.gestures.ptt = crate::hotkey::gesture::Chord(vec![0x41]);
        let err = shortcuts.sanitized().unwrap_err();
        assert!(err.code.starts_with("shortcut."), "{}", err.code);
    }

    #[test]
    fn ui_language_defaults_to_auto_and_unknown_values_fall_back() {
        assert_eq!(Settings::default().ui_language, "auto");
        let with = |lang: &str| {
            Settings {
                ui_language: lang.into(),
                ..Settings::default()
            }
            .sanitized()
            .unwrap()
            .ui_language
        };
        assert_eq!(with("en"), "en");
        assert_eq!(with("pt-BR"), "pt-BR");
        assert_eq!(with("auto"), "auto");
        assert_eq!(with("klingon"), "auto");
        // Arquivo antigo com "pt-BR" explícito continua em português.
        let old = migrate(json!({"version": 5, "ui_language": "pt-BR"})).unwrap();
        assert_eq!(old.ui_language, "pt-BR");
        // Sem o campo: padrão "auto".
        assert_eq!(migrate(json!({"version": 5})).unwrap().ui_language, "auto");
    }

    #[test]
    fn privacy_defaults_without_a_version_bump_and_is_normalized_on_save() {
        // Arquivo v5 sem o campo `privacy`: padrões (gerenciadores de senha bloqueados, sem áudio).
        let s = migrate(json!({"version": 5, "sounds": false})).unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert!(s.privacy.block_defaults && !s.privacy.save_audio);
        assert!(s.privacy.apps.is_empty());
        let saved = Settings {
            privacy: Privacy {
                apps: vec!["Banco.EXE".into(), "banco".into(), "  ".into()],
                domains: vec!["https://www.Banco.com/x".into()],
                ..Privacy::default()
            },
            ..Settings::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(saved.privacy.apps, ["banco"]);
        assert_eq!(saved.privacy.domains, ["banco.com"]);
        let full = migrate(json!({
            "version": 5,
            "privacy": {"block_defaults": false, "save_audio": true, "apps": ["x"]}
        }))
        .unwrap();
        assert!(!full.privacy.block_defaults && full.privacy.save_audio);
        assert_eq!(full.privacy.apps, ["x"]);
    }

    #[test]
    fn v4_file_gets_llm_url_and_onboarding_defaults() {
        let s = migrate(json!({"version": 4, "llm_provider": "ollama", "sounds": false})).unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert_eq!(s.llm_url, None);
        assert!(!s.onboarding_done);
        assert_eq!(s.llm_provider, Some(LlmKind::Ollama));
        assert!(!s.sounds);
        let full = migrate(json!({
            "version": 5,
            "llm_url": "http://10.0.0.5:11434",
            "onboarding_done": true
        }))
        .unwrap();
        assert_eq!(full.llm_url.as_deref(), Some("http://10.0.0.5:11434"));
        assert!(full.onboarding_done);
    }

    #[test]
    fn llm_url_is_normalized_and_validated_on_save() {
        let with = |url: Option<&str>| {
            Settings {
                llm_url: url.map(Into::into),
                ..Settings::default()
            }
            .sanitized()
        };
        assert_eq!(with(None).unwrap().llm_url, None);
        assert_eq!(with(Some("   ")).unwrap().llm_url, None);
        assert_eq!(
            with(Some(" http://localhost:11434/ "))
                .unwrap()
                .llm_url
                .as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(
            with(Some("https://llm.exemplo.com/v1/chat"))
                .unwrap()
                .llm_url
                .as_deref(),
            Some("https://llm.exemplo.com/v1/chat")
        );
        for bad in [
            "localhost:11434",
            "ftp://x",
            "http://",
            "http://a b",
            "javascript:alert(1)",
        ] {
            assert!(with(Some(bad)).is_err(), "{bad} devia ser recusado");
        }
    }

    #[test]
    fn onboarding_defaults_to_not_done_and_round_trips() {
        let d = Settings::default();
        assert!(!d.onboarding_done);
        let on = Settings {
            onboarding_done: true,
            ..d
        };
        let back: Settings = serde_json::from_value(serde_json::to_value(&on).unwrap()).unwrap();
        assert!(back.onboarding_done);
    }

    #[test]
    fn v3_file_gets_sound_defaults() {
        let s = migrate(json!({"version": 3, "sounds": false})).unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert_eq!(s.media_mode, MediaMode::Off);
        assert!((s.sound_volume - 0.6).abs() < 1e-6);
        assert!((s.media_duck_factor - 0.2).abs() < 1e-6);
        let fx = s.fx();
        assert!(!fx.sound.enabled);
        let loud = Settings {
            sound_volume: 7.0,
            ..Settings::default()
        }
        .sanitized()
        .unwrap();
        assert!((loud.sound_volume - 1.0).abs() < 1e-6);
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
