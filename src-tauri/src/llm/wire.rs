//! Formato de cada API: monta o corpo do pedido e extrai o texto da resposta.
//!
//! Sem rede aqui; é tudo função pura, testada com respostas gravadas (no formato real de
//! cada provedor).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::error::LlmError;
use crate::settings::secrets::Provider;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmKind {
    Anthropic,
    OpenAi,
    Ollama,
}

impl LlmKind {
    pub fn name(self) -> &'static str {
        match self {
            LlmKind::Anthropic => "Anthropic",
            LlmKind::OpenAi => "OpenAI",
            LlmKind::Ollama => "Ollama",
        }
    }

    /// Chave no Credential Manager; o Ollama local não usa.
    pub fn secret(self) -> Option<Provider> {
        match self {
            LlmKind::Anthropic => Some(Provider::Anthropic),
            LlmKind::OpenAi => Some(Provider::OpenAi),
            LlmKind::Ollama => None,
        }
    }

    pub fn default_url(self) -> &'static str {
        match self {
            LlmKind::Anthropic => "https://api.anthropic.com/v1/messages",
            LlmKind::OpenAi => "https://api.openai.com/v1/chat/completions",
            LlmKind::Ollama => "http://localhost:11434/api/chat",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            LlmKind::Anthropic => "claude-haiku-5-5",
            LlmKind::OpenAi => "gpt-4o-mini",
            LlmKind::Ollama => "llama3.2",
        }
    }
}

/// Teto de tokens da resposta: a limpeza devolve algo do tamanho do ditado.
pub fn max_tokens_for(raw_chars: usize) -> u32 {
    ((raw_chars / 2 + 64) as u32).clamp(64, 2_048)
}

/// Corpo JSON do pedido. Temperatura 0: queremos reescrita estável, não criativa.
pub fn request_body(
    kind: LlmKind,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Value {
    match kind {
        LlmKind::Anthropic => json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": 0,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        }),
        LlmKind::OpenAi => json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": 0,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        }),
        LlmKind::Ollama => json!({
            "model": model,
            "stream": false,
            "options": { "temperature": 0, "num_predict": max_tokens },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        }),
    }
}

/// Extrai o texto da resposta. Resposta cortada por limite de tokens vira erro: um texto
/// pela metade é pior que o ditado cru.
pub fn parse_response(kind: LlmKind, body: &[u8]) -> Result<String, LlmError> {
    let provider = kind.name();
    let bad = |detail: &str| LlmError::BadResponse {
        provider,
        detail: detail.to_string(),
    };
    let value: Value = serde_json::from_slice(body).map_err(|_| bad("não é JSON"))?;
    let (text, truncated) = match kind {
        LlmKind::Anthropic => {
            let blocks = value
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("sem o campo \"content\""))?;
            let text: String = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            let cut = value.get("stop_reason").and_then(Value::as_str) == Some("max_tokens");
            (text, cut)
        }
        LlmKind::OpenAi => {
            let choice = value
                .pointer("/choices/0")
                .ok_or_else(|| bad("sem \"choices\""))?;
            let text = choice
                .pointer("/message/content")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("sem o texto da resposta"))?
                .to_string();
            let cut = choice.get("finish_reason").and_then(Value::as_str) == Some("length");
            (text, cut)
        }
        LlmKind::Ollama => {
            let text = value
                .pointer("/message/content")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("sem o texto da resposta"))?
                .to_string();
            let cut = value.get("done_reason").and_then(Value::as_str) == Some("length");
            (text, cut)
        }
    };
    if truncated {
        return Err(bad("resposta cortada pelo limite de tokens"));
    }
    Ok(text)
}

/// Mensagem de erro do corpo (`{"error":{"message":..}}`, `{"error":".."}` ou texto cru).
pub fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error"))
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Respostas gravadas (formato real de cada API; texto de exemplo).
    const ANTHROPIC_OK: &str = r#"{"id":"msg_01X","type":"message","role":"assistant","model":"claude-haiku-5-5",
        "content":[{"type":"text","text":"Oi, tudo bem?"}],"stop_reason":"end_turn","stop_sequence":null,
        "usage":{"input_tokens":310,"output_tokens":8}}"#;
    const ANTHROPIC_CUT: &str =
        r#"{"content":[{"type":"text","text":"Oi, tu"}],"stop_reason":"max_tokens"}"#;
    const OPENAI_OK: &str = r#"{"id":"chatcmpl-9","object":"chat.completion","model":"gpt-4o-mini",
        "choices":[{"index":0,"message":{"role":"assistant","content":"Oi, tudo bem?"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":300,"completion_tokens":7,"total_tokens":307}}"#;
    const OPENAI_CUT: &str =
        r#"{"choices":[{"message":{"content":"Oi, tu"},"finish_reason":"length"}]}"#;
    const OLLAMA_OK: &str = r#"{"model":"llama3.2","created_at":"2026-10-20T20:00:00Z",
        "message":{"role":"assistant","content":"Oi, tudo bem?"},"done_reason":"stop","done":true}"#;
    const OLLAMA_CUT: &str =
        r#"{"message":{"content":"Oi, tu"},"done_reason":"length","done":true}"#;

    #[test]
    fn parses_recorded_responses_for_each_provider() {
        for (kind, body) in [
            (LlmKind::Anthropic, ANTHROPIC_OK),
            (LlmKind::OpenAi, OPENAI_OK),
            (LlmKind::Ollama, OLLAMA_OK),
        ] {
            assert_eq!(
                parse_response(kind, body.as_bytes()).unwrap(),
                "Oi, tudo bem?"
            );
        }
    }

    #[test]
    fn anthropic_joins_text_blocks_and_ignores_the_rest() {
        let body = r#"{"content":[{"type":"thinking","thinking":"..."},
            {"type":"text","text":"Primeira. "},{"type":"text","text":"Segunda."}],"stop_reason":"end_turn"}"#;
        assert_eq!(
            parse_response(LlmKind::Anthropic, body.as_bytes()).unwrap(),
            "Primeira. Segunda."
        );
    }

    #[test]
    fn truncated_responses_are_errors() {
        for (kind, body) in [
            (LlmKind::Anthropic, ANTHROPIC_CUT),
            (LlmKind::OpenAi, OPENAI_CUT),
            (LlmKind::Ollama, OLLAMA_CUT),
        ] {
            assert!(matches!(
                parse_response(kind, body.as_bytes()),
                Err(LlmError::BadResponse { .. })
            ));
        }
    }

    #[test]
    fn malformed_bodies_are_typed_errors() {
        for kind in [LlmKind::Anthropic, LlmKind::OpenAi, LlmKind::Ollama] {
            for body in ["<html>", "{}", r#"{"choices":[]}"#] {
                assert!(
                    matches!(
                        parse_response(kind, body.as_bytes()),
                        Err(LlmError::BadResponse { .. })
                    ),
                    "{kind:?} {body}"
                );
            }
        }
    }

    #[test]
    fn request_bodies_follow_each_api_shape() {
        let a = request_body(LlmKind::Anthropic, "claude-haiku-5-5", "SYS", "USR", 100);
        assert_eq!(a["system"], "SYS");
        assert_eq!(a["messages"][0]["role"], "user");
        assert_eq!(a["messages"][0]["content"], "USR");
        assert_eq!(a["max_tokens"], 100);
        assert_eq!(a["temperature"], 0);

        let o = request_body(LlmKind::OpenAi, "gpt-4o-mini", "SYS", "USR", 100);
        assert_eq!(o["messages"][0]["role"], "system");
        assert_eq!(o["messages"][0]["content"], "SYS");
        assert_eq!(o["messages"][1]["content"], "USR");

        let l = request_body(LlmKind::Ollama, "llama3.2", "SYS", "USR", 100);
        assert_eq!(l["stream"], false);
        assert_eq!(l["options"]["num_predict"], 100);
        assert_eq!(l["messages"][1]["role"], "user");
    }

    #[test]
    fn defaults_and_secrets() {
        assert_eq!(LlmKind::Anthropic.default_model(), "claude-haiku-5-5");
        assert_eq!(LlmKind::Anthropic.secret(), Some(Provider::Anthropic));
        assert_eq!(LlmKind::OpenAi.secret(), Some(Provider::OpenAi));
        assert_eq!(LlmKind::Ollama.secret(), None);
        assert!(
            LlmKind::Ollama
                .default_url()
                .starts_with("http://localhost")
        );
    }

    #[test]
    fn max_tokens_scales_with_the_dictation() {
        assert_eq!(max_tokens_for(0), 64);
        assert_eq!(max_tokens_for(400), 264);
        assert_eq!(max_tokens_for(1_000_000), 2_048);
    }

    #[test]
    fn error_message_handles_the_three_shapes() {
        assert_eq!(
            error_message(
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#
            ),
            "bad model"
        );
        assert_eq!(
            error_message(r#"{"error":"model 'x' not found"}"#),
            "model 'x' not found"
        );
        assert_eq!(error_message("  plain text "), "plain text");
    }
}
