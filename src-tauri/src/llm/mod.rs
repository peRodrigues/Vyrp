//! Limpeza opcional por LLM atrás de LlmProvider: Claude, OpenAI, Ollama.
//!
//! Card V13 (spec `docs/re/wispr-flow-spec.md` §6). O texto cru + nível + estilo + categoria
//! do app + vocabulário viram um pedido; a resposta passa por uma guarda contra "o modelo
//! respondeu ao ditado". Qualquer falha (rede, prazo, chave, guarda) cai no texto sem LLM:
//! use [`clean_with_fallback`] — o ditado nunca fica sem texto por causa do LLM.
//!
//! - Prompts próprios, em inglês, em [`prompt`]; formatos das APIs em [`wire`].
//! - A chave vem do [`KeyProvider`] (Credential Manager) a cada chamada; não é guardada,
//!   logada nem repetida em erros.
//! - Nível `None` não chama o LLM.

pub mod error;
pub mod guard;
pub mod prompt;
pub mod transform;
pub mod wire;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::core::types::{CleanRequest, CleanupLevel, LlmProvider, Result};
use crate::stt::cloud::KeyProvider;

pub use error::LlmError;
pub use wire::LlmKind;

/// Prazo padrão da limpeza (spec §6, V13: 2,5 s): passou disso, vale mais o texto cru.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2_500);
/// Quanto do corpo de erro entra na mensagem.
const MAX_ERROR_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub timeout: Duration,
    /// Troca o modelo padrão do provedor.
    pub model: Option<String>,
    /// Troca o endereço da API (Ollama em outra porta/máquina, proxies, testes).
    pub url: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            model: None,
            url: None,
        }
    }
}

/// Provedor de limpeza para Anthropic, OpenAI ou Ollama.
pub struct LlmCleaner {
    kind: LlmKind,
    keys: Arc<dyn KeyProvider>,
    cfg: LlmConfig,
    client: reqwest::Client,
}

// Sem a chave (que nem fica guardada aqui) e sem o cliente HTTP.
impl std::fmt::Debug for LlmCleaner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmCleaner")
            .field("kind", &self.kind)
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

/// Troca a chave por `[chave]` e corta o texto: a API às vezes repete o que recebeu.
fn scrub(text: &str, key: &str) -> String {
    let cleaned = if key.is_empty() {
        text.to_string()
    } else {
        text.replace(key, "[chave]")
    };
    let mut out: String = cleaned.chars().take(MAX_ERROR_CHARS).collect();
    if cleaned.chars().count() > MAX_ERROR_CHARS {
        out.push('…');
    }
    out
}

/// Traduz uma resposta que não é 2xx em erro tipado.
pub fn classify_failure(
    kind: LlmKind,
    status: u16,
    retry_after_secs: Option<u64>,
    body: &str,
    key: &str,
) -> LlmError {
    let provider = kind.name();
    match status {
        401 | 403 => LlmError::Unauthorized(provider),
        408 | 504 => LlmError::Timeout(provider),
        429 => LlmError::RateLimited {
            provider,
            retry_after_secs,
        },
        // 529 = "overloaded" da Anthropic.
        500..=599 => LlmError::Server { provider, status },
        _ => LlmError::Rejected {
            provider,
            status,
            message: scrub(&wire::error_message(body), key),
        },
    }
}

impl LlmCleaner {
    pub fn new(kind: LlmKind, keys: Arc<dyn KeyProvider>, cfg: LlmConfig) -> Self {
        Self {
            kind,
            keys,
            cfg,
            client: reqwest::Client::new(),
        }
    }

    pub fn anthropic(keys: Arc<dyn KeyProvider>) -> Self {
        Self::new(LlmKind::Anthropic, keys, LlmConfig::default())
    }

    pub fn openai(keys: Arc<dyn KeyProvider>) -> Self {
        Self::new(LlmKind::OpenAi, keys, LlmConfig::default())
    }

    /// Ollama local; não precisa de chave.
    pub fn ollama() -> Self {
        struct NoKeys;
        impl KeyProvider for NoKeys {
            fn api_key(
                &self,
                _: crate::settings::secrets::Provider,
            ) -> std::result::Result<Option<String>, String> {
                Ok(None)
            }
        }
        Self::new(LlmKind::Ollama, Arc::new(NoKeys), LlmConfig::default())
    }

    pub fn kind(&self) -> LlmKind {
        self.kind
    }

    fn url(&self) -> &str {
        self.cfg
            .url
            .as_deref()
            .unwrap_or_else(|| self.kind.default_url())
    }

    fn model(&self) -> &str {
        self.cfg
            .model
            .as_deref()
            .unwrap_or_else(|| self.kind.default_model())
    }

    fn api_key(&self) -> std::result::Result<Option<String>, LlmError> {
        let Some(secret) = self.kind.secret() else {
            return Ok(None);
        };
        let key = self
            .keys
            .api_key(secret)
            .map_err(LlmError::KeyStore)?
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .ok_or(LlmError::MissingKey(self.kind.name()))?;
        Ok(Some(key))
    }

    fn network_error(&self, err: reqwest::Error, key: &str) -> LlmError {
        let provider = self.kind.name();
        if err.is_timeout() {
            return LlmError::Timeout(provider);
        }
        LlmError::Network {
            provider,
            detail: scrub(&err.without_url().to_string(), key),
        }
    }

    /// Limpa o texto ou diz por quê não deu. Nível `None`/texto vazio devolvem o cru sem rede.
    pub async fn try_clean(&self, req: &CleanRequest) -> std::result::Result<String, LlmError> {
        let raw = req.raw.trim();
        if req.level == CleanupLevel::None || raw.is_empty() {
            return Ok(raw.to_string());
        }
        let key = self.api_key()?;
        let provider = self.kind.name();

        let system = prompt::system_prompt(req);
        let user = prompt::user_message(req);
        let max_tokens = wire::max_tokens_for(raw.chars().count());
        let body = wire::request_body(self.kind, self.model(), &system, &user, max_tokens);
        let body = serde_json::to_vec(&body).map_err(|e| LlmError::BadResponse {
            provider,
            detail: e.to_string(),
        })?;

        let mut request = self
            .client
            .post(self.url())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(self.cfg.timeout)
            .body(body);
        match (self.kind, key.as_deref()) {
            (LlmKind::Anthropic, Some(key)) => {
                request = request
                    .header("x-api-key", key)
                    .header("anthropic-version", wire::ANTHROPIC_VERSION);
            }
            (LlmKind::OpenAi, Some(key)) => request = request.bearer_auth(key),
            _ => {}
        }

        let started = Instant::now();
        let key_text = key.as_deref().unwrap_or("");
        tracing::info!(
            provider,
            model = self.model(),
            chars = raw.chars().count(),
            "limpando texto"
        );
        let response = request
            .send()
            .await
            .map_err(|e| self.network_error(e, key_text))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let bytes = response
            .bytes()
            .await
            .map_err(|e| self.network_error(e, key_text))?;
        tracing::info!(
            provider,
            status,
            ms = started.elapsed().as_millis() as u64,
            "resposta da limpeza"
        );

        if !(200..300).contains(&status) {
            let text = String::from_utf8_lossy(&bytes);
            return Err(classify_failure(
                self.kind,
                status,
                retry_after,
                &text,
                key_text,
            ));
        }
        let text = wire::parse_response(self.kind, &bytes)?;
        guard::sanitize(raw, &text, req.level).map_err(|reject| LlmError::Guarded(reject.reason()))
    }
}

#[async_trait]
impl LlmProvider for LlmCleaner {
    async fn clean(&self, req: &CleanRequest) -> Result<String> {
        self.try_clean(req).await.map_err(Into::into)
    }
}

/// Resultado de [`clean_with_fallback`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanOutcome {
    pub text: String,
    /// `true` = o texto veio do LLM; `false` = é o cru (nível `None` ou fallback).
    pub used_llm: bool,
    /// Por que caiu no cru, em português (para o log e o histórico). `None` se não houve falha.
    pub fallback_reason: Option<String>,
}

/// Limpa com o provedor; em qualquer falha devolve o texto cru e diz o motivo.
pub async fn clean_with_fallback(provider: &dyn LlmProvider, req: &CleanRequest) -> CleanOutcome {
    let raw = req.raw.trim().to_string();
    if req.level == CleanupLevel::None || raw.is_empty() {
        return CleanOutcome {
            text: raw,
            used_llm: false,
            fallback_reason: None,
        };
    }
    match provider.clean(req).await {
        Ok(text) => CleanOutcome {
            text,
            used_llm: true,
            fallback_reason: None,
        },
        Err(err) => {
            tracing::warn!(error = %err, "limpeza por LLM falhou; usando o texto sem LLM");
            CleanOutcome {
                text: raw,
                used_llm: false,
                fallback_reason: Some(err.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{AppCategory, AppContext, VyrpError, WritingStyle};
    use crate::settings::secrets::Provider;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    const KEY: &str = "sk-ant-SEGREDO-123";

    struct FixedKey(Option<String>);
    impl KeyProvider for FixedKey {
        fn api_key(&self, _: Provider) -> std::result::Result<Option<String>, String> {
            Ok(self.0.clone())
        }
    }

    fn keys(key: Option<&str>) -> Arc<dyn KeyProvider> {
        Arc::new(FixedKey(key.map(Into::into)))
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    struct Seen {
        head: String,
        body: Vec<u8>,
    }

    /// Servidor HTTP falso: atende UMA requisição e a devolve pelo canal.
    fn mock_server(
        status: u16,
        extra_header: &'static str,
        reply: String,
        delay: Duration,
    ) -> (String, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/chat", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head_end, content_len) = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (pos + 4, len);
                }
            };
            while buf.len() < head_end + content_len {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let _ = tx.send(Seen {
                head: String::from_utf8_lossy(&buf[..head_end]).to_string(),
                body: buf[head_end..].to_vec(),
            });
            std::thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra_header}Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (url, rx)
    }

    fn cleaner(kind: LlmKind, key: Option<&str>, url: &str, timeout: Duration) -> LlmCleaner {
        LlmCleaner::new(
            kind,
            keys(key),
            LlmConfig {
                timeout,
                url: Some(url.into()),
                ..Default::default()
            },
        )
    }

    fn request(raw: &str) -> CleanRequest {
        CleanRequest {
            raw: raw.into(),
            level: CleanupLevel::Medium,
            style: WritingStyle::Formal,
            context: AppContext {
                category: AppCategory::Email,
                ..Default::default()
            },
            vocabulary: vec!["Vyrp".into()],
        }
    }

    fn anthropic_reply(text: &str) -> String {
        serde_json::json!({
            "id": "msg_01", "type": "message", "role": "assistant",
            "content": [{ "type": "text", "text": text }],
            "stop_reason": "end_turn"
        })
        .to_string()
    }

    fn openai_reply(text: &str) -> String {
        serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }]
        })
        .to_string()
    }

    fn ollama_reply(text: &str) -> String {
        serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done_reason": "stop", "done": true
        })
        .to_string()
    }

    const RAW: &str = "oi tudo bem queria marcar a reunião do vyrp pra amanhã";
    const CLEAN: &str = "Oi, tudo bem? Queria marcar a reunião do Vyrp para amanhã.";

    // ------------------------------------------------------------ provedores

    #[test]
    fn anthropic_sends_key_version_model_prompt_and_dictation() {
        let (url, rx) = mock_server(200, "", anthropic_reply(CLEAN), Duration::ZERO);
        let c = cleaner(LlmKind::Anthropic, Some(KEY), &url, Duration::from_secs(3));
        let out = block_on(c.clean(&request(RAW))).unwrap();
        assert_eq!(out, CLEAN);

        let seen = rx.recv().unwrap();
        let head = seen.head.to_lowercase();
        assert!(head.starts_with("post /v1/chat"));
        assert!(head.contains(&format!("x-api-key: {}", KEY.to_lowercase())));
        assert!(head.contains("anthropic-version: 2023-06-01"));
        assert!(head.contains("content-type: application/json"));
        let body: serde_json::Value = serde_json::from_slice(&seen.body).unwrap();
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["temperature"], 0);
        let system = body["system"].as_str().unwrap();
        assert!(system.contains("do NOT answer it"));
        assert!(system.contains("level: MEDIUM") && system.contains("Style: FORMAL"));
        assert!(system.contains("an email"));
        assert!(system.contains("exactly as listed: Vyrp"));
        assert_eq!(
            body["messages"][0]["content"],
            format!("<dictation>\n{RAW}\n</dictation>")
        );
        assert!(!String::from_utf8_lossy(&seen.body).contains(KEY));
    }

    #[test]
    fn openai_uses_bearer_and_chat_messages() {
        let (url, rx) = mock_server(200, "", openai_reply(CLEAN), Duration::ZERO);
        let c = cleaner(
            LlmKind::OpenAi,
            Some("sk-abc"),
            &url,
            Duration::from_secs(3),
        );
        assert_eq!(block_on(c.clean(&request(RAW))).unwrap(), CLEAN);
        let seen = rx.recv().unwrap();
        assert!(
            seen.head
                .to_lowercase()
                .contains("authorization: bearer sk-abc")
        );
        let body: serde_json::Value = serde_json::from_slice(&seen.body).unwrap();
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn ollama_needs_no_key_and_honors_a_custom_model() {
        let (url, rx) = mock_server(200, "", ollama_reply(CLEAN), Duration::ZERO);
        let mut c = LlmCleaner::ollama();
        c.cfg.url = Some(url);
        c.cfg.model = Some("qwen2.5:3b".into());
        assert_eq!(block_on(c.clean(&request(RAW))).unwrap(), CLEAN);
        let seen = rx.recv().unwrap();
        let head = seen.head.to_lowercase();
        assert!(!head.contains("authorization") && !head.contains("x-api-key"));
        let body: serde_json::Value = serde_json::from_slice(&seen.body).unwrap();
        assert_eq!(body["model"], "qwen2.5:3b");
        assert_eq!(body["stream"], false);
    }

    // ------------------------------------------------------------ falhas tipadas

    #[test]
    fn unauthorized_never_echoes_the_key() {
        let (url, _rx) = mock_server(
            401,
            "",
            format!(
                r#"{{"type":"error","error":{{"type":"authentication_error","message":"invalid x-api-key {KEY}"}}}}"#
            ),
            Duration::ZERO,
        );
        let c = cleaner(LlmKind::Anthropic, Some(KEY), &url, Duration::from_secs(3));
        let err = block_on(c.try_clean(&request(RAW))).unwrap_err();
        assert_eq!(err, LlmError::Unauthorized("Anthropic"));
        assert!(!err.to_string().contains(KEY));
        assert!(!format!("{c:?}").contains(KEY));
    }

    #[test]
    fn rejected_message_is_scrubbed() {
        let body = format!(r#"{{"error":{{"message":"bad request for key {KEY}"}}}}"#);
        let err = classify_failure(LlmKind::OpenAi, 400, None, &body, KEY);
        let text = err.to_string();
        assert!(text.contains("bad request") && text.contains("[chave]") && !text.contains(KEY));
    }

    #[test]
    fn rate_limit_server_error_and_overload_are_typed() {
        let (url, _rx) = mock_server(429, "Retry-After: 9\r\n", "{}".into(), Duration::ZERO);
        let c = cleaner(LlmKind::OpenAi, Some("k"), &url, Duration::from_secs(3));
        assert_eq!(
            block_on(c.try_clean(&request(RAW))).unwrap_err(),
            LlmError::RateLimited {
                provider: "OpenAI",
                retry_after_secs: Some(9)
            }
        );
        assert_eq!(
            classify_failure(LlmKind::Anthropic, 529, None, "{}", "k"),
            LlmError::Server {
                provider: "Anthropic",
                status: 529
            }
        );
        assert_eq!(
            classify_failure(
                LlmKind::Ollama,
                404,
                None,
                r#"{"error":"model 'x' not found"}"#,
                ""
            ),
            LlmError::Rejected {
                provider: "Ollama",
                status: 404,
                message: "model 'x' not found".into()
            }
        );
    }

    #[test]
    fn slow_server_times_out_within_the_deadline() {
        let (url, _rx) = mock_server(
            200,
            "",
            anthropic_reply(CLEAN),
            Duration::from_millis(1_500),
        );
        let c = cleaner(
            LlmKind::Anthropic,
            Some("k"),
            &url,
            Duration::from_millis(250),
        );
        let started = Instant::now();
        let err = block_on(c.try_clean(&request(RAW))).unwrap_err();
        assert_eq!(err, LlmError::Timeout("Anthropic"));
        assert!(started.elapsed() < Duration::from_millis(1_400));
        assert!(
            matches!(VyrpError::from(err), VyrpError::Provider { error, .. } if error.code == "provider.timeout")
        );
    }

    #[test]
    fn missing_key_and_dead_server() {
        let c = cleaner(
            LlmKind::Anthropic,
            None,
            "http://127.0.0.1:1/x",
            Duration::from_secs(3),
        );
        assert_eq!(
            block_on(c.try_clean(&request(RAW))).unwrap_err(),
            LlmError::MissingKey("Anthropic")
        );
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let c = cleaner(
            LlmKind::Ollama,
            None,
            &format!("http://127.0.0.1:{port}/x"),
            Duration::from_secs(3),
        );
        assert!(matches!(
            block_on(c.try_clean(&request(RAW))),
            Err(LlmError::Network { .. })
        ));
    }

    #[test]
    fn level_none_and_empty_text_skip_the_network() {
        // URL inválida: se tentasse enviar, o erro seria de rede/chave.
        let c = cleaner(
            LlmKind::Anthropic,
            None,
            "http://127.0.0.1:1/x",
            Duration::from_secs(3),
        );
        let mut none = request("  texto  ");
        none.level = CleanupLevel::None;
        assert_eq!(block_on(c.try_clean(&none)).unwrap(), "texto");
        assert_eq!(block_on(c.try_clean(&request("   "))).unwrap(), "");
    }

    // ------------------------------------------------------------ guarda e fallback

    #[test]
    fn conversational_reply_is_guarded() {
        let (url, _rx) = mock_server(
            200,
            "",
            anthropic_reply("I'm sorry, but I can't help with scheduling meetings."),
            Duration::ZERO,
        );
        let c = cleaner(LlmKind::Anthropic, Some("k"), &url, Duration::from_secs(3));
        let err = block_on(c.try_clean(&request(RAW))).unwrap_err();
        assert!(matches!(err, LlmError::Guarded(_)));
    }

    #[test]
    fn fallback_returns_the_raw_text_and_the_reason() {
        let (url, _rx) = mock_server(500, "", "{}".into(), Duration::ZERO);
        let c = cleaner(LlmKind::OpenAi, Some("k"), &url, Duration::from_secs(3));
        let out = block_on(clean_with_fallback(&c, &request("  oi tudo bem  ")));
        assert_eq!(out.text, "oi tudo bem");
        assert!(!out.used_llm);
        assert!(out.fallback_reason.unwrap().contains("indisponível"));
    }

    #[test]
    fn fallback_is_silent_on_success_and_when_the_level_is_none() {
        let (url, _rx) = mock_server(200, "", openai_reply(CLEAN), Duration::ZERO);
        let c = cleaner(LlmKind::OpenAi, Some("k"), &url, Duration::from_secs(3));
        let ok = block_on(clean_with_fallback(&c, &request(RAW)));
        assert_eq!(
            (ok.text.as_str(), ok.used_llm, ok.fallback_reason),
            (CLEAN, true, None)
        );

        let mut none = request(RAW);
        none.level = CleanupLevel::None;
        let skipped = block_on(clean_with_fallback(&c, &none));
        assert_eq!((skipped.text.as_str(), skipped.used_llm), (RAW, false));
        assert_eq!(skipped.fallback_reason, None);
    }

    #[test]
    fn guarded_answer_to_a_question_falls_back_to_the_dictation() {
        // O usuário ditou uma pergunta; o modelo (errado) respondeu. O texto final é o ditado.
        let raw = "qual é a capital da frança";
        let (url, _rx) = mock_server(200, "", ollama_reply("Paris."), Duration::ZERO);
        let mut c = LlmCleaner::ollama();
        c.cfg.url = Some(url);
        let out = block_on(clean_with_fallback(&c, &request(raw)));
        assert_eq!(out.text, raw);
        assert!(!out.used_llm);
        assert!(out.fallback_reason.is_some());
    }

    #[test]
    fn trait_errors_use_vyrp_llm_variant() {
        let c = cleaner(
            LlmKind::OpenAi,
            None,
            "http://127.0.0.1:1/x",
            Duration::from_secs(3),
        );
        let err = block_on(c.clean(&request(RAW))).unwrap_err();
        assert!(
            matches!(&err, VyrpError::Provider { error, .. } if error.code == "provider.missing_key"),
            "{err}"
        );
        assert!(err.to_string().contains("chave de API"));
    }
}
