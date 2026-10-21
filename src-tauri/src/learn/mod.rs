//! Auto-aprendizado do dicionário (V24): depois de uma colagem VERIFICADA, observa o mesmo
//! campo (mesma janela e mesmo elemento de UIA) por até 15 s e, se o usuário corrigiu uma
//! palavra, propõe (ou grava, no modo automático) o termo certo como vocabulário, com o
//! erro do ASR em `observed_asr_error` e origem `learned`.
//!
//! Privacidade: o texto do campo só existe em memória durante a observação; nada do
//! contexto é gravado nem logado. Campo de senha encerra a observação na hora.
//!
//! Contrato com o hub (Lima): Settings `dictionary_learning` = `off` | `suggest` | `auto`;
//! evento `dictionary://learned` ([`LearnedEvent`]); commands `learn_suggestions`,
//! `learn_accept { id }`, `learn_dismiss { id }`, `learn_undo { entryId }`.

pub mod diff;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::ui_error::UiError;
use crate::post::PostProcessor;
use crate::storage::dictionary::{DictionaryEntry, NewDictionaryEntry};
use crate::storage::{Db, now_ms};
pub use diff::Learned;

pub const EVENT_LEARNED: &str = "dictionary://learned";
pub const ORIGIN: &str = "learned";
/// Janela de observação depois da colagem.
const WATCH_FOR: Duration = Duration::from_secs(15);
const FIRST_LOOK: Duration = Duration::from_millis(800);
const POLL_EVERY: Duration = Duration::from_millis(700);
/// Quanto do texto em volta da colagem serve de âncora.
const ANCHOR_CHARS: usize = 24;
const MAX_PENDING: usize = 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnMode {
    Off,
    /// Mostra no hub para o usuário aceitar.
    #[default]
    Suggest,
    /// Grava direto (com desfazer).
    Auto,
}

/// Onde e o que foi colado (só em memória, até a observação acabar).
#[derive(Debug, Clone)]
pub(crate) struct PasteMark {
    pub hwnd: isize,
    pub focus_id: Vec<i32>,
    pub prefix: String,
    pub suffix: String,
    pub pasted: String,
}

static LAST_PASTE: Mutex<Option<PasteMark>> = Mutex::new(None);
/// Muda a cada ditado novo: uma observação antiga para (o ditado novo é prioridade).
static EPOCH: AtomicU64 = AtomicU64::new(0);

fn tail(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Chamado pela inserção (V15) depois de uma colagem confirmada por UIA.
pub(crate) fn remember_paste(
    hwnd: isize,
    focus_id: Vec<i32>,
    before: &str,
    after: &str,
    pasted: &str,
) {
    *LAST_PASTE.lock().unwrap() = Some(PasteMark {
        hwnd,
        focus_id,
        prefix: tail(before, ANCHOR_CHARS),
        suffix: head(after, ANCHOR_CHARS),
        pasted: pasted.to_string(),
    });
}

/// Descarta a marca da última colagem (aprendizado desligado, ou já usada).
pub fn forget() {
    LAST_PASTE.lock().unwrap().take();
}

/// Um ditado novo começou: observações em andamento param.
pub fn new_dictation() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

fn take_mark(pasted: &str) -> Option<PasteMark> {
    let mark = LAST_PASTE.lock().unwrap().take()?;
    (mark.pasted == pasted).then_some(mark)
}

/// Como a observação terminou.
#[derive(Debug, Clone, PartialEq)]
pub enum Watch {
    /// Último texto visto no trecho colado (pode ser igual ao colado).
    Edited(String),
    /// Campo de senha, trecho perdido ou ditado novo: nada a aprender.
    Abandoned(&'static str),
}

/// Observa o campo da colagem. Bloqueia até 15 s: rodar fora da UI e da thread de gestos.
pub(crate) fn watch(mark: &PasteMark, timeout: Duration) -> Watch {
    let epoch = EPOCH.load(Ordering::SeqCst);
    let started = Instant::now();
    let mut last: Option<String> = None;
    std::thread::sleep(FIRST_LOOK);
    while started.elapsed() < timeout {
        if EPOCH.load(Ordering::SeqCst) != epoch {
            return Watch::Abandoned("novo ditado");
        }
        let snap = crate::context::snapshot_target(mark.hwnd, None);
        if snap.password || snap.context.password {
            return Watch::Abandoned("campo de senha");
        }
        let same_field = snap.hwnd == mark.hwnd
            && !mark.focus_id.is_empty()
            && snap.focus_id == mark.focus_id
            && snap.text_range;
        if !same_field {
            break; // perdeu o foco: vale o que já foi visto
        }
        let field = format!(
            "{}{}{}",
            snap.context.before, snap.context.selection, snap.context.after
        );
        match diff::locate(&field, &mark.prefix, &mark.suffix) {
            Some(region) => last = Some(region.to_string()),
            None if last.is_none() => return Watch::Abandoned("trecho colado não encontrado"),
            None => {}
        }
        std::thread::sleep(POLL_EVERY);
    }
    match last {
        Some(text) => Watch::Edited(text),
        None => Watch::Abandoned("nada observado"),
    }
}

/// Diagnóstico (diag-desktop `learn`): observa a última colagem como o app faria e
/// devolve as correções, sem gravar nada. `Err` = observação abandonada.
pub fn diag_watch(pasted: &str, timeout: Duration) -> Result<Vec<Learned>, String> {
    let mark = take_mark(pasted).ok_or("nenhuma colagem verificada para observar")?;
    match watch(&mark, timeout) {
        Watch::Edited(text) => Ok(diff::candidates(&mark.pasted, &text, &|_| false)),
        Watch::Abandoned(why) => Err(why.into()),
    }
}

/// Sugestão pendente (modo `suggest`), só em memória.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub id: u64,
    pub phrase: String,
    pub observed: String,
}

/// Payload de `dictionary://learned`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedEvent {
    pub mode: LearnMode,
    /// Modo `suggest`: id da sugestão; modo `auto`: id da entrada criada (para desfazer).
    pub id: i64,
    pub phrase: String,
    pub observed: String,
}

#[derive(Default)]
pub struct Learning {
    pending: Mutex<VecDeque<Suggestion>>,
    next_id: AtomicU64,
}

impl Learning {
    fn push(&self, learned: &Learned) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|s| !s.phrase.eq_ignore_ascii_case(&learned.phrase));
        pending.push_back(Suggestion {
            id,
            phrase: learned.phrase.clone(),
            observed: learned.observed.clone(),
        });
        while pending.len() > MAX_PENDING {
            pending.pop_front();
        }
        id
    }

    fn take(&self, id: u64) -> Option<Suggestion> {
        let mut pending = self.pending.lock().unwrap();
        let at = pending.iter().position(|s| s.id == id)?;
        pending.remove(at)
    }
}

pub fn init(app: &AppHandle) {
    app.manage(Learning::default());
}

fn save(db: &Db, learned: &Learned) -> crate::storage::Result<DictionaryEntry> {
    db.dictionary().upsert(
        &NewDictionaryEntry {
            phrase: learned.phrase.clone(),
            manual_entry: false,
            origin: Some(ORIGIN.into()),
            observed_asr_error: Some(learned.observed.clone()),
            ..Default::default()
        },
        now_ms(),
    )
}

/// Depois de uma colagem verificada: observa em segundo plano e aprende conforme `mode`.
pub fn start(app: AppHandle, pasted: String, mode: LearnMode) {
    if mode == LearnMode::Off {
        forget();
        return;
    }
    let Some(mark) = take_mark(&pasted) else {
        return;
    };
    std::thread::Builder::new()
        .name("vyrp-learn".into())
        .spawn(move || {
            let edited = match watch(&mark, WATCH_FOR) {
                Watch::Edited(text) => text,
                Watch::Abandoned(why) => {
                    tracing::debug!(why, "aprendizado: observação encerrada sem resultado");
                    return;
                }
            };
            let Some(db) = app.try_state::<Db>() else {
                return;
            };
            let known: Vec<String> = db
                .dictionary()
                .list()
                .map(|l| l.into_iter().map(|e| e.phrase.to_lowercase()).collect())
                .unwrap_or_default();
            let found = diff::candidates(&mark.pasted, &edited, &|t: &str| {
                known.contains(&t.to_lowercase())
            });
            tracing::info!(
                count = found.len(),
                ?mode,
                "aprendizado: correções encontradas"
            );
            apply(&app, &db, &found, mode);
        })
        .expect("thread de aprendizado");
}

fn apply(app: &AppHandle, db: &Db, found: &[Learned], mode: LearnMode) {
    let mut saved_any = false;
    for learned in found {
        let id = match mode {
            LearnMode::Auto => match save(db, learned) {
                Ok(entry) => {
                    saved_any = true;
                    entry.id
                }
                Err(err) => {
                    tracing::warn!(%err, "aprendizado: termo não gravado");
                    continue;
                }
            },
            LearnMode::Suggest => match app.try_state::<Learning>() {
                Some(state) => state.push(learned) as i64,
                None => continue,
            },
            LearnMode::Off => return,
        };
        let _ = app.emit(
            EVENT_LEARNED,
            LearnedEvent {
                mode,
                id,
                phrase: learned.phrase.clone(),
                observed: learned.observed.clone(),
            },
        );
    }
    if saved_any && let Some(post) = app.try_state::<PostProcessor>() {
        let _ = post.reload_from_db(db);
    }
}

fn raw(err: impl std::fmt::Display) -> UiError {
    UiError::raw(err.to_string())
}

#[tauri::command]
pub fn learn_suggestions(state: State<'_, Learning>) -> Vec<Suggestion> {
    state.pending.lock().unwrap().iter().cloned().collect()
}

/// Aceita a sugestão: vira entrada do dicionário (origem `learned`).
#[tauri::command]
pub fn learn_accept(
    state: State<'_, Learning>,
    db: State<'_, Db>,
    post: State<'_, PostProcessor>,
    id: u64,
) -> Result<DictionaryEntry, UiError> {
    let suggestion = state
        .take(id)
        .ok_or_else(|| UiError::new("learn.not_found", "sugestão não encontrada"))?;
    let entry = save(
        &db,
        &Learned {
            phrase: suggestion.phrase,
            observed: suggestion.observed,
        },
    )
    .map_err(raw)?;
    post.reload_from_db(&db).map_err(raw)?;
    Ok(entry)
}

#[tauri::command]
pub fn learn_dismiss(state: State<'_, Learning>, id: u64) -> bool {
    state.take(id).is_some()
}

/// Desfaz um termo aprendido no modo automático (só apaga entradas de origem `learned`).
#[tauri::command]
pub fn learn_undo(
    db: State<'_, Db>,
    post: State<'_, PostProcessor>,
    entry_id: i64,
) -> Result<bool, UiError> {
    let learned = db
        .dictionary()
        .list()
        .map_err(raw)?
        .into_iter()
        .any(|e| e.id == entry_id && e.origin.as_deref() == Some(ORIGIN));
    if !learned {
        return Err(UiError::new(
            "learn.not_learned",
            "só termos aprendidos podem ser desfeitos aqui",
        ));
    }
    let removed = db.dictionary().soft_delete(entry_id).map_err(raw)?;
    post.reload_from_db(&db).map_err(raw)?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_are_bounded_and_deduplicated() {
        let state = Learning::default();
        let l = |p: &str| Learned {
            phrase: p.into(),
            observed: "x".into(),
        };
        let first = state.push(&l("Vyrp"));
        state.push(&l("vyrp"));
        assert_eq!(
            state.pending.lock().unwrap().len(),
            1,
            "mesmo termo não repete"
        );
        assert!(
            state.take(first).is_none(),
            "o repetido substituiu o primeiro"
        );
        for i in 0..30 {
            state.push(&l(&format!("Termo{i}")));
        }
        assert_eq!(state.pending.lock().unwrap().len(), MAX_PENDING);
    }

    #[test]
    fn paste_mark_is_taken_once_and_only_for_the_same_text() {
        remember_paste(7, vec![1], "antes do ponto ", " depois", "o vai arpe");
        assert!(
            take_mark("outro texto").is_none(),
            "texto diferente descarta a marca"
        );
        remember_paste(7, vec![1], "antes do ponto ", " depois", "o vai arpe");
        let mark = take_mark("o vai arpe").unwrap();
        assert_eq!(mark.prefix, "antes do ponto ");
        assert!(take_mark("o vai arpe").is_none());
    }

    #[test]
    fn save_marks_origin_and_asr_error() {
        let db = Db::open_in_memory().unwrap();
        let entry = save(
            &db,
            &Learned {
                phrase: "Vyrp".into(),
                observed: "vai arpe".into(),
            },
        )
        .unwrap();
        assert_eq!(entry.origin.as_deref(), Some(ORIGIN));
        assert_eq!(entry.observed_asr_error.as_deref(), Some("vai arpe"));
        assert!(!entry.manual_entry);
        assert!(
            db.dictionary()
                .vocabulary()
                .unwrap()
                .contains(&"Vyrp".to_string())
        );
    }
}
