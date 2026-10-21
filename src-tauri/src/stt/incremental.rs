//! Decodificação incremental (V9b, spec §5.1): enquanto o usuário fala, cada trecho
//! fechado por uma pausa do VAD é decodificado em segundo plano pelo modelo quente; ao
//! soltar, só o resto vai ao whisper e os textos são juntados. O tempo soltar→texto fica
//! perto de constante em ditados longos.
//!
//! Só STT local: a nuvem segue com o clipe inteiro. O áudio chega pelo tap da gravação
//! ([`crate::audio::AudioService::begin_tapped`]), na mesma ordem do clipe final, então
//! "o resto" é simplesmente `clip.pcm[committed..]`.
//!
//! Qualquer falha em segundo plano não perde nada: [`Incremental::finish`] devolve `None`
//! e o ditado volta ao caminho normal (clipe inteiro).

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::audio::PcmTap;
use crate::core::types::{
    AudioClip, Result, SAMPLE_RATE, SttEngine, SttOptions, Transcript, TranscriptSegment, VyrpError,
};
use crate::stt::whisper::{Continuation, WhisperEngine};
use crate::vad::{VadService, compact, speech_ms};

/// Trecho mínimo para decodificar sozinho (menos que isso o whisper perde contexto).
const MIN_CHUNK_MS: usize = 4_000;
/// Silêncio no fim do trecho para considerá-lo fechado (além da margem do VAD).
const TRAILING_SILENCE_MS: usize = 300;
/// De quanto em quanto áudio novo procurar uma pausa.
const CHECK_EVERY_MS: usize = 500;
/// O VAD olha também esse tanto antes do trecho (a pausa do corte): o VAD de energia
/// mede o ruído de fundo pelo próprio trecho e erraria num trecho quase só de fala.
const LOOKBACK_MS: usize = 1_000;
/// Até quantas palavras repetidas na junção são removidas.
const MAX_OVERLAP_WORDS: usize = 4;

const fn samples(ms: usize) -> usize {
    SAMPLE_RATE as usize * ms / 1000
}

/// Decodificador de um segmento (o whisper; nos testes, um falso).
pub trait SegmentDecoder: Send + Sync {
    fn decode(
        &self,
        clip: &AudioClip,
        opts: &SttOptions,
        cont: &Continuation,
    ) -> Result<Transcript>;
}

impl SegmentDecoder for WhisperEngine {
    fn decode(
        &self,
        clip: &AudioClip,
        opts: &SttOptions,
        cont: &Continuation,
    ) -> Result<Transcript> {
        self.transcribe_continuing(clip, opts, cont)
    }
}

/// Onde cortar o áudio ainda não decodificado (`len` amostras, trechos de fala do VAD já
/// suavizados): no meio da ÚLTIMA pausa que deixa ao menos `min_chunk` antes dela.
/// `None` = ainda não há trecho fechado.
pub fn find_cut(segments: &[Range<usize>], len: usize, min_chunk: usize) -> Option<usize> {
    let mut cuts: Vec<usize> = segments
        .windows(2)
        .map(|w| (w[0].end + w[1].start) / 2)
        .collect();
    // Pausa em curso depois da última fala (o usuário parou agora).
    if let Some(last) = segments.last()
        && last.end + samples(TRAILING_SILENCE_MS) <= len
    {
        cuts.push(last.end);
    }
    cuts.into_iter().rev().find(|&cut| cut >= min_chunk)
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect()
}

/// Junta dois textos de segmentos seguidos sem repetir palavras da junção (o whisper às
/// vezes repete o fim do prompt no começo do segmento novo).
pub fn join_texts(previous: &str, next: &str) -> String {
    let (previous, next) = (previous.trim(), next.trim());
    if previous.is_empty() {
        return next.to_string();
    }
    if next.is_empty() {
        return previous.to_string();
    }
    let (a, b) = (words(previous), words(next));
    let overlap = (1..=MAX_OVERLAP_WORDS.min(a.len()).min(b.len()))
        .rev()
        .find(|&n| a[a.len() - n..] == b[..n] && b[..n].iter().any(|w| !w.is_empty()))
        .unwrap_or(0);
    let rest: Vec<&str> = next.split_whitespace().skip(overlap).collect();
    if rest.is_empty() {
        return previous.to_string();
    }
    format!("{previous} {}", rest.join(" "))
}

/// Trechos de fala de `pcm[from..]`, relativos a `from`.
fn detect_from(vad: &VadService, pcm: &[i16], from: usize) -> Vec<Range<usize>> {
    let start = from.saturating_sub(samples(LOOKBACK_MS));
    let shift = from - start;
    vad.detect(&pcm[start..])
        .0
        .into_iter()
        .filter_map(|r| {
            let begin = r.start.max(shift);
            (r.end > begin).then(|| begin - shift..r.end - shift)
        })
        .collect()
}

/// Só a fala de `pcm` (trechos `speech`, até `end`), ou `None` se for pouca.
fn speech_clip(
    vad: &VadService,
    pcm: &[i16],
    speech: &[Range<usize>],
    end: usize,
) -> Option<AudioClip> {
    let kept: Vec<Range<usize>> = speech
        .iter()
        .filter(|r| r.start < end)
        .map(|r| r.start..r.end.min(end))
        .collect();
    if speech_ms(&kept) < vad.config().min_total_ms {
        return None;
    }
    Some(compact(
        &AudioClip::new(pcm[..end].to_vec()),
        &kept,
        &vad.config(),
    ))
}

#[derive(Default)]
struct Progress {
    /// Amostras do começo do clipe já decodificadas.
    committed: usize,
    text: String,
    language: Option<String>,
    segments: Vec<TranscriptSegment>,
    decode_ms: u64,
    chunks: usize,
    /// Algo deu errado em segundo plano: o fim usa o clipe inteiro.
    failed: bool,
}

struct Shared {
    progress: Mutex<Progress>,
    /// Soltou: termina o trecho em curso e não começa outro.
    stop: AtomicBool,
    /// Cancelou: interrompe até o whisper.
    abort: Arc<AtomicBool>,
}

/// Sessão incremental de um ditado. Soltar → [`finish`](Self::finish); cancelar ou
/// descartar (drop) → interrompe tudo.
pub struct Incremental {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    decoder: Arc<dyn SegmentDecoder>,
    vad: Arc<VadService>,
    opts: SttOptions,
}

impl Incremental {
    /// Começa a sessão; o [`PcmTap`] devolvido vai para a gravação.
    pub fn start(
        decoder: Arc<dyn SegmentDecoder>,
        vad: Arc<VadService>,
        opts: SttOptions,
    ) -> (Self, PcmTap) {
        let (tap, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            progress: Mutex::new(Progress::default()),
            stop: AtomicBool::new(false),
            abort: Arc::new(AtomicBool::new(false)),
        });
        let worker = {
            let (shared, decoder, vad, opts) = (
                Arc::clone(&shared),
                Arc::clone(&decoder),
                Arc::clone(&vad),
                opts.clone(),
            );
            std::thread::Builder::new()
                .name("vyrp-stt-incremental".into())
                .spawn(move || background(&rx, &shared, decoder.as_ref(), &vad, &opts))
                .expect("thread da decodificação incremental")
        };
        (
            Self {
                shared,
                worker: Some(worker),
                decoder,
                vad,
                opts,
            },
            tap,
        )
    }

    /// Soltou: espera o trecho em curso, decodifica só o resto de `clip` e junta.
    /// `None` = nada foi adiantado (ou falhou): use o caminho normal com o clipe inteiro.
    pub fn finish(mut self, clip: &AudioClip) -> Option<Result<Transcript>> {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let progress = std::mem::take(&mut *self.shared.progress.lock().unwrap());
        if progress.failed || progress.committed == 0 || progress.committed > clip.pcm.len() {
            return None;
        }
        let started = Instant::now();
        let rest = AudioClip::new(clip.pcm[progress.committed..].to_vec());
        let speech = detect_from(&self.vad, &clip.pcm, progress.committed);
        let tail = match speech_clip(&self.vad, &rest.pcm, &speech, rest.pcm.len()) {
            None => Transcript::default(),
            Some(compacted) => {
                let cont = Continuation {
                    previous: progress.text.clone(),
                    abort: Some(Arc::clone(&self.shared.abort)),
                };
                match self.decoder.decode(&compacted, &self.opts, &cont) {
                    Ok(t) => t,
                    Err(err) => return Some(Err(err)),
                }
            }
        };
        let offset = (progress.committed * 1000 / SAMPLE_RATE as usize) as u64;
        let mut segments = progress.segments;
        segments.extend(tail.segments.into_iter().map(|s| TranscriptSegment {
            start_ms: s.start_ms + offset,
            end_ms: s.end_ms + offset,
            text: s.text,
        }));
        let tail_ms = started.elapsed().as_millis() as u64;
        tracing::info!(
            chunks = progress.chunks,
            ahead_ms = offset,
            rest_ms = rest.duration_ms(),
            background_decode_ms = progress.decode_ms,
            tail_decode_ms = tail_ms,
            "STT incremental: juntado"
        );
        Some(Ok(Transcript {
            text: join_texts(&progress.text, &tail.text),
            language: progress.language.or(tail.language),
            segments,
            timings: crate::core::types::SttTimings {
                load_ms: 0,
                decode_ms: progress.decode_ms + tail_ms,
            },
        }))
    }

    /// Cancelou: para a thread e o whisper sem esperar.
    pub fn cancel(&mut self) {
        self.shared.abort.store(true, Ordering::SeqCst);
        self.shared.stop.store(true, Ordering::SeqCst);
        self.worker.take(); // a thread sai sozinha (abort/stop); não bloqueia o gesto
    }
}

impl Drop for Incremental {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.cancel();
        }
    }
}

fn background(
    rx: &Receiver<Vec<i16>>,
    shared: &Shared,
    decoder: &dyn SegmentDecoder,
    vad: &VadService,
    opts: &SttOptions,
) {
    let mut pcm: Vec<i16> = Vec::new();
    let mut checked = 0usize;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let idle = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(block) => {
                pcm.extend_from_slice(&block);
                // Junta o que já está na fila antes de olhar o VAD.
                for more in rx.try_iter() {
                    pcm.extend_from_slice(&more);
                }
                false
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => return, // gravação acabou/cancelou
        };
        // Olha a cada meio segundo de áudio novo, ou quando o áudio parou de chegar.
        if pcm.len() < checked + samples(CHECK_EVERY_MS) && !(idle && pcm.len() > checked) {
            continue;
        }
        checked = pcm.len();
        let committed = shared.progress.lock().unwrap().committed;
        let region = &pcm[committed..];
        if region.len() < samples(MIN_CHUNK_MS) {
            continue;
        }
        let speech = detect_from(vad, &pcm, committed);
        let Some(cut) = find_cut(&speech, region.len(), samples(MIN_CHUNK_MS)) else {
            continue;
        };
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let voiced = speech_clip(vad, region, &speech, cut);
        if !decode_chunk(shared, decoder, opts, voiced, cut, committed) {
            return;
        }
    }
}

/// Decodifica um trecho fechado e avança o progresso. `false` = parar a thread.
fn decode_chunk(
    shared: &Shared,
    decoder: &dyn SegmentDecoder,
    opts: &SttOptions,
    voiced: Option<AudioClip>,
    len: usize,
    committed: usize,
) -> bool {
    let started = Instant::now();
    let previous = shared.progress.lock().unwrap().text.clone();
    let transcript = match voiced {
        None => Ok(Transcript::default()),
        Some(compacted) => decoder.decode(
            &compacted,
            opts,
            &Continuation {
                previous,
                abort: Some(Arc::clone(&shared.abort)),
            },
        ),
    };
    let mut progress = shared.progress.lock().unwrap();
    match transcript {
        Ok(t) => {
            let offset = (committed * 1000 / SAMPLE_RATE as usize) as u64;
            progress.text = join_texts(&progress.text, &t.text);
            progress.language = progress.language.take().or(t.language);
            progress
                .segments
                .extend(t.segments.into_iter().map(|s| TranscriptSegment {
                    start_ms: s.start_ms + offset,
                    end_ms: s.end_ms + offset,
                    text: s.text,
                }));
            progress.committed = committed + len;
            progress.decode_ms += started.elapsed().as_millis() as u64;
            progress.chunks += 1;
            tracing::debug!(
                chunk_ms = len * 1000 / SAMPLE_RATE as usize,
                decode_ms = started.elapsed().as_millis() as u64,
                "STT incremental: trecho decodificado"
            );
            true
        }
        Err(VyrpError::Cancelled) => false,
        Err(err) => {
            tracing::warn!(%err, "STT incremental: trecho falhou; o fim usa o clipe inteiro");
            progress.failed = true;
            false
        }
    }
}

/// [`SttEngine`] do ditado com sessão incremental: no `transcribe`, usa o que já foi
/// adiantado; sem nada adiantado, delega ao motor normal (VAD + política) com o clipe todo.
pub struct IncrementalStt {
    session: Mutex<Option<Incremental>>,
    fallback: Arc<dyn SttEngine>,
}

impl IncrementalStt {
    pub fn new(session: Incremental, fallback: Arc<dyn SttEngine>) -> Self {
        Self {
            session: Mutex::new(Some(session)),
            fallback,
        }
    }
}

#[async_trait]
impl SttEngine for IncrementalStt {
    async fn transcribe(&self, clip: &AudioClip, opts: &SttOptions) -> Result<Transcript> {
        let session = self.session.lock().unwrap().take();
        if let Some(session) = session {
            let owned = clip.clone();
            let done = tauri::async_runtime::spawn_blocking(move || session.finish(&owned))
                .await
                .map_err(|e| VyrpError::Stt(format!("tarefa de transcrição falhou: {e}")))?;
            if let Some(result) = done {
                return result;
            }
        }
        self.fallback.transcribe(clip, opts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::VadConfig;

    #[test]
    fn cut_goes_in_the_last_pause_after_the_minimum() {
        let s = samples;
        let speech = vec![0..s(2_000), s(2_600)..s(5_000), s(5_800)..s(9_000)];
        // Pausas em 2,3 s e 5,4 s; a última que deixa ≥ 4 s antes dela é a de 5,4 s.
        assert_eq!(find_cut(&speech, s(9_000), s(4_000)), Some(s(5_400)));
        // Ainda falando no fim e só a pausa de 2,3 s: curta demais.
        assert_eq!(find_cut(&speech[..2], s(5_000), s(4_000)), None);
        // Parou de falar agora (silêncio depois da última fala).
        assert_eq!(find_cut(&speech, s(9_400), s(4_000)), Some(s(9_000)));
        assert_eq!(find_cut(&[], s(9_000), s(4_000)), None);
    }

    #[test]
    fn join_removes_repeated_words_at_the_seam() {
        assert_eq!(
            join_texts("Primeiro trecho do ditado.", "Do ditado. Segundo trecho"),
            "Primeiro trecho do ditado. Segundo trecho"
        );
        assert_eq!(join_texts("Oi.", "Tudo bem?"), "Oi. Tudo bem?");
        assert_eq!(join_texts("", " só o fim "), "só o fim");
        assert_eq!(join_texts("só o começo", ""), "só o começo");
        assert_eq!(join_texts("igual", "igual"), "igual");
    }

    /// Falso: devolve "s<início>" por trecho e anota o prompt recebido.
    struct Fake {
        calls: Mutex<Vec<(u64, String)>>,
    }
    impl SegmentDecoder for Fake {
        fn decode(
            &self,
            clip: &AudioClip,
            _: &SttOptions,
            cont: &Continuation,
        ) -> Result<Transcript> {
            let n = self.calls.lock().unwrap().len();
            self.calls
                .lock()
                .unwrap()
                .push((clip.duration_ms(), cont.previous.clone()));
            Ok(Transcript {
                text: format!("trecho{n}."),
                ..Transcript::default()
            })
        }
    }

    /// Fala (ruído alto) e silêncio, em ms.
    fn audio(parts: &[(bool, usize)]) -> Vec<i16> {
        let mut pcm = Vec::new();
        for &(speech, ms) in parts {
            for i in 0..samples(ms) {
                pcm.push(if speech {
                    if (i / 8) % 2 == 0 { 9_000 } else { -9_000 }
                } else {
                    0
                });
            }
        }
        pcm
    }

    fn feed(tap: &PcmTap, pcm: &[i16]) {
        for block in pcm.chunks(samples(100)) {
            tap.send(block.to_vec()).unwrap();
        }
    }

    #[test]
    fn decodes_closed_stretches_ahead_and_only_the_rest_at_the_end() {
        let fake = Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
        });
        let vad = VadService::new(VadConfig::default());
        let (session, tap) = Incremental::start(fake.clone(), vad, SttOptions::default());
        let pcm = audio(&[(true, 5_000), (false, 1_000), (true, 3_000)]);
        feed(&tap, &pcm[..samples(6_000)]);
        // Espera o trecho fechado ser decodificado em segundo plano.
        let started = Instant::now();
        while fake.calls.lock().unwrap().is_empty() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fake.calls.lock().unwrap().len(), 1, "um trecho adiantado");
        feed(&tap, &pcm[samples(6_000)..]);
        drop(tap);
        let out = session.finish(&AudioClip::new(pcm)).unwrap().unwrap();
        assert_eq!(out.text, "trecho0. trecho1.");
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1].1, "trecho0.",
            "o resto recebe o texto anterior no prompt"
        );
        assert!(calls[1].0 < 4_500, "no fim só o resto: {} ms", calls[1].0);
    }

    #[test]
    fn short_dictation_falls_back_to_the_whole_clip() {
        let fake = Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
        });
        let vad = VadService::new(VadConfig::default());
        let (session, tap) = Incremental::start(fake.clone(), vad, SttOptions::default());
        let pcm = audio(&[(true, 2_000)]);
        feed(&tap, &pcm);
        drop(tap);
        assert!(session.finish(&AudioClip::new(pcm)).is_none());
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn cancel_stops_the_background_thread() {
        let fake = Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
        });
        let vad = VadService::new(VadConfig::default());
        let (mut session, tap) = Incremental::start(fake.clone(), vad, SttOptions::default());
        session.cancel();
        let pcm = audio(&[(true, 5_000), (false, 1_000)]);
        // Depois do cancelamento a thread sai; o tap pode até falhar.
        for block in pcm.chunks(samples(100)) {
            let _ = tap.send(block.to_vec());
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            fake.calls.lock().unwrap().is_empty(),
            "nada decodificado depois do Cancel"
        );
        assert!(session.shared.abort.load(Ordering::SeqCst));
    }
}
