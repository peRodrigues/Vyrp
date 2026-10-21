//! STT na nuvem (V10): `GroqEngine` e `OpenAiEngine` falam a API compatível com a da
//! OpenAI (`/audio/transcriptions`), mais a política local / nuvem / nuvem com fallback.
//!
//! - A chave vem do Credential Manager a cada chamada ([`KeyProvider`]) e nunca é
//!   guardada no motor, logada nem repetida em mensagens de erro.
//! - O corpo `multipart/form-data` é montado à mão (só precisa do `reqwest` básico).
//! - Erros são tipados ([`CloudSttError`]); para o resto do app viram `VyrpError`.
//!
//! Este arquivo é autônomo: para ligar, a Forja declara `pub mod cloud;` em `stt/mod.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::audio::wav;
use crate::core::types::{
    AudioClip, Result, SttEngine, SttOptions, SttTimings, Transcript, TranscriptSegment, VyrpError,
};
use crate::settings::secrets::{Provider, Secrets};

/// Limite de upload das duas APIs (25 MB); deixamos uma folga para o cabeçalho multipart.
pub const MAX_UPLOAD_BYTES: usize = 24 * 1024 * 1024;
/// Abaixo disso a API recusa o áudio; nem enviamos.
const MIN_AUDIO_MS: u64 = 100;
/// Prazo base da chamada (spec §6, V10: 3 s por padrão).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
/// O prompt do whisper aceita ~224 tokens; 800 caracteres cabem com folga.
const MAX_PROMPT_CHARS: usize = 800;
/// Quanto do corpo de erro entra na mensagem.
const MAX_ERROR_CHARS: usize = 200;

// ---------------------------------------------------------------- erros

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudSttError {
    #[error("falta a chave de API do {0}")]
    MissingKey(&'static str),
    #[error("não foi possível ler a chave de API: {0}")]
    KeyStore(String),
    #[error("áudio vazio ou curto demais para transcrever")]
    EmptyAudio,
    #[error("áudio grande demais para enviar ({size} bytes; limite {limit})")]
    TooLarge { size: usize, limit: usize },
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
}

fn retry_hint(secs: Option<u64>) -> String {
    secs.map(|s| format!(" (tente de novo em {s} s)"))
        .unwrap_or_default()
}

impl CloudSttError {
    /// Vale cair para o motor local? (Tudo, menos o que o local também sofreria.)
    pub fn allows_fallback(&self) -> bool {
        !matches!(
            self,
            CloudSttError::EmptyAudio | CloudSttError::TooLarge { .. }
        )
    }
}

impl From<CloudSttError> for VyrpError {
    fn from(err: CloudSttError) -> Self {
        match err {
            CloudSttError::Timeout(_) => VyrpError::Timeout(err.to_string()),
            other => VyrpError::Stt(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------- provedor e chave

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    Groq,
    OpenAi,
}

impl CloudProvider {
    pub fn name(self) -> &'static str {
        match self {
            CloudProvider::Groq => "Groq",
            CloudProvider::OpenAi => "OpenAI",
        }
    }

    pub fn secret(self) -> Provider {
        match self {
            CloudProvider::Groq => Provider::Groq,
            CloudProvider::OpenAi => Provider::OpenAi,
        }
    }

    pub fn default_url(self) -> &'static str {
        match self {
            CloudProvider::Groq => "https://api.groq.com/openai/v1/audio/transcriptions",
            CloudProvider::OpenAi => "https://api.openai.com/v1/audio/transcriptions",
        }
    }

    /// Modelos que devolvem `verbose_json` (com segmentos e idioma detectado).
    pub fn default_model(self) -> &'static str {
        match self {
            CloudProvider::Groq => "whisper-large-v3-turbo",
            CloudProvider::OpenAi => "whisper-1",
        }
    }
}

/// De onde vem a chave. Em produção é o Credential Manager ([`Secrets`]).
pub trait KeyProvider: Send + Sync {
    fn api_key(&self, provider: Provider) -> std::result::Result<Option<String>, String>;
}

impl KeyProvider for Secrets {
    fn api_key(&self, provider: Provider) -> std::result::Result<Option<String>, String> {
        self.get(provider)
    }
}

/// Lê as chaves do `Secrets` registrado no Tauri, sem guardar nenhuma.
pub struct AppKeys(pub tauri::AppHandle);

impl KeyProvider for AppKeys {
    fn api_key(&self, provider: Provider) -> std::result::Result<Option<String>, String> {
        use tauri::Manager;
        self.0.state::<Secrets>().get(provider)
    }
}

// ---------------------------------------------------------------- configuração

#[derive(Debug, Clone, PartialEq)]
pub struct CloudConfig {
    /// Prazo base da chamada; clipes longos ganham tempo extra ([`CloudConfig::timeout_for`]).
    pub timeout: Duration,
    pub model: Option<String>,
    /// Troca o endereço da API (testes, proxies).
    pub url: Option<String>,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            model: None,
            url: None,
        }
    }
}

impl CloudConfig {
    /// Prazo total: o base + 1 s para cada 5 s de áudio (upload e decodificação crescem com o clipe).
    pub fn timeout_for(&self, clip_ms: u64) -> Duration {
        self.timeout + Duration::from_millis(clip_ms / 5)
    }
}

// ---------------------------------------------------------------- pedido

/// Idioma único (ISO 639-1) quando o usuário configurou só um; vários ou nenhum = detectar.
pub fn language_param(opts: &SttOptions) -> Option<String> {
    match opts.languages.as_slice() {
        [only] => {
            let code: String = only
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .take(2)
                .collect::<String>()
                .to_lowercase();
            (code.len() == 2).then_some(code)
        }
        _ => None,
    }
}

/// Vocabulário do dicionário como prompt inicial: sem repetidos, até [`MAX_PROMPT_CHARS`].
pub fn build_prompt(vocabulary: &[String]) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    for word in vocabulary
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
    {
        if !seen.iter().any(|s| s.eq_ignore_ascii_case(word)) {
            seen.push(word);
        }
    }
    let mut out = String::new();
    for word in seen {
        let extra = if out.is_empty() { 0 } else { 2 };
        if out.chars().count() + extra + word.chars().count() > MAX_PROMPT_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(word);
    }
    (!out.is_empty()).then_some(out)
}

/// Corpo `multipart/form-data`: campos de texto e o arquivo de áudio.
pub fn multipart_body(
    boundary: &str,
    fields: &[(&str, &str)],
    file_name: &str,
    mime: &str,
    file: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(file.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

// ---------------------------------------------------------------- resposta

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

/// Mensagem do corpo de erro (`{"error":{"message":..}}` ou texto cru).
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string())
}

/// Traduz uma resposta que não é 2xx em erro tipado.
pub fn classify_failure(
    provider: CloudProvider,
    status: u16,
    retry_after_secs: Option<u64>,
    body: &str,
    key: &str,
) -> CloudSttError {
    let name = provider.name();
    match status {
        401 | 403 => CloudSttError::Unauthorized(name),
        408 | 504 => CloudSttError::Timeout(name),
        413 => CloudSttError::TooLarge {
            size: 0,
            limit: MAX_UPLOAD_BYTES,
        },
        429 => CloudSttError::RateLimited {
            provider: name,
            retry_after_secs,
        },
        500..=599 => CloudSttError::Server {
            provider: name,
            status,
        },
        _ => CloudSttError::Rejected {
            provider: name,
            status,
            message: scrub(&error_message(body), key),
        },
    }
}

/// Lê o `verbose_json` (ou o `json` simples) para um [`Transcript`].
pub fn parse_transcript(
    provider: CloudProvider,
    body: &[u8],
    elapsed: Duration,
) -> std::result::Result<Transcript, CloudSttError> {
    let bad = |detail: &str| CloudSttError::BadResponse {
        provider: provider.name(),
        detail: detail.to_string(),
    };
    let value: Value = serde_json::from_slice(body).map_err(|_| bad("não é JSON"))?;
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("sem o campo \"text\""))?
        .trim()
        .to_string();
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .map(|l| l.to_string());
    let to_ms = |seconds: f64| (seconds.max(0.0) * 1000.0).round() as u64;
    let segments = value
        .get("segments")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|seg| {
                    Some(TranscriptSegment {
                        start_ms: to_ms(seg.get("start")?.as_f64()?),
                        end_ms: to_ms(seg.get("end")?.as_f64()?),
                        text: seg.get("text")?.as_str()?.trim().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Transcript {
        text,
        language,
        segments,
        timings: SttTimings {
            load_ms: 0,
            decode_ms: elapsed.as_millis() as u64,
        },
    })
}

// ---------------------------------------------------------------- motor

/// Núcleo comum aos dois provedores.
pub struct CloudStt {
    provider: CloudProvider,
    keys: Arc<dyn KeyProvider>,
    cfg: CloudConfig,
    client: reqwest::Client,
}

// Sem a chave (que nem fica guardada aqui) e sem o cliente HTTP.
impl std::fmt::Debug for CloudStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudStt")
            .field("provider", &self.provider)
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl CloudStt {
    pub fn new(provider: CloudProvider, keys: Arc<dyn KeyProvider>, cfg: CloudConfig) -> Self {
        Self {
            provider,
            keys,
            cfg,
            client: reqwest::Client::new(),
        }
    }

    pub fn provider(&self) -> CloudProvider {
        self.provider
    }

    fn url(&self) -> &str {
        self.cfg
            .url
            .as_deref()
            .unwrap_or_else(|| self.provider.default_url())
    }

    fn model(&self) -> &str {
        self.cfg
            .model
            .as_deref()
            .unwrap_or_else(|| self.provider.default_model())
    }

    async fn run(
        &self,
        clip: &AudioClip,
        opts: &SttOptions,
    ) -> std::result::Result<Transcript, CloudSttError> {
        let name = self.provider.name();
        let clip_ms = clip.duration_ms();
        if clip.pcm.is_empty() || clip_ms < MIN_AUDIO_MS {
            return Err(CloudSttError::EmptyAudio);
        }
        let key = self
            .keys
            .api_key(self.provider.secret())
            .map_err(CloudSttError::KeyStore)?
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .ok_or(CloudSttError::MissingKey(name))?;

        let audio = wav::encode(&clip.pcm, clip.sample_rate);
        if audio.len() > MAX_UPLOAD_BYTES {
            return Err(CloudSttError::TooLarge {
                size: audio.len(),
                limit: MAX_UPLOAD_BYTES,
            });
        }

        let language = language_param(opts);
        let prompt = build_prompt(&opts.vocabulary);
        let mut fields = vec![
            ("model", self.model()),
            ("response_format", "verbose_json"),
            ("temperature", "0"),
        ];
        if let Some(lang) = language.as_deref() {
            fields.push(("language", lang));
        }
        if let Some(p) = prompt.as_deref() {
            fields.push(("prompt", p));
        }
        let boundary = format!("vyrp-{}", uuid::Uuid::new_v4().simple());
        let body = multipart_body(&boundary, &fields, "audio.wav", "audio/wav", &audio);

        let started = Instant::now();
        tracing::info!(
            provider = name,
            bytes = body.len(),
            "enviando áudio para transcrição"
        );
        let response = self
            .client
            .post(self.url())
            .bearer_auth(&key)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .timeout(self.cfg.timeout_for(clip_ms))
            .body(body)
            .send()
            .await
            .map_err(|e| self.network_error(e, &key))?;

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let bytes = response
            .bytes()
            .await
            .map_err(|e| self.network_error(e, &key))?;
        tracing::info!(provider = name, status, "resposta da transcrição");

        if !(200..300).contains(&status) {
            let text = String::from_utf8_lossy(&bytes);
            return Err(classify_failure(
                self.provider,
                status,
                retry_after,
                &text,
                &key,
            ));
        }
        parse_transcript(self.provider, &bytes, started.elapsed())
    }

    fn network_error(&self, err: reqwest::Error, key: &str) -> CloudSttError {
        let name = self.provider.name();
        if err.is_timeout() {
            return CloudSttError::Timeout(name);
        }
        CloudSttError::Network {
            provider: name,
            detail: scrub(&err.without_url().to_string(), key),
        }
    }
}

#[async_trait]
impl SttEngine for CloudStt {
    async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
        self.run(clip, opts).await.map_err(Into::into)
    }
}

macro_rules! provider_engine {
    ($(#[$doc:meta])* $name:ident, $provider:expr) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name(CloudStt);

        impl $name {
            pub fn new(keys: Arc<dyn KeyProvider>) -> Self {
                Self::with_config(keys, CloudConfig::default())
            }

            pub fn with_config(keys: Arc<dyn KeyProvider>, cfg: CloudConfig) -> Self {
                Self(CloudStt::new($provider, keys, cfg))
            }
        }

        #[async_trait]
        impl SttEngine for $name {
            async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
                self.0.transcribe(clip, opts).await
            }
        }
    };
}

provider_engine!(
    /// Groq (`whisper-large-v3-turbo`).
    GroqEngine,
    CloudProvider::Groq
);
provider_engine!(
    /// OpenAI (`whisper-1`).
    OpenAiEngine,
    CloudProvider::OpenAi
);

// ---------------------------------------------------------------- política

/// Onde transcrever (spec §6, V10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttPolicy {
    /// Só o motor local: nada sai da máquina.
    #[default]
    LocalOnly,
    /// Só a nuvem: erro da nuvem é erro do ditado.
    CloudOnly,
    /// Nuvem primeiro; se falhar (inclusive por prazo), tenta o local.
    CloudWithFallback,
}

/// Motor que aplica a [`SttPolicy`] sobre um motor de nuvem e um local.
pub struct PolicyStt {
    policy: SttPolicy,
    cloud: Option<Arc<dyn SttEngine>>,
    local: Option<Arc<dyn SttEngine>>,
}

impl PolicyStt {
    pub fn new(
        policy: SttPolicy,
        cloud: Option<Arc<dyn SttEngine>>,
        local: Option<Arc<dyn SttEngine>>,
    ) -> Self {
        Self {
            policy,
            cloud,
            local,
        }
    }

    fn missing(which: &str) -> VyrpError {
        VyrpError::Stt(format!("motor {which} não configurado"))
    }
}

#[async_trait]
impl SttEngine for PolicyStt {
    async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
        match self.policy {
            SttPolicy::LocalOnly => match &self.local {
                Some(local) => local.transcribe(clip, opts).await,
                None => Err(Self::missing("local")),
            },
            SttPolicy::CloudOnly => match &self.cloud {
                Some(cloud) => cloud.transcribe(clip, opts).await,
                None => Err(Self::missing("de nuvem")),
            },
            SttPolicy::CloudWithFallback => {
                let Some(cloud) = &self.cloud else {
                    return match &self.local {
                        Some(local) => local.transcribe(clip, opts).await,
                        None => Err(Self::missing("local")),
                    };
                };
                match cloud.transcribe(clip, opts).await {
                    Ok(done) => Ok(done),
                    Err(err) => {
                        let Some(local) = &self.local else {
                            return Err(err);
                        };
                        tracing::warn!(error = %err, "nuvem falhou; usando o motor local");
                        local.transcribe(clip, opts).await
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    const KEY: &str = "gsk_SEGREDO_123";

    // ------------------------------------------------------------ apoio

    struct FixedKey(Option<String>);
    impl KeyProvider for FixedKey {
        fn api_key(&self, _: Provider) -> std::result::Result<Option<String>, String> {
            Ok(self.0.clone())
        }
    }

    struct BrokenKeys;
    impl KeyProvider for BrokenKeys {
        fn api_key(&self, _: Provider) -> std::result::Result<Option<String>, String> {
            Err("credential manager travou".into())
        }
    }

    fn keys(key: Option<&str>) -> Arc<dyn KeyProvider> {
        Arc::new(FixedKey(key.map(Into::into)))
    }

    fn clip(ms: u64) -> AudioClip {
        AudioClip::new(vec![100; (16 * ms) as usize])
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    struct Seen {
        head: String,
        body: Vec<u8>,
    }

    /// Servidor HTTP falso: atende UMA requisição com o status/corpo dados e a devolve
    /// pelo canal. `delay` simula um provedor lento.
    fn mock_server(
        status: u16,
        extra_header: &'static str,
        reply: &'static str,
        delay: Duration,
    ) -> (String, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/v1/audio/transcriptions",
            listener.local_addr().unwrap()
        );
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

    fn engine(
        provider: CloudProvider,
        key: Option<&str>,
        url: &str,
        timeout: Duration,
    ) -> CloudStt {
        CloudStt::new(
            provider,
            keys(key),
            CloudConfig {
                timeout,
                url: Some(url.into()),
                ..Default::default()
            },
        )
    }

    const OK_REPLY: &str = r#"{"text":" Olá, mundo. ","language":"portuguese","duration":1.2,
        "segments":[{"start":0.0,"end":0.62,"text":" Olá,"},{"start":0.62,"end":1.2,"text":" mundo."}]}"#;

    // ------------------------------------------------------------ pedido

    #[test]
    fn language_only_when_a_single_one_is_configured() {
        let opts = |l: &[&str]| SttOptions {
            languages: l.iter().map(|s| s.to_string()).collect(),
            vocabulary: vec![],
        };
        assert_eq!(language_param(&opts(&["pt"])).as_deref(), Some("pt"));
        assert_eq!(language_param(&opts(&["pt-BR"])).as_deref(), Some("pt"));
        assert_eq!(language_param(&opts(&["EN"])).as_deref(), Some("en"));
        assert_eq!(language_param(&opts(&["pt", "en"])), None);
        assert_eq!(language_param(&opts(&[])), None);
        assert_eq!(language_param(&opts(&["x"])), None);
    }

    #[test]
    fn prompt_dedupes_and_respects_the_limit() {
        let v = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            build_prompt(&v(&["Vyrp", " vyrp ", "Tauri", ""])).as_deref(),
            Some("Vyrp, Tauri")
        );
        assert_eq!(build_prompt(&[]), None);
        let many: Vec<String> = (0..500).map(|i| format!("palavra{i}")).collect();
        let prompt = build_prompt(&many).unwrap();
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.starts_with("palavra0, palavra1"));
    }

    #[test]
    fn multipart_body_has_fields_file_and_closing_boundary() {
        let body = multipart_body(
            "B",
            &[("model", "m"), ("language", "pt")],
            "a.wav",
            "audio/wav",
            b"RIFFxx",
        );
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(
            text.contains("--B\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nm\r\n")
        );
        assert!(text.contains("name=\"language\"\r\n\r\npt\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFFxx\r\n--B--\r\n"));
    }

    #[test]
    fn timeout_grows_with_the_clip() {
        let cfg = CloudConfig::default();
        assert_eq!(cfg.timeout_for(0), Duration::from_secs(3));
        assert_eq!(cfg.timeout_for(60_000), Duration::from_secs(15));
    }

    // ------------------------------------------------------------ resposta

    #[test]
    fn status_codes_map_to_typed_errors() {
        let c = |status, retry| classify_failure(CloudProvider::Groq, status, retry, "{}", KEY);
        assert_eq!(c(401, None), CloudSttError::Unauthorized("Groq"));
        assert_eq!(c(403, None), CloudSttError::Unauthorized("Groq"));
        assert_eq!(
            c(429, Some(7)),
            CloudSttError::RateLimited {
                provider: "Groq",
                retry_after_secs: Some(7)
            }
        );
        assert_eq!(
            c(503, None),
            CloudSttError::Server {
                provider: "Groq",
                status: 503
            }
        );
        assert_eq!(c(504, None), CloudSttError::Timeout("Groq"));
        assert!(matches!(c(413, None), CloudSttError::TooLarge { .. }));
        match c(400, None) {
            CloudSttError::Rejected { status, .. } => assert_eq!(status, 400),
            other => panic!("esperava Rejected, veio {other:?}"),
        }
    }

    #[test]
    fn rejected_message_comes_from_the_body_and_never_carries_the_key() {
        let body = format!(r#"{{"error":{{"message":"chave {KEY} inválida para o modelo"}}}}"#);
        let err = classify_failure(CloudProvider::OpenAi, 400, None, &body, KEY);
        let text = err.to_string();
        assert!(text.contains("inválida para o modelo"));
        assert!(!text.contains(KEY), "a chave vazou: {text}");
        assert!(text.contains("[chave]"));
    }

    #[test]
    fn parse_transcript_reads_text_language_and_segments() {
        let t = parse_transcript(
            CloudProvider::Groq,
            OK_REPLY.as_bytes(),
            Duration::from_millis(321),
        )
        .unwrap();
        assert_eq!(t.text, "Olá, mundo.");
        assert_eq!(t.language.as_deref(), Some("portuguese"));
        assert_eq!(t.segments.len(), 2);
        assert_eq!(
            t.segments[1],
            TranscriptSegment {
                start_ms: 620,
                end_ms: 1200,
                text: "mundo.".into()
            }
        );
        assert_eq!(t.timings.decode_ms, 321);
    }

    #[test]
    fn plain_json_without_segments_still_works_and_garbage_is_typed() {
        let t =
            parse_transcript(CloudProvider::OpenAi, br#"{"text":"oi"}"#, Duration::ZERO).unwrap();
        assert_eq!((t.text.as_str(), t.segments.len()), ("oi", 0));
        for bad in [&b"<html>"[..], &br#"{"nada":1}"#[..]] {
            assert!(matches!(
                parse_transcript(CloudProvider::OpenAi, bad, Duration::ZERO),
                Err(CloudSttError::BadResponse { .. })
            ));
        }
    }

    #[test]
    fn errors_convert_to_vyrp_errors() {
        assert!(matches!(
            VyrpError::from(CloudSttError::Timeout("Groq")),
            VyrpError::Timeout(_)
        ));
        assert!(matches!(
            VyrpError::from(CloudSttError::Unauthorized("Groq")),
            VyrpError::Stt(_)
        ));
        assert!(CloudSttError::Timeout("x").allows_fallback());
        assert!(!CloudSttError::EmptyAudio.allows_fallback());
    }

    // ------------------------------------------------------------ servidor falso

    #[test]
    fn groq_uploads_wav_with_key_model_language_and_prompt() {
        let (url, rx) = mock_server(200, "", OK_REPLY, Duration::ZERO);
        let groq = GroqEngine::with_config(
            keys(Some(KEY)),
            CloudConfig {
                url: Some(url),
                ..Default::default()
            },
        );
        let opts = SttOptions {
            languages: vec!["pt-BR".into()],
            vocabulary: vec!["Vyrp".into(), "Tauri".into()],
        };
        let out = block_on(groq.transcribe(&clip(1_200), &opts)).unwrap();
        assert_eq!(out.text, "Olá, mundo.");
        assert_eq!(out.segments.len(), 2);

        let seen = rx.recv().unwrap();
        let head = seen.head.to_lowercase();
        assert!(head.starts_with("post /v1/audio/transcriptions"));
        assert!(head.contains(&format!("authorization: bearer {}", KEY.to_lowercase())));
        assert!(head.contains("content-type: multipart/form-data; boundary=vyrp-"));
        let body = String::from_utf8_lossy(&seen.body).to_string();
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-large-v3-turbo"));
        assert!(body.contains("name=\"response_format\"\r\n\r\nverbose_json"));
        assert!(body.contains("name=\"language\"\r\n\r\npt\r\n"));
        assert!(body.contains("name=\"prompt\"\r\n\r\nVyrp, Tauri\r\n"));
        assert!(body.contains("filename=\"audio.wav\""));
        assert!(seen.body.windows(4).any(|w| w == b"RIFF"));
    }

    #[test]
    fn openai_uses_its_own_model_and_omits_language_when_auto() {
        let (url, rx) = mock_server(200, "", OK_REPLY, Duration::ZERO);
        let engine = OpenAiEngine::with_config(
            keys(Some("sk-abc")),
            CloudConfig {
                url: Some(url),
                ..Default::default()
            },
        );
        block_on(engine.transcribe(&clip(500), &SttOptions::default())).unwrap();
        let seen = rx.recv().unwrap();
        let body = String::from_utf8_lossy(&seen.body).to_string();
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1"));
        assert!(!body.contains("name=\"language\""));
        assert!(!body.contains("name=\"prompt\""));
        assert!(
            seen.head
                .to_lowercase()
                .contains("authorization: bearer sk-abc")
        );
    }

    #[test]
    fn unauthorized_never_echoes_the_key() {
        let (url, _rx) = mock_server(
            401,
            "",
            r#"{"error":{"message":"Incorrect API key provided: gsk_SEGREDO_123"}}"#,
            Duration::ZERO,
        );
        let e = engine(CloudProvider::Groq, Some(KEY), &url, Duration::from_secs(3));
        let err = block_on(e.run(&clip(500), &SttOptions::default())).unwrap_err();
        assert_eq!(err, CloudSttError::Unauthorized("Groq"));
        assert!(!err.to_string().contains(KEY));
        assert!(!format!("{e:?}").contains(KEY));
    }

    #[test]
    fn rate_limit_reports_retry_after() {
        let (url, _rx) = mock_server(429, "Retry-After: 12\r\n", "{}", Duration::ZERO);
        let e = engine(
            CloudProvider::OpenAi,
            Some("k"),
            &url,
            Duration::from_secs(3),
        );
        let err = block_on(e.run(&clip(500), &SttOptions::default())).unwrap_err();
        assert_eq!(
            err,
            CloudSttError::RateLimited {
                provider: "OpenAI",
                retry_after_secs: Some(12)
            }
        );
        assert!(err.to_string().contains("12 s"));
    }

    #[test]
    fn server_error_and_bad_body_are_typed() {
        let (url, _rx) = mock_server(502, "", "bad gateway", Duration::ZERO);
        let e = engine(CloudProvider::Groq, Some("k"), &url, Duration::from_secs(3));
        assert_eq!(
            block_on(e.run(&clip(500), &SttOptions::default())).unwrap_err(),
            CloudSttError::Server {
                provider: "Groq",
                status: 502
            }
        );
        let (url, _rx) = mock_server(200, "", "isto não é json", Duration::ZERO);
        let e = engine(CloudProvider::Groq, Some("k"), &url, Duration::from_secs(3));
        assert!(matches!(
            block_on(e.run(&clip(500), &SttOptions::default())),
            Err(CloudSttError::BadResponse { .. })
        ));
    }

    #[test]
    fn slow_server_times_out() {
        let (url, _rx) = mock_server(200, "", OK_REPLY, Duration::from_millis(1_500));
        let e = engine(
            CloudProvider::Groq,
            Some("k"),
            &url,
            Duration::from_millis(250),
        );
        let started = Instant::now();
        let err = block_on(e.run(&clip(500), &SttOptions::default())).unwrap_err();
        assert_eq!(err, CloudSttError::Timeout("Groq"));
        assert!(
            started.elapsed() < Duration::from_millis(1_400),
            "o prazo não cortou a espera"
        );
    }

    #[test]
    fn dead_server_is_a_network_error() {
        // Porta reservada e fechada em seguida: ninguém atende.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let e = engine(
            CloudProvider::Groq,
            Some("k"),
            &format!("http://127.0.0.1:{port}/x"),
            Duration::from_secs(3),
        );
        assert!(matches!(
            block_on(e.run(&clip(500), &SttOptions::default())),
            Err(CloudSttError::Network { .. })
        ));
    }

    // ------------------------------------------------------------ validações sem rede

    #[test]
    fn missing_or_blank_key_never_touches_the_network() {
        for key in [None, Some("   ")] {
            // URL inválida de propósito: se tentasse enviar, o erro seria de rede.
            let e = engine(
                CloudProvider::Groq,
                key,
                "http://127.0.0.1:1/x",
                Duration::from_secs(3),
            );
            assert_eq!(
                block_on(e.run(&clip(500), &SttOptions::default())).unwrap_err(),
                CloudSttError::MissingKey("Groq")
            );
        }
    }

    #[test]
    fn key_store_failure_is_typed() {
        let e = CloudStt::new(
            CloudProvider::OpenAi,
            Arc::new(BrokenKeys),
            CloudConfig::default(),
        );
        assert_eq!(
            block_on(e.run(&clip(500), &SttOptions::default())).unwrap_err(),
            CloudSttError::KeyStore("credential manager travou".into())
        );
    }

    #[test]
    fn empty_and_oversized_audio_are_refused_before_sending() {
        let e = engine(
            CloudProvider::Groq,
            Some("k"),
            "http://127.0.0.1:1/x",
            Duration::from_secs(3),
        );
        assert_eq!(
            block_on(e.run(&AudioClip::new(vec![]), &SttOptions::default())).unwrap_err(),
            CloudSttError::EmptyAudio
        );
        assert_eq!(
            block_on(e.run(&clip(50), &SttOptions::default())).unwrap_err(),
            CloudSttError::EmptyAudio
        );
        let huge = AudioClip::new(vec![0; MAX_UPLOAD_BYTES / 2 + 1_000]);
        assert!(matches!(
            block_on(e.run(&huge, &SttOptions::default())),
            Err(CloudSttError::TooLarge { .. })
        ));
    }

    #[test]
    fn trait_surface_returns_vyrp_errors() {
        let e = GroqEngine::new(keys(None));
        let err = block_on(e.transcribe(&clip(500), &SttOptions::default())).unwrap_err();
        assert!(matches!(err, VyrpError::Stt(m) if m.contains("chave de API")));
    }

    // ------------------------------------------------------------ política

    struct Fake {
        calls: Arc<AtomicUsize>,
        result: std::result::Result<&'static str, &'static str>,
    }

    #[async_trait]
    impl SttEngine for Fake {
        async fn transcribe(&self, _: &AudioClip, _: &SttOptions) -> Result<Transcript> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.result {
                Ok(text) => Ok(Transcript {
                    text: text.into(),
                    ..Default::default()
                }),
                Err(msg) => Err(VyrpError::Stt(msg.into())),
            }
        }
    }

    fn fake(
        result: std::result::Result<&'static str, &'static str>,
    ) -> (Arc<dyn SttEngine>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Fake {
                calls: calls.clone(),
                result,
            }),
            calls,
        )
    }

    fn run_policy(
        policy: SttPolicy,
        cloud: Option<Arc<dyn SttEngine>>,
        local: Option<Arc<dyn SttEngine>>,
    ) -> Result<Transcript> {
        block_on(
            PolicyStt::new(policy, cloud, local).transcribe(&clip(500), &SttOptions::default()),
        )
    }

    #[test]
    fn local_only_never_calls_the_cloud() {
        let (cloud, cloud_calls) = fake(Ok("nuvem"));
        let (local, local_calls) = fake(Ok("local"));
        let out = run_policy(SttPolicy::LocalOnly, Some(cloud), Some(local)).unwrap();
        assert_eq!(out.text, "local");
        assert_eq!(
            (
                cloud_calls.load(Ordering::SeqCst),
                local_calls.load(Ordering::SeqCst)
            ),
            (0, 1)
        );
    }

    #[test]
    fn cloud_only_surfaces_cloud_errors_without_fallback() {
        let (cloud, _) = fake(Err("nuvem caiu"));
        let (local, local_calls) = fake(Ok("local"));
        let err = run_policy(SttPolicy::CloudOnly, Some(cloud), Some(local)).unwrap_err();
        assert!(matches!(err, VyrpError::Stt(m) if m == "nuvem caiu"));
        assert_eq!(local_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fallback_uses_local_only_when_the_cloud_fails() {
        let (cloud, cloud_calls) = fake(Ok("nuvem"));
        let (local, local_calls) = fake(Ok("local"));
        assert_eq!(
            run_policy(SttPolicy::CloudWithFallback, Some(cloud), Some(local))
                .unwrap()
                .text,
            "nuvem"
        );
        assert_eq!(
            (
                cloud_calls.load(Ordering::SeqCst),
                local_calls.load(Ordering::SeqCst)
            ),
            (1, 0)
        );

        let (cloud, _) = fake(Err("timeout"));
        let (local, local_calls) = fake(Ok("local"));
        assert_eq!(
            run_policy(SttPolicy::CloudWithFallback, Some(cloud), Some(local))
                .unwrap()
                .text,
            "local"
        );
        assert_eq!(local_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fallback_without_local_keeps_the_cloud_error_and_without_cloud_goes_local() {
        let (cloud, _) = fake(Err("nuvem caiu"));
        let err = run_policy(SttPolicy::CloudWithFallback, Some(cloud), None).unwrap_err();
        assert!(matches!(err, VyrpError::Stt(m) if m == "nuvem caiu"));
        let (local, _) = fake(Ok("local"));
        assert_eq!(
            run_policy(SttPolicy::CloudWithFallback, None, Some(local))
                .unwrap()
                .text,
            "local"
        );
        assert!(run_policy(SttPolicy::LocalOnly, None, None).is_err());
    }

    #[test]
    fn policy_wire_names_and_default() {
        assert_eq!(SttPolicy::default(), SttPolicy::LocalOnly);
        assert_eq!(
            serde_json::to_string(&SttPolicy::CloudWithFallback).unwrap(),
            "\"cloud_with_fallback\""
        );
    }

    #[test]
    fn real_secrets_type_works_as_a_key_provider() {
        let provider: &dyn KeyProvider = &Secrets::with_backend(Box::new(NoKeys));
        assert_eq!(provider.api_key(Provider::Groq).unwrap(), None);
    }

    struct NoKeys;
    impl crate::settings::secrets::SecretBackend for NoKeys {
        fn set(&self, _: Provider, _: &str) -> std::result::Result<(), String> {
            Ok(())
        }
        fn get(&self, _: Provider) -> std::result::Result<Option<String>, String> {
            Ok(None)
        }
        fn clear(&self, _: Provider) -> std::result::Result<(), String> {
            Ok(())
        }
    }
}
