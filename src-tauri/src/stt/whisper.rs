//! Motor local whisper.cpp (whisper-rs): carregado em segundo plano no boot e mantido
//! quente (com uma decodificação de aquecimento), atrás do trait [`SttEngine`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::Serialize;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::core::types::{
    AudioClip, Result, SAMPLE_RATE, SttEngine, SttOptions, SttTimings, Transcript,
    TranscriptSegment, VyrpError,
};
use crate::core::ui_error::UiError;

/// Áudio mais curto que isso não vai ao whisper (vira transcrição vazia).
const MIN_AUDIO_MS: u64 = 300;
/// Teto do prompt inicial (vocabulário); o whisper usa ~224 tokens de contexto.
const MAX_PROMPT_CHARS: usize = 600;
/// Quanto do texto já decodificado entra no prompt do próximo segmento (V9b).
const MAX_PREVIOUS_CHARS: usize = 200;
/// Contexto do encoder = 30 s de áudio (1500 quadros de 20 ms).
const FULL_AUDIO_CTX: i32 = 1500;
/// Folga sobre a duração real ao reduzir o contexto do encoder.
const AUDIO_CTX_MARGIN: i32 = 128;

/// Segmento que continua um ditado já em parte decodificado (incremental, V9b).
#[derive(Debug, Clone, Default)]
pub struct Continuation {
    /// Texto dos segmentos anteriores: o fim dele vai no prompt (contexto e grafia).
    pub previous: String,
    /// `true` interrompe a decodificação (Cancel do ditado).
    pub abort: Option<Arc<AtomicBool>>,
}

/// Como rodar o whisper: ditado normal ou passada de aquecimento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    Dictation,
    WarmUp,
}

/// Aceleração compilada neste binário (feature do Cargo).
pub fn backend() -> &'static str {
    if cfg!(feature = "gpu-cuda") {
        "cuda"
    } else if cfg!(feature = "gpu-vulkan") {
        "vulkan"
    } else {
        "cpu"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadState {
    None,
    Loading,
    Ready,
    Error,
}

/// Payload de `model_status` e do evento `stt://status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SttStatus {
    pub model: Option<String>,
    pub state: LoadState,
    pub backend: &'static str,
    /// Carga + aquecimento, em ms.
    pub load_ms: Option<u64>,
    pub error: Option<UiError>,
}

type StatusHook = Box<dyn Fn(&SttStatus) + Send + Sync>;

pub struct WhisperEngine {
    inner: Mutex<Inner>,
    on_status: StatusHook,
}

struct Inner {
    status: SttStatus,
    ctx: Option<Arc<WhisperContext>>,
    /// Incrementa a cada `load`: uma carga antiga que termina tarde é descartada.
    generation: u64,
}

impl WhisperEngine {
    pub fn new(on_status: impl Fn(&SttStatus) + Send + Sync + 'static) -> Arc<Self> {
        // Logs do whisper.cpp vão para o `tracing` (feature tracing_backend), não para o stderr.
        whisper_rs::install_logging_hooks();
        Arc::new(Self {
            inner: Mutex::new(Inner {
                status: SttStatus {
                    model: None,
                    state: LoadState::None,
                    backend: backend(),
                    load_ms: None,
                    error: None,
                },
                ctx: None,
                generation: 0,
            }),
            on_status: Box::new(on_status),
        })
    }

    pub fn status(&self) -> SttStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Modelo carregado ou carregando.
    pub fn current_model(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        matches!(inner.status.state, LoadState::Loading | LoadState::Ready)
            .then(|| inner.status.model.clone())
            .flatten()
    }

    /// Carrega `path` numa thread própria; o anterior segue respondendo até o novo ficar pronto.
    pub fn load(self: &Arc<Self>, id: &str, path: PathBuf) {
        let generation = {
            let mut inner = self.inner.lock().unwrap();
            inner.generation += 1;
            inner.status = SttStatus {
                model: Some(id.to_string()),
                state: LoadState::Loading,
                backend: backend(),
                load_ms: None,
                error: None,
            };
            inner.generation
        };
        self.notify();
        let engine = Arc::clone(self);
        let id = id.to_string();
        std::thread::Builder::new()
            .name("vyrp-stt-load".into())
            .spawn(move || {
                let started = Instant::now();
                let result = load_and_warm(&path);
                let mut inner = engine.inner.lock().unwrap();
                if inner.generation != generation {
                    return; // outro modelo foi pedido nesse meio-tempo
                }
                match result {
                    Ok(ctx) => {
                        let ms = started.elapsed().as_millis() as u64;
                        tracing::info!(model = %id, backend = backend(), load_ms = ms, "modelo whisper pronto");
                        inner.ctx = Some(Arc::new(ctx));
                        inner.status.state = LoadState::Ready;
                        inner.status.load_ms = Some(ms);
                    }
                    Err(err) => {
                        tracing::error!(model = %id, %err, "falha ao carregar o modelo whisper");
                        inner.status.state = LoadState::Error;
                        inner.status.error = Some(
                            UiError::new("model.load_failed", format!("falha ao carregar o modelo: {err}"))
                                .with("detail", err),
                        );
                    }
                }
                drop(inner);
                engine.notify();
            })
            .expect("thread de carga do whisper");
    }

    /// Carga síncrona (CLI de benchmark): devolve carga + aquecimento em ms.
    pub fn load_blocking(
        &self,
        id: &str,
        path: &std::path::Path,
    ) -> std::result::Result<u64, String> {
        let started = Instant::now();
        let ctx = load_and_warm(path)?;
        let ms = started.elapsed().as_millis() as u64;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.generation += 1;
            inner.ctx = Some(Arc::new(ctx));
            inner.status = SttStatus {
                model: Some(id.to_string()),
                state: LoadState::Ready,
                backend: backend(),
                load_ms: Some(ms),
                error: None,
            };
        }
        self.notify();
        Ok(ms)
    }

    fn notify(&self) {
        let status = self.status();
        (self.on_status)(&status);
    }

    fn context(&self) -> Result<Arc<WhisperContext>> {
        let inner = self.inner.lock().unwrap();
        inner.ctx.clone().ok_or_else(|| {
            VyrpError::Stt(match inner.status.state {
                LoadState::Loading => "o modelo ainda está carregando".into(),
                LoadState::Error => inner
                    .status
                    .error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                _ => "nenhum modelo de transcrição instalado".into(),
            })
        })
    }

    /// Transcreve bloqueando a thread atual (use fora do runtime async).
    pub fn transcribe_blocking(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
        let ctx = self.context()?;
        run(&ctx, clip, opts)
    }

    /// Como [`transcribe_blocking`](Self::transcribe_blocking), para um segmento que
    /// continua `cont.previous` e pode ser interrompido por `cont.abort`.
    pub fn transcribe_continuing(
        &self,
        clip: &AudioClip,
        opts: &SttOptions,
        cont: &Continuation,
    ) -> Result<Transcript> {
        let ctx = self.context()?;
        run_pass(&ctx, clip, opts, Pass::Dictation, cont)
    }

    /// Há modelo carregado e pronto?
    pub fn is_ready(&self) -> bool {
        self.inner.lock().unwrap().ctx.is_some()
    }
}

#[async_trait]
impl SttEngine for WhisperEngine {
    async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
        let ctx = self.context()?;
        let (clip, opts) = (clip.clone(), opts.clone());
        tauri::async_runtime::spawn_blocking(move || run(&ctx, &clip, &opts))
            .await
            .map_err(|e| VyrpError::Stt(format!("tarefa de transcrição falhou: {e}")))?
    }
}

fn load_and_warm(path: &std::path::Path) -> std::result::Result<WhisperContext, String> {
    let mut params = WhisperContextParameters::default();
    params.use_gpu(backend() != "cpu");
    let ctx = WhisperContext::new_with_params(path, params).map_err(|e| e.to_string())?;
    // Aquecimento: aloca os buffers de computação agora, não no primeiro ditado.
    let warm = AudioClip::new(vec![0; SAMPLE_RATE as usize]);
    run_pass(
        &ctx,
        &warm,
        &SttOptions::default(),
        Pass::WarmUp,
        &Continuation::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok(ctx)
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4) as i32
}

/// Prompt inicial a partir do vocabulário (ajuda o whisper a grafar nomes próprios).
pub fn initial_prompt(vocabulary: &[String]) -> String {
    let mut prompt = String::new();
    for word in vocabulary {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if prompt.len() + word.len() + 2 > MAX_PROMPT_CHARS {
            break;
        }
        if !prompt.is_empty() {
            prompt.push_str(", ");
        }
        prompt.push_str(word);
    }
    prompt
}

/// Prompt de um segmento que continua o ditado: vocabulário e, por último (o whisper
/// fica com o fim do prompt quando passa do limite), o fim do texto anterior.
pub fn continuation_prompt(vocabulary: &[String], previous: &str) -> String {
    let vocab = initial_prompt(vocabulary);
    // Reticências no fim do texto anterior induzem o whisper a pôr mais (pausas).
    let previous = crate::post::strip_pause_ellipses(previous);
    let previous = previous.trim();
    if previous.is_empty() {
        return vocab;
    }
    let chars: Vec<char> = previous.chars().collect();
    let mut tail: String = chars[chars.len().saturating_sub(MAX_PREVIOUS_CHARS)..]
        .iter()
        .collect();
    if chars.len() > MAX_PREVIOUS_CHARS {
        // Começa numa palavra inteira.
        if let Some(space) = tail.find(' ') {
            tail = tail[space + 1..].to_string();
        }
    }
    if vocab.is_empty() {
        tail
    } else {
        format!("{vocab}. {tail}")
    }
}

/// Contexto do encoder proporcional ao áudio (o whisper sempre codifica 30 s; para um
/// ditado de poucos segundos isso é a maior parte da latência). 0 = padrão (30 s).
pub fn audio_ctx_for(duration_ms: u64) -> i32 {
    let frames = (duration_ms as f64 / 30_000.0 * f64::from(FULL_AUDIO_CTX)).ceil() as i32;
    let ctx = frames + AUDIO_CTX_MARGIN;
    if ctx >= FULL_AUDIO_CTX { 0 } else { ctx }
}

/// Contexto do encoder para um modelo com `n_mels` bandas. Os modelos v3 (128 mels, ex.:
/// large-v3-turbo) entram em laço e repetem frases com o contexto reduzido: neles vale
/// o padrão (30 s). Medido em 2026-10-21 (V-GPU): turbo com 11 s repetia a 1ª frase.
pub fn audio_ctx_for_model(n_mels: i32, duration_ms: u64) -> i32 {
    if n_mels >= 128 {
        0
    } else {
        audio_ctx_for(duration_ms)
    }
}

/// Idioma fixo se houver exatamente um; senão autodetecção.
pub fn language_for(opts: &SttOptions) -> Option<&str> {
    match opts.languages.as_slice() {
        [only] => Some(only.split(['-', '_']).next().unwrap_or(only)),
        _ => None,
    }
}

/// Callback de abort do whisper.cpp: lê o `AtomicBool` de [`Continuation::abort`].
unsafe extern "C" fn abort_requested(data: *mut std::ffi::c_void) -> bool {
    // SAFETY: `data` aponta para o `AtomicBool` que `run_pass` mantém vivo.
    unsafe { (*(data as *const AtomicBool)).load(Ordering::Relaxed) }
}

fn run(ctx: &WhisperContext, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
    run_pass(ctx, clip, opts, Pass::Dictation, &Continuation::default())
}

fn run_pass(
    ctx: &WhisperContext,
    clip: &AudioClip,
    opts: &SttOptions,
    pass: Pass,
    cont: &Continuation,
) -> Result<Transcript> {
    if clip.duration_ms() < MIN_AUDIO_MS {
        return Ok(Transcript::default());
    }
    let started = Instant::now();
    let mut state = ctx
        .create_state()
        .map_err(|e| VyrpError::Stt(e.to_string()))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads());
    params.set_language(language_for(opts));
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_audio_ctx(audio_ctx_for_model(ctx.model_n_mels(), clip.duration_ms()));
    if pass == Pass::WarmUp {
        // Só aloca buffers: um token, sem re-decodificação por temperatura (silêncio
        // faz o whisper entrar em laço e o fallback multiplica o custo).
        params.set_single_segment(true);
        params.set_max_tokens(1);
        params.set_temperature_inc(0.0);
    }
    let prompt = continuation_prompt(&opts.vocabulary, &cont.previous);
    if !prompt.is_empty() {
        params.set_initial_prompt(&prompt);
    }
    // `abort` vive até o fim desta função, depois do `state.full` que lê o ponteiro.
    // (Não usar `set_abort_callback_safe` do whisper-rs 0.16: o trampoline dele lê o
    // closure com o tipo errado e o encode falha com -6.)
    let abort = cont.abort.clone();
    if let Some(flag) = &abort {
        // SAFETY: função `extern "C"` compatível com `ggml_abort_callback`; o dado é um
        // `AtomicBool` mantido vivo por `abort` durante toda a decodificação.
        unsafe {
            params.set_abort_callback(Some(abort_requested));
            params.set_abort_callback_user_data(Arc::as_ptr(flag) as *mut std::ffi::c_void);
        }
    }
    let samples: Vec<f32> = clip
        .pcm
        .iter()
        .map(|&s| f32::from(s) / f32::from(i16::MAX))
        .collect();
    let decoded = state.full(params, &samples);
    // Abortado o whisper.cpp devolve erro genérico (-6): vale o pedido de cancelamento.
    if abort.as_ref().is_some_and(|a| a.load(Ordering::Relaxed)) {
        return Err(VyrpError::Cancelled);
    }
    decoded.map_err(|e| VyrpError::Stt(e.to_string()))?;

    let mut segments = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|e| VyrpError::Stt(e.to_string()))?
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        // Timestamps do whisper em centésimos de segundo.
        segments.push(TranscriptSegment {
            start_ms: segment.start_timestamp().max(0) as u64 * 10,
            end_ms: segment.end_timestamp().max(0) as u64 * 10,
            text,
        });
    }
    let text = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
    Ok(Transcript {
        text,
        language,
        segments,
        timings: SttTimings {
            load_ms: 0,
            decode_ms: started.elapsed().as_millis() as u64,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_joins_and_caps() {
        let vocab: Vec<String> = ["Vyrp", " ", "Tauri", "whisper.cpp"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(initial_prompt(&vocab), "Vyrp, Tauri, whisper.cpp");
        let many: Vec<String> = (0..500).map(|i| format!("termo{i}")).collect();
        assert!(initial_prompt(&many).len() <= MAX_PROMPT_CHARS);
        assert!(initial_prompt(&[]).is_empty());
    }

    #[test]
    fn continuation_prompt_ends_with_previous_text() {
        let vocab = vec!["Vyrp".to_string()];
        assert_eq!(continuation_prompt(&vocab, ""), "Vyrp");
        assert_eq!(
            continuation_prompt(&vocab, " Primeira frase. "),
            "Vyrp. Primeira frase."
        );
        assert_eq!(continuation_prompt(&[], "só texto"), "só texto");
        assert_eq!(continuation_prompt(&[], "eu acho... que"), "eu acho que");
        let long = "palavra ".repeat(100);
        let prompt = continuation_prompt(&[], &long);
        assert!(prompt.chars().count() <= MAX_PREVIOUS_CHARS);
        assert!(prompt.starts_with("palavra"), "corta em palavra inteira");
    }

    #[test]
    fn audio_ctx_scales_with_duration() {
        assert_eq!(audio_ctx_for(1_000), 50 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx_for(6_000), 300 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx_for(28_000), 0, "perto de 30 s usa o padrão");
        assert_eq!(audio_ctx_for(120_000), 0);
        assert_eq!(audio_ctx_for_model(80, 6_000), 300 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx_for_model(128, 6_000), 0, "v3: contexto cheio");
    }

    #[test]
    fn language_selection() {
        let with = |langs: &[&str]| SttOptions {
            languages: langs.iter().map(|s| s.to_string()).collect(),
            vocabulary: vec![],
        };
        assert_eq!(language_for(&with(&["pt"])), Some("pt"));
        assert_eq!(language_for(&with(&["pt-BR"])), Some("pt"));
        assert_eq!(language_for(&with(&[])), None);
        assert_eq!(
            language_for(&with(&["pt", "en"])),
            None,
            "vários = autodetecção"
        );
    }

    #[test]
    fn engine_without_model_reports_clearly() {
        let engine = WhisperEngine::new(|_| {});
        assert_eq!(engine.status().state, LoadState::None);
        let err = engine
            .transcribe_blocking(&AudioClip::new(vec![0; 16_000]), &SttOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("nenhum modelo"), "{err}");
    }
}
