//! Erros tipados da limpeza por LLM.

use crate::core::types::VyrpError;
use crate::core::ui_error::UiError;

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

impl LlmError {
    /// Código estável + parâmetros para a tela (`uierr.<code>`); o `Display` segue em português.
    pub fn ui(&self) -> UiError {
        let err = UiError::new(self.code(), self.to_string());
        match self {
            LlmError::MissingKey(p) | LlmError::Unauthorized(p) | LlmError::Timeout(p) => {
                err.with("provider", p)
            }
            LlmError::KeyStore(detail) => err.with("detail", detail),
            LlmError::RateLimited {
                provider,
                retry_after_secs,
            } => {
                let err = err.with("provider", provider);
                match retry_after_secs {
                    Some(secs) => err.with("secs", secs),
                    None => err,
                }
            }
            LlmError::Network { provider, detail } | LlmError::BadResponse { provider, detail } => {
                err.with("provider", provider).with("detail", detail)
            }
            LlmError::Server { provider, status } => {
                err.with("provider", provider).with("status", status)
            }
            LlmError::Rejected {
                provider,
                status,
                message,
            } => err
                .with("provider", provider)
                .with("status", status)
                .with("message", message),
            LlmError::Guarded(_) => err,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            LlmError::MissingKey(_) => "provider.missing_key",
            LlmError::KeyStore(_) => "provider.key_store",
            LlmError::Unauthorized(_) => "provider.unauthorized",
            LlmError::RateLimited {
                retry_after_secs: Some(_),
                ..
            } => "provider.rate_limited_retry",
            LlmError::RateLimited { .. } => "provider.rate_limited",
            LlmError::Timeout(_) => "provider.timeout",
            LlmError::Network { .. } => "provider.network",
            LlmError::Server { .. } => "provider.server",
            LlmError::Rejected { .. } => "provider.rejected",
            LlmError::BadResponse { .. } => "provider.bad_response",
            LlmError::Guarded(reason) => crate::llm::guard::reason_code(reason),
        }
    }
}

impl From<LlmError> for VyrpError {
    fn from(err: LlmError) -> Self {
        VyrpError::Provider {
            kind: "LLM",
            error: err.ui(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(err: LlmError) -> (String, Vec<(String, String)>) {
        let ui = err.ui();
        (ui.code, ui.params.into_iter().collect())
    }

    #[test]
    fn every_variant_has_a_stable_code_and_its_parameters() {
        let cases = [
            (LlmError::MissingKey("Groq"), "provider.missing_key"),
            (LlmError::KeyStore("x".into()), "provider.key_store"),
            (LlmError::Unauthorized("OpenAI"), "provider.unauthorized"),
            (LlmError::Timeout("Ollama"), "provider.timeout"),
            (
                LlmError::RateLimited {
                    provider: "Anthropic",
                    retry_after_secs: None,
                },
                "provider.rate_limited",
            ),
            (
                LlmError::Network {
                    provider: "Ollama",
                    detail: "recusada".into(),
                },
                "provider.network",
            ),
            (
                LlmError::Server {
                    provider: "OpenAI",
                    status: 503,
                },
                "provider.server",
            ),
            (
                LlmError::Rejected {
                    provider: "OpenAI",
                    status: 400,
                    message: "ruim".into(),
                },
                "provider.rejected",
            ),
            (
                LlmError::BadResponse {
                    provider: "OpenAI",
                    detail: "sem texto".into(),
                },
                "provider.bad_response",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(code(err).0, expected);
        }
    }

    #[test]
    fn rate_limit_with_a_wait_time_is_its_own_code() {
        let err = LlmError::RateLimited {
            provider: "Anthropic",
            retry_after_secs: Some(30),
        };
        let ui = err.ui();
        assert_eq!(ui.code, "provider.rate_limited_retry");
        assert_eq!(ui.params["secs"], "30");
        assert_eq!(ui.params["provider"], "Anthropic");
        // O texto de log continua em português.
        assert!(err.to_string().contains("tente de novo em 30 s"));
    }

    #[test]
    fn guard_reasons_map_to_codes() {
        for (reject, code) in [
            (crate::llm::guard::GuardReject::Empty, "llm.guard_empty"),
            (
                crate::llm::guard::GuardReject::Conversational,
                "llm.guard_conversational",
            ),
            (
                crate::llm::guard::GuardReject::TooLong,
                "llm.guard_too_long",
            ),
            (
                crate::llm::guard::GuardReject::TooShort,
                "llm.guard_too_short",
            ),
        ] {
            assert_eq!(LlmError::Guarded(reject.reason()).ui().code, code);
        }
    }

    #[test]
    fn the_vyrp_error_keeps_the_code_and_the_portuguese_log_text() {
        let err: VyrpError = LlmError::Unauthorized("OpenAI").into();
        assert_eq!(err.ui().code, "provider.unauthorized");
        assert_eq!(err.to_string(), "LLM: chave de API recusada pelo OpenAI");
    }
}
