//! Motor local whisper.cpp (whisper-rs): carregado em segundo plano no boot e mantido
//! quente (com uma decodificação de aquecimento), atrás do trait [`SttEngine`].

use std::path::PathBuf;
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
/// Contexto do encoder = 30 s de áudio (1500 quadros de 20 ms).
const FULL_AUDIO_CTX: i32 = 1500;
/// Folga sobre a duração real ao reduzir o contexto do encoder.
const AUDIO_CTX_MARGIN: i32 = 128;

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
    run_pass(&ctx, &warm, &SttOptions::default(), Pass::WarmUp).map_err(|e| e.to_string())?;
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

/// Contexto do encoder proporcional ao áudio (o whisper sempre codifica 30 s; para um
/// ditado de poucos segundos isso é a maior parte da latência). 0 = padrão (30 s).
pub fn audio_ctx_for(duration_ms: u64) -> i32 {
    let frames = (duration_ms as f64 / 30_000.0 * f64::from(FULL_AUDIO_CTX)).ceil() as i32;
    let ctx = frames + AUDIO_CTX_MARGIN;
    if ctx >= FULL_AUDIO_CTX { 0 } else { ctx }
}

/// Idioma fixo se houver exatamente um; senão autodetecção.
pub fn language_for(opts: &SttOptions) -> Option<&str> {
    match opts.languages.as_slice() {
        [only] => Some(only.split(['-', '_']).next().unwrap_or(only)),
        _ => None,
    }
}

fn run(ctx: &WhisperContext, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
    run_pass(ctx, clip, opts, Pass::Dictation)
}

fn run_pass(
    ctx: &WhisperContext,
    clip: &AudioClip,
    opts: &SttOptions,
    pass: Pass,
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
    params.set_audio_ctx(audio_ctx_for(clip.duration_ms()));
    if pass == Pass::WarmUp {
        // Só aloca buffers: um token, sem re-decodificação por temperatura (silêncio
        // faz o whisper entrar em laço e o fallback multiplica o custo).
        params.set_single_segment(true);
        params.set_max_tokens(1);
        params.set_temperature_inc(0.0);
    }
    let prompt = initial_prompt(&opts.vocabulary);
    if !prompt.is_empty() {
        params.set_initial_prompt(&prompt);
    }
    let samples: Vec<f32> = clip
        .pcm
        .iter()
        .map(|&s| f32::from(s) / f32::from(i16::MAX))
        .collect();
    state
        .full(params, &samples)
        .map_err(|e| VyrpError::Stt(e.to_string()))?;

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
    fn audio_ctx_scales_with_duration() {
        assert_eq!(audio_ctx_for(1_000), 50 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx_for(6_000), 300 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx_for(28_000), 0, "perto de 30 s usa o padrão");
        assert_eq!(audio_ctx_for(120_000), 0);
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
