//! Erro que chega à tela: um código estável + parâmetros, que a interface traduz por chave
//! (`uierr.<code>`). O `Display` continua em português, para o log e para quem lê o terminal.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Código usado quando só existe o texto (erro de SO, de biblioteca): a UI mostra `params.text`.
pub const RAW: &str = "raw";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiError {
    pub code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Mensagem em português para o log. Não vai para a tela.
    #[serde(skip)]
    text: String,
}

impl UiError {
    pub fn new(code: &str, text: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            params: BTreeMap::new(),
            text: text.into(),
        }
    }

    /// Só texto (sem tradução conhecida).
    pub fn raw(text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(RAW, text.clone()).with("text", text)
    }

    pub fn with(mut self, key: &str, value: impl ToString) -> Self {
        self.params.insert(key.to_string(), value.to_string());
        self
    }

    pub fn is_raw(&self) -> bool {
        self.code == RAW
    }
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for UiError {}

impl From<String> for UiError {
    fn from(text: String) -> Self {
        Self::raw(text)
    }
}

impl From<&str> for UiError {
    fn from(text: &str) -> Self {
        Self::raw(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_code_and_params_only() {
        let err = UiError::new("provider.timeout", "tempo esgotado falando com o Groq")
            .with("provider", "Groq");
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({"code": "provider.timeout", "params": {"provider": "Groq"}})
        );
        assert_eq!(err.to_string(), "tempo esgotado falando com o Groq");
    }

    #[test]
    fn params_are_omitted_when_empty() {
        let err = UiError::new("stt.empty_audio", "áudio vazio");
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({"code": "stt.empty_audio"})
        );
    }

    #[test]
    fn plain_strings_become_raw_errors() {
        let err: UiError = "falhou".to_string().into();
        assert!(err.is_raw());
        assert_eq!(err.params["text"], "falhou");
        assert_eq!(err.to_string(), "falhou");
    }
}
