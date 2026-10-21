//! Detecção de voz (V8): trechos de fala no [`AudioClip`], descarte de clipe sem fala e
//! compactação dos silêncios longos antes do STT (o whisper "ouve" música ou frases
//! inteiras em quase-silêncio; menos áudio também é menos latência).
//!
//! Motor principal: **Silero** pelo próprio whisper.cpp (modelo ggml de 885 KB, baixado
//! no primeiro uso com SHA-256 conferido), sem trazer outro runtime (ONNX). Reserva:
//! VAD de energia em Rust puro ([`energy`]) enquanto o Silero não está pronto.
//!
//! Uso: [`VadGated`] embrulha qualquer [`SttEngine`]; é o que a fábrica entrega ao V11.

pub mod energy;

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::core::types::{AudioClip, Result, SAMPLE_RATE, SttEngine, SttOptions, Transcript};
use crate::stt::models;

/// Modelo Silero v6.2.0 (ggml), repositório `ggml-org/whisper-vad` no Hugging Face;
/// tamanho e SHA-256 conferidos na API em 2026-10-20.
pub const SILERO_FILE: &str = "ggml-silero-v6.2.0.bin";
const SILERO_URL: &str =
    "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin";
const SILERO_SIZE: u64 = 885_098;
const SILERO_SHA256: &str = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987";

/// Milissegundos → amostras a 16 kHz.
const fn ms(millis: usize) -> usize {
    SAMPLE_RATE as usize * millis / 1000
}

/// Regras de suavização dos trechos de fala.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadConfig {
    /// Probabilidade mínima de fala (Silero).
    pub threshold: f32,
    /// Trechos mais curtos que isso são ruído (clique, tosse curta).
    pub min_speech_ms: usize,
    /// Pausas menores que isso não separam trechos.
    pub min_silence_ms: usize,
    /// Margem em volta de cada trecho (não cortar começo/fim de palavra).
    pub pad_ms: usize,
    /// Fala total abaixo disso = clipe sem fala.
    pub min_total_ms: usize,
    /// Silêncio deixado entre trechos ao compactar.
    pub joint_ms: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_speech_ms: 250,
            min_silence_ms: 300,
            pad_ms: 200,
            min_total_ms: 300,
            joint_ms: 250,
        }
    }
}

/// Junta trechos próximos, descarta os curtos e aplica a margem. Trechos em amostras.
pub fn smooth(mut raw: Vec<Range<usize>>, len: usize, cfg: &VadConfig) -> Vec<Range<usize>> {
    raw.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for seg in raw {
        match merged.last_mut() {
            Some(last) if seg.start <= last.end + ms(cfg.min_silence_ms) => {
                last.end = last.end.max(seg.end);
            }
            _ => merged.push(seg),
        }
    }
    let padded: Vec<Range<usize>> = merged
        .into_iter()
        .filter(|s| s.end - s.start >= ms(cfg.min_speech_ms))
        .map(|s| s.start.saturating_sub(ms(cfg.pad_ms))..(s.end + ms(cfg.pad_ms)).min(len))
        .collect();
    // A margem pode fazer trechos se encostarem: junta de novo.
    let mut out: Vec<Range<usize>> = Vec::new();
    for seg in padded {
        match out.last_mut() {
            Some(last) if seg.start <= last.end => last.end = last.end.max(seg.end),
            _ => out.push(seg),
        }
    }
    out
}

pub fn speech_ms(segments: &[Range<usize>]) -> usize {
    segments.iter().map(|s| s.end - s.start).sum::<usize>() * 1000 / SAMPLE_RATE as usize
}

/// Clipe só com os trechos de fala, separados por `joint_ms` de silêncio; os
/// `speech_segments` do resultado apontam para o áudio compactado.
pub fn compact(clip: &AudioClip, segments: &[Range<usize>], cfg: &VadConfig) -> AudioClip {
    let mut pcm = Vec::new();
    let mut kept = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            pcm.extend(std::iter::repeat_n(0i16, ms(cfg.joint_ms)));
        }
        let start = pcm.len();
        let end = seg.end.min(clip.pcm.len());
        pcm.extend_from_slice(&clip.pcm[seg.start.min(end)..end]);
        kept.push(start..pcm.len());
    }
    AudioClip {
        pcm,
        sample_rate: clip.sample_rate,
        speech_segments: kept,
    }
}

/// Detector de fala.
pub trait Vad: Send + Sync {
    fn name(&self) -> &'static str;
    fn segments(&self, pcm: &[i16]) -> std::result::Result<Vec<Range<usize>>, String>;
}

/// Silero pelo whisper.cpp. O contexto exige `&mut`: um ditado por vez.
pub struct SileroVad {
    ctx: Mutex<WhisperVadContext>,
    cfg: VadConfig,
}

impl SileroVad {
    pub fn load(path: &std::path::Path, cfg: VadConfig) -> std::result::Result<Self, String> {
        let mut params = WhisperVadContextParams::default();
        params.set_n_threads(2);
        params.set_use_gpu(false);
        let ctx =
            WhisperVadContext::new(&path.to_string_lossy(), params).map_err(|e| e.to_string())?;
        Ok(Self {
            ctx: Mutex::new(ctx),
            cfg,
        })
    }
}

impl Vad for SileroVad {
    fn name(&self) -> &'static str {
        "silero"
    }

    fn segments(&self, pcm: &[i16]) -> std::result::Result<Vec<Range<usize>>, String> {
        let samples: Vec<f32> = pcm
            .iter()
            .map(|&s| f32::from(s) / f32::from(i16::MAX))
            .collect();
        let mut params = WhisperVadParams::new();
        params.set_threshold(self.cfg.threshold);
        // A suavização é nossa (igual para Silero e energia): pedimos o bruto ao whisper.cpp.
        params.set_min_speech_duration(0);
        params.set_min_silence_duration(0);
        params.set_speech_pad(0);
        let mut ctx = self.ctx.lock().unwrap();
        let found = ctx
            .segments_from_samples(params, &samples)
            .map_err(|e| e.to_string())?;
        // Timestamps do whisper.cpp em centésimos de segundo.
        let to_index = |cs: f32| ((cs / 100.0) * SAMPLE_RATE as f32).max(0.0) as usize;
        let raw = found
            .map(|s| to_index(s.start).min(pcm.len())..to_index(s.end).min(pcm.len()))
            .filter(|r| r.end > r.start)
            .collect();
        Ok(smooth(raw, pcm.len(), &self.cfg))
    }
}

/// VAD de energia como [`Vad`].
pub struct EnergyVad(pub VadConfig);

impl Vad for EnergyVad {
    fn name(&self) -> &'static str {
        "energia"
    }

    fn segments(&self, pcm: &[i16]) -> std::result::Result<Vec<Range<usize>>, String> {
        Ok(energy::segments(pcm, &self.0))
    }
}

/// Silero quando carregado; energia até lá (ou se o Silero falhar).
pub struct VadService {
    silero: RwLock<Option<Arc<SileroVad>>>,
    fallback: EnergyVad,
}

impl VadService {
    pub fn new(cfg: VadConfig) -> Arc<Self> {
        Arc::new(Self {
            silero: RwLock::new(None),
            fallback: EnergyVad(cfg),
        })
    }

    pub fn config(&self) -> VadConfig {
        self.fallback.0
    }

    pub fn set_silero(&self, vad: SileroVad) {
        *self.silero.write().unwrap() = Some(Arc::new(vad));
    }

    /// Trechos de fala e o nome do detector usado.
    pub fn detect(&self, pcm: &[i16]) -> (Vec<Range<usize>>, &'static str) {
        let silero = self.silero.read().unwrap().clone();
        if let Some(silero) = silero {
            match silero.segments(pcm) {
                Ok(segs) => return (segs, silero.name()),
                Err(err) => tracing::warn!(%err, "Silero falhou; usando o VAD de energia"),
            }
        }
        let segs = self.fallback.segments(pcm).unwrap_or_default();
        (segs, self.fallback.name())
    }
}

/// Resultado da triagem de um clipe.
#[derive(Debug, Clone, PartialEq)]
pub enum Screened {
    /// Sem fala suficiente: não transcrever.
    NoSpeech { speech_ms: usize },
    /// Clipe compactado (só fala), com `speech_segments`.
    Speech(AudioClip),
}

pub fn screen(vad: &VadService, clip: &AudioClip) -> (Screened, &'static str) {
    let (segments, engine) = vad.detect(&clip.pcm);
    let total = speech_ms(&segments);
    if total < vad.config().min_total_ms {
        return (Screened::NoSpeech { speech_ms: total }, engine);
    }
    (
        Screened::Speech(compact(clip, &segments, &vad.config())),
        engine,
    )
}

/// [`SttEngine`] com o VAD na frente: clipe sem fala não chega ao STT (transcrição vazia).
pub struct VadGated {
    vad: Arc<VadService>,
    inner: Arc<dyn SttEngine>,
}

impl VadGated {
    pub fn new(vad: Arc<VadService>, inner: Arc<dyn SttEngine>) -> Self {
        Self { vad, inner }
    }
}

#[async_trait]
impl SttEngine for VadGated {
    async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
        let started = std::time::Instant::now();
        let (screened, engine) = screen(&self.vad, clip);
        let vad_ms = started.elapsed().as_millis() as u64;
        match screened {
            Screened::NoSpeech { speech_ms } => {
                tracing::info!(
                    vad = engine,
                    vad_ms,
                    audio_ms = clip.duration_ms(),
                    speech_ms,
                    "VAD: clipe sem fala, descartado (sem STT)"
                );
                Ok(Transcript::default())
            }
            Screened::Speech(compacted) => {
                tracing::info!(
                    vad = engine,
                    vad_ms,
                    audio_ms = clip.duration_ms(),
                    kept_ms = compacted.duration_ms(),
                    segments = compacted.speech_segments.len(),
                    "VAD: fala detectada"
                );
                self.inner.transcribe(&compacted, opts).await
            }
        }
    }
}

/// Registra o [`VadService`] e prepara o Silero (baixa na primeira vez; ~885 KB).
pub fn init(app: &AppHandle) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let service = VadService::new(VadConfig::default());
    app.manage(Arc::clone(&service));
    let path: PathBuf = app.path().app_data_dir()?.join("models").join(SILERO_FILE);
    tauri::async_runtime::spawn(async move {
        let installed = std::fs::metadata(&path).is_ok_and(|m| m.len() == SILERO_SIZE);
        if !installed {
            tracing::info!("baixando o modelo do VAD (Silero)");
            if let Err(err) =
                models::download_to(SILERO_URL, &path, SILERO_SIZE, SILERO_SHA256, |_, _| {}).await
            {
                tracing::warn!(%err, "VAD Silero indisponível; usando o VAD de energia");
                return;
            }
        }
        match SileroVad::load(&path, service.config()) {
            Ok(vad) => {
                service.set_silero(vad);
                tracing::info!("VAD Silero pronto");
            }
            Err(err) => tracing::warn!(%err, "falha ao carregar o Silero; usando o VAD de energia"),
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn r(a_ms: usize, b_ms: usize) -> Range<usize> {
        ms(a_ms)..ms(b_ms)
    }

    #[test]
    fn smooth_merges_drops_and_pads() {
        let cfg = VadConfig::default();
        let len = ms(10_000);
        // 1–2 s e 2,1–2,5 s se juntam (pausa de 100 ms); 5–5,1 s é curto demais.
        let out = smooth(
            vec![r(2_100, 2_500), r(1_000, 2_000), r(5_000, 5_100)],
            len,
            &cfg,
        );
        assert_eq!(out, vec![r(800, 2_700)]);
        // A margem não passa das bordas.
        assert_eq!(smooth(vec![r(0, 400)], ms(500), &cfg), vec![0..ms(500)]);
    }

    #[test]
    fn compact_keeps_only_speech_with_joints() {
        let cfg = VadConfig::default();
        let clip = AudioClip::new((0..ms(4_000)).map(|i| (i % 100) as i16 + 1).collect());
        let segs = vec![r(500, 1_000), r(3_000, 3_500)];
        let out = compact(&clip, &segs, &cfg);
        assert_eq!(out.pcm.len(), ms(500) + ms(cfg.joint_ms) + ms(500));
        assert_eq!(out.speech_segments, vec![0..ms(500), ms(750)..ms(1_250)]);
        assert_eq!(out.pcm[0], clip.pcm[ms(500)]);
        assert!(out.pcm[ms(500)..ms(750)].iter().all(|&s| s == 0));
    }

    struct Counting(AtomicUsize);

    #[async_trait]
    impl SttEngine for Counting {
        async fn transcribe(&self, clip: &AudioClip, _: &SttOptions) -> Result<Transcript> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Transcript {
                text: format!("{} ms", clip.duration_ms()),
                ..Transcript::default()
            })
        }
    }

    #[test]
    fn gate_skips_stt_without_speech_and_compacts_with_speech() {
        let inner = Arc::new(Counting(AtomicUsize::new(0)));
        let gated = VadGated::new(VadService::new(VadConfig::default()), inner.clone());
        let opts = SttOptions::default();

        let silence = AudioClip::new(vec![3; ms(5_000)]);
        let out = tauri::async_runtime::block_on(gated.transcribe(&silence, &opts)).unwrap();
        assert_eq!(out.text, "");
        assert_eq!(inner.0.load(Ordering::SeqCst), 0, "silêncio não vai ao STT");

        let mut speech = vec![3i16; ms(2_000)];
        speech.extend((0..ms(1_000)).map(|i| ((i as f32 * 0.12).sin() * 9_000.0) as i16));
        speech.extend(vec![3i16; ms(3_000)]);
        let out = tauri::async_runtime::block_on(gated.transcribe(&AudioClip::new(speech), &opts))
            .unwrap();
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
        let kept: u64 = out.text.trim_end_matches(" ms").parse().unwrap();
        assert!((1_000..=1_500).contains(&kept), "compactado para {kept} ms");
    }
}
