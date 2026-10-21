//! Chaves de API no Windows Credential Manager (crate `keyring`).
//!
//! O valor só entra (`set_api_key`) e é lido por código Rust ([`Secrets::get`]);
//! nenhum command devolve a chave à UI, só `has_api_key`.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::core::ui_error::UiError;

const SERVICE: &str = "vyrp";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Groq,
    OpenAi,
    Anthropic,
}

impl Provider {
    fn account(self) -> &'static str {
        match self {
            Provider::Groq => "groq",
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
        }
    }
}

/// Onde as chaves moram. O app usa o Credential Manager; os testes, memória.
pub trait SecretBackend: Send + Sync {
    fn set(&self, provider: Provider, value: &str) -> Result<(), String>;
    fn get(&self, provider: Provider) -> Result<Option<String>, String>;
    /// Remover o que não existe não é erro.
    fn clear(&self, provider: Provider) -> Result<(), String>;
}

struct KeyringBackend;

impl KeyringBackend {
    fn entry(provider: Provider) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, provider.account()).map_err(|e| e.to_string())
    }
}

impl SecretBackend for KeyringBackend {
    fn set(&self, provider: Provider, value: &str) -> Result<(), String> {
        Self::entry(provider)?
            .set_password(value)
            .map_err(|e| e.to_string())
    }

    fn get(&self, provider: Provider) -> Result<Option<String>, String> {
        match Self::entry(provider)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn clear(&self, provider: Provider) -> Result<(), String> {
        match Self::entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub struct Secrets {
    backend: Box<dyn SecretBackend>,
}

impl Secrets {
    pub fn system() -> Self {
        Self::with_backend(Box::new(KeyringBackend))
    }

    pub fn with_backend(backend: Box<dyn SecretBackend>) -> Self {
        Self { backend }
    }

    pub fn set(&self, provider: Provider, value: &str) -> Result<(), UiError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(UiError::new("key.empty", "a chave está vazia"));
        }
        Ok(self.backend.set(provider, value)?)
    }

    pub fn clear(&self, provider: Provider) -> Result<(), UiError> {
        Ok(self.backend.clear(provider)?)
    }

    pub fn has(&self, provider: Provider) -> Result<bool, UiError> {
        Ok(self.backend.get(provider)?.is_some())
    }

    /// Só para os módulos Rust que chamam o provedor (STT/LLM). Não é command.
    pub fn get(&self, provider: Provider) -> Result<Option<String>, String> {
        self.backend.get(provider)
    }
}

#[tauri::command]
pub fn set_api_key(
    secrets: State<'_, Secrets>,
    provider: Provider,
    value: String,
) -> Result<(), UiError> {
    secrets.set(provider, &value)
}

#[tauri::command]
pub fn clear_api_key(secrets: State<'_, Secrets>, provider: Provider) -> Result<(), UiError> {
    secrets.clear(provider)
}

#[tauri::command]
pub fn has_api_key(secrets: State<'_, Secrets>, provider: Provider) -> Result<bool, UiError> {
    secrets.has(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memory(Mutex<HashMap<&'static str, String>>);

    impl SecretBackend for Memory {
        fn set(&self, p: Provider, v: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(p.account(), v.into());
            Ok(())
        }
        fn get(&self, p: Provider) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(p.account()).cloned())
        }
        fn clear(&self, p: Provider) -> Result<(), String> {
            self.0.lock().unwrap().remove(p.account());
            Ok(())
        }
    }

    fn secrets() -> Secrets {
        Secrets::with_backend(Box::new(Memory::default()))
    }

    #[test]
    fn set_has_clear_cycle() {
        let s = secrets();
        assert!(!s.has(Provider::Groq).unwrap());
        s.set(Provider::Groq, "  gsk_abc ").unwrap();
        assert!(s.has(Provider::Groq).unwrap());
        assert_eq!(s.get(Provider::Groq).unwrap().as_deref(), Some("gsk_abc"));
        s.clear(Provider::Groq).unwrap();
        assert!(!s.has(Provider::Groq).unwrap());
    }

    #[test]
    fn providers_are_independent() {
        let s = secrets();
        s.set(Provider::OpenAi, "k").unwrap();
        assert!(!s.has(Provider::Anthropic).unwrap());
    }

    #[test]
    fn empty_key_is_refused_and_clear_missing_is_ok() {
        let s = secrets();
        assert_eq!(s.set(Provider::Groq, "   ").unwrap_err().code, "key.empty");
        s.clear(Provider::Anthropic).unwrap();
    }

    #[test]
    fn provider_wire_names() {
        assert_eq!(
            serde_json::to_string(&Provider::OpenAi).unwrap(),
            "\"open_ai\""
        );
    }
}
