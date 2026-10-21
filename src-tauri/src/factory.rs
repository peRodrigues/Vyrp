//! Fábricas dos motores a partir do Settings, para o orquestrador (V11):
//!
//! - [`build_stt`]: VAD na frente ([`VadGated`]) → política local/nuvem ([`PolicyStt`]);
//! - [`build_llm`]: limpeza por LLM, ou `None` quando desligada.
//!
//! Chamar a cada ditado (é barato): assim mudanças no Settings valem no próximo ditado.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::core::types::{CleanupLevel, LlmProvider, SttEngine};
use crate::llm::{LlmCleaner, LlmConfig, LlmKind};
use crate::settings::Settings;
use crate::stt::LocalStt;
use crate::stt::cloud::{AppKeys, CloudConfig, CloudStt, SttPolicy};
use crate::vad::{VadGated, VadService};

/// A política precisa de um motor de nuvem?
pub fn needs_cloud(policy: SttPolicy) -> bool {
    policy != SttPolicy::LocalOnly
}

/// Endereço completo da API a partir do que o usuário digitou. Aceita só o servidor
/// (`http://host:11434`) e completa o caminho padrão do provedor; um endereço que já tem
/// caminho fica como está (proxies, endpoints compatíveis).
pub fn resolve_llm_url(kind: LlmKind, url: &str) -> String {
    let base = url.trim().trim_end_matches('/');
    let after_scheme = base.split_once("://").map_or(base, |(_, rest)| rest);
    let has_path = after_scheme.contains('/');
    match kind {
        LlmKind::Ollama if !has_path => format!("{base}/api/chat"),
        LlmKind::OpenAi if base.ends_with("/v1") => format!("{base}/chat/completions"),
        LlmKind::OpenAi if !has_path => format!("{base}/v1/chat/completions"),
        _ => base.to_string(),
    }
}

/// Provedor e config da limpeza, ou `None` (nível "nenhum" ou provedor desligado).
pub fn llm_choice(settings: &Settings) -> Option<(LlmKind, LlmConfig)> {
    if settings.cleanup == CleanupLevel::None {
        return None;
    }
    let kind = settings.llm_provider?;
    let model = settings
        .llm_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string);
    let url = settings
        .llm_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| resolve_llm_url(kind, u));
    Some((
        kind,
        LlmConfig {
            model,
            url,
            ..LlmConfig::default()
        },
    ))
}

/// Motor de STT conforme o Settings: VAD → política (local / nuvem / nuvem com reserva local).
pub fn build_stt(settings: &Settings, app: &AppHandle) -> Arc<dyn SttEngine> {
    let local = app
        .try_state::<LocalStt>()
        .map(|s| Arc::clone(&s.0) as Arc<dyn SttEngine>);
    let cloud = needs_cloud(settings.stt_policy).then(|| {
        Arc::new(CloudStt::new(
            settings.stt_cloud,
            Arc::new(AppKeys(app.clone())),
            CloudConfig::default(),
        )) as Arc<dyn SttEngine>
    });
    let policy: Arc<dyn SttEngine> = Arc::new(crate::stt::cloud::PolicyStt::new(
        settings.stt_policy,
        cloud,
        local,
    ));
    match app.try_state::<Arc<VadService>>() {
        Some(vad) => Arc::new(VadGated::new(Arc::clone(&vad), policy)),
        None => policy,
    }
}

/// Provedor de limpeza conforme o Settings; `None` = colar o texto sem LLM.
pub fn build_llm(settings: &Settings, app: &AppHandle) -> Option<Arc<dyn LlmProvider>> {
    let (kind, cfg) = llm_choice(settings)?;
    Some(Arc::new(LlmCleaner::new(
        kind,
        Arc::new(AppKeys(app.clone())),
        cfg,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_off_by_default_and_when_cleanup_none() {
        let s = Settings::default();
        assert!(llm_choice(&s).is_none(), "sem provedor escolhido");
        let on = Settings {
            llm_provider: Some(LlmKind::Anthropic),
            ..Settings::default()
        };
        assert_eq!(llm_choice(&on).unwrap().0, LlmKind::Anthropic);
        let none_level = Settings {
            cleanup: CleanupLevel::None,
            ..on.clone()
        };
        assert!(llm_choice(&none_level).is_none());
    }

    #[test]
    fn llm_model_override_ignores_blank() {
        let mut s = Settings {
            llm_provider: Some(LlmKind::Ollama),
            llm_model: Some("  ".into()),
            ..Settings::default()
        };
        assert_eq!(llm_choice(&s).unwrap().1.model, None);
        s.llm_model = Some("qwen3:8b".into());
        assert_eq!(llm_choice(&s).unwrap().1.model.as_deref(), Some("qwen3:8b"));
    }

    #[test]
    fn llm_url_is_forwarded_and_blank_means_default() {
        let mut s = Settings {
            llm_provider: Some(LlmKind::Ollama),
            llm_url: Some("http://10.0.0.5:11434".into()),
            ..Settings::default()
        };
        assert_eq!(
            llm_choice(&s).unwrap().1.url.as_deref(),
            Some("http://10.0.0.5:11434/api/chat"),
            "só o servidor: completa o caminho do Ollama"
        );
        s.llm_url = Some("  ".into());
        assert_eq!(llm_choice(&s).unwrap().1.url, None);
        s.llm_url = None;
        assert_eq!(llm_choice(&s).unwrap().1.url, None);
    }

    #[test]
    fn resolve_llm_url_completes_only_bare_servers() {
        use LlmKind::*;
        assert_eq!(
            resolve_llm_url(Ollama, "http://10.0.0.5:11434/"),
            "http://10.0.0.5:11434/api/chat"
        );
        assert_eq!(
            resolve_llm_url(Ollama, "http://h:1/api/chat"),
            "http://h:1/api/chat"
        );
        assert_eq!(
            resolve_llm_url(Ollama, "http://h:1/proxy/chat"),
            "http://h:1/proxy/chat"
        );
        assert_eq!(
            resolve_llm_url(OpenAi, "https://api.x.com"),
            "https://api.x.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_llm_url(OpenAi, "https://api.x.com/v1"),
            "https://api.x.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_llm_url(OpenAi, "https://api.x.com/v1/chat/completions"),
            "https://api.x.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_llm_url(Anthropic, "https://proxy.x.com/v1/messages"),
            "https://proxy.x.com/v1/messages"
        );
    }

    #[test]
    fn cloud_only_when_policy_asks() {
        assert!(!needs_cloud(SttPolicy::LocalOnly));
        assert!(needs_cloud(SttPolicy::CloudOnly));
        assert!(needs_cloud(SttPolicy::CloudWithFallback));
    }
}
