//! Erros tipados da limpeza por LLM.

use crate::core::types::VyrpError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("falta a chave de API do {0}")]
    MissingKey(&'static str),
    #[error("não foi possível ler a chave de API: {0}")]
    KeyStore(String),
    #[error("chave de API recusada pelo {0}")]
    Unauthorized(&'static str),
    #[error("limite de uso do {provider} atingido{}", retry_hint(*.retry_after_secs))]
    RateLimited {
        provider: &'static str,
        retry_after_secs: Option<u64>,
    },
    #[error("tempo esgotado falando com o {0}")]
    Timeout(&'static str),
    #[error("sem conexão com o {provider}: {detail}")]
    Network {
        provider: &'static str,
        detail: String,
    },
    #[error("{provider} indisponível (HTTP {status})")]
    Server { provider: &'static str, status: u16 },
    #[error("{provider} recusou o pedido (HTTP {status}): {message}")]
    Rejected {
        provider: &'static str,
        status: u16,
        message: String,
    },
    #[error("resposta inesperada do {provider}: {detail}")]
    BadResponse {
        provider: &'static str,
        detail: String,
    },
    /// O modelo respondeu ao ditado em vez de reescrevê-lo (ou devolveu algo implausível).
    #[error("resposta descartada: {0}")]
    Guarded(&'static str),
}

fn retry_hint(secs: Option<u64>) -> String {
    secs.map(|s| format!(" (tente de novo em {s} s)"))
        .unwrap_or_default()
}

impl From<LlmError> for VyrpError {
    fn from(err: LlmError) -> Self {
        match err {
            LlmError::Timeout(_) => VyrpError::Timeout(err.to_string()),
            other => VyrpError::Llm(other.to_string()),
        }
    }
}
