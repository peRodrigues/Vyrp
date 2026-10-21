//! CRUD do dicionário para a UI (V18): regras de validação, CSV, seed inicial e os
//! commands Tauri. Toda mudança recompila o [`PostProcessor`].

use serde::{Deserialize, Serialize};
use tauri::State;

use super::dictionary::NewDictionaryEntry;
use super::{Db, DictionaryEntry, Result, now_ms};
use crate::core::ui_error::UiError;
use crate::post::PostProcessor;

/// O que a UI envia ao salvar. `id` presente = editar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryDraft {
    pub id: Option<i64>,
    pub phrase: String,
    pub replacement: Option<String>,
    pub is_snippet: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub added: u32,
    pub updated: u32,
    pub skipped: u32,
}

/// Valida e normaliza o rascunho; erro = código estável para a UI traduzir.
fn clean(draft: &DictionaryDraft) -> std::result::Result<NewDictionaryEntry, UiError> {
    let phrase = draft.phrase.trim();
    if phrase.is_empty() {
        return Err(UiError::new(
            "dictionary.phrase_required",
            "Digite a frase.",
        ));
    }
    let replacement = draft
        .replacement
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if draft.is_snippet && replacement.is_none() {
        return Err(UiError::new(
            "dictionary.snippet_needs_text",
            "Um snippet precisa de um texto de expansão.",
        ));
    }
    Ok(NewDictionaryEntry {
        phrase: phrase.to_string(),
        replacement,
        is_snippet: draft.is_snippet,
        manual_entry: true,
        ..Default::default()
    })
}

fn raw(err: impl std::fmt::Display) -> UiError {
    UiError::raw(err.to_string())
}

fn friendly(err: super::StorageError) -> UiError {
    let text = err.to_string();
    if text.contains("UNIQUE") {
        UiError::new(
            "dictionary.duplicate",
            "Já existe uma entrada com essa frase.",
        )
    } else {
        UiError::raw(text)
    }
}

/// Cria ou edita. Editar com a frase de outra entrada viva é erro.
pub fn save(
    db: &Db,
    draft: &DictionaryDraft,
    now: i64,
) -> std::result::Result<DictionaryEntry, UiError> {
    let new = clean(draft)?;
    match draft.id {
        Some(id) => db
            .dictionary()
            .update(id, &new)
            .map_err(friendly)?
            .ok_or_else(|| UiError::new("dictionary.gone", "Essa entrada não existe mais.")),
        None => db.dictionary().upsert(&new, now).map_err(friendly),
    }
}

/// Entradas vivas filtradas por `search` (frase ou expansão, sem caixa).
pub fn list(db: &Db, search: Option<&str>) -> Result<Vec<DictionaryEntry>> {
    let mut entries = db.dictionary().list()?;
    if let Some(term) = search
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
    {
        entries.retain(|e| {
            e.phrase.to_lowercase().contains(&term)
                || e.replacement
                    .as_deref()
                    .is_some_and(|r| r.to_lowercase().contains(&term))
        });
    }
    Ok(entries)
}

/// Exemplos do primeiro uso. Só semeia um dicionário que nunca teve linha nenhuma
/// (nem apagada), para não ressuscitar o que o usuário removeu.
pub fn seed_if_empty(db: &Db, now: i64) -> Result<bool> {
    if db.dictionary().count_all()? > 0 {
        return Ok(false);
    }
    let examples = [
        ("Vyrp", None, false),
        ("meu e-mail", Some("seuemail@exemplo.com"), true),
        ("assinatura", Some("Abraço,\nSeu Nome"), true),
    ];
    for (phrase, replacement, is_snippet) in examples {
        db.dictionary().upsert(
            &NewDictionaryEntry {
                phrase: phrase.into(),
                replacement: replacement.map(Into::into),
                is_snippet,
                manual_entry: false,
                origin: Some("seed".into()),
                ..Default::default()
            },
            now,
        )?;
    }
    Ok(true)
}

// ---------------------------------------------------------------- CSV

const CSV_HEADER: [&str; 4] = ["phrase", "replacement", "is_snippet", "favorite"];

fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn to_csv(entries: &[DictionaryEntry]) -> String {
    let mut out = CSV_HEADER.join(",");
    out.push_str("\r\n");
    for e in entries {
        let row = [
            csv_field(&e.phrase),
            csv_field(e.replacement.as_deref().unwrap_or("")),
            u8::from(e.is_snippet).to_string(),
            u8::from(e.favorite).to_string(),
        ];
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

/// CSV (RFC 4180 simplificado): aspas duplas, `""` escapado, quebras dentro de aspas.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if field.is_empty() => quoted = true,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

pub fn import_csv(db: &Db, text: &str, now: i64) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let mut rows = parse_csv(text);
    if rows
        .first()
        .and_then(|r| r.first())
        .is_some_and(|c| c.trim().eq_ignore_ascii_case("phrase"))
    {
        rows.remove(0);
    }
    let flag = |row: &[String], i: usize| {
        row.get(i)
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "sim"))
    };
    for row in rows {
        let draft = DictionaryDraft {
            id: None,
            phrase: row.first().cloned().unwrap_or_default(),
            replacement: row.get(1).cloned(),
            is_snippet: flag(&row, 2),
        };
        let Ok(new) = clean(&draft) else {
            report.skipped += 1;
            continue;
        };
        let existed = db
            .dictionary()
            .list()?
            .iter()
            .any(|e| e.phrase.eq_ignore_ascii_case(&new.phrase));
        let entry = db.dictionary().upsert(&new, now)?;
        if flag(&row, 3) {
            db.dictionary().set_favorite(entry.id, true)?;
        }
        if existed {
            report.updated += 1;
        } else {
            report.added += 1;
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------- commands Tauri

fn reload(db: &Db, post: &PostProcessor) -> std::result::Result<(), UiError> {
    post.reload_from_db(db).map_err(raw)
}

#[tauri::command]
pub fn dictionary_list(
    db: State<'_, Db>,
    search: Option<String>,
) -> std::result::Result<Vec<DictionaryEntry>, UiError> {
    list(&db, search.as_deref()).map_err(raw)
}

#[tauri::command]
pub fn dictionary_save(
    db: State<'_, Db>,
    post: State<'_, PostProcessor>,
    draft: DictionaryDraft,
) -> std::result::Result<DictionaryEntry, UiError> {
    let entry = save(&db, &draft, now_ms())?;
    reload(&db, &post)?;
    Ok(entry)
}

#[tauri::command]
pub fn dictionary_delete(
    db: State<'_, Db>,
    post: State<'_, PostProcessor>,
    id: i64,
) -> std::result::Result<bool, UiError> {
    let removed = db.dictionary().soft_delete(id).map_err(raw)?;
    reload(&db, &post)?;
    Ok(removed)
}

#[tauri::command]
pub fn dictionary_set_favorite(
    db: State<'_, Db>,
    id: i64,
    favorite: bool,
) -> std::result::Result<bool, UiError> {
    db.dictionary().set_favorite(id, favorite).map_err(raw)
}

#[tauri::command]
pub fn dictionary_export_csv(db: State<'_, Db>) -> std::result::Result<String, UiError> {
    let entries = db.dictionary().list().map_err(raw)?;
    Ok(to_csv(&entries))
}

#[tauri::command]
pub fn dictionary_import_csv(
    db: State<'_, Db>,
    post: State<'_, PostProcessor>,
    csv: String,
) -> std::result::Result<ImportReport, UiError> {
    let report = import_csv(&db, &csv, now_ms()).map_err(raw)?;
    reload(&db, &post)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn draft(phrase: &str, replacement: Option<&str>, snippet: bool) -> DictionaryDraft {
        DictionaryDraft {
            id: None,
            phrase: phrase.into(),
            replacement: replacement.map(Into::into),
            is_snippet: snippet,
        }
    }

    #[test]
    fn save_validates_the_draft() {
        let db = db();
        assert!(save(&db, &draft("   ", None, false), 1).is_err());
        assert!(save(&db, &draft("gatilho", Some("  "), true), 1).is_err());
        let ok = save(&db, &draft(" virp ", Some(" Vyrp "), false), 1).unwrap();
        assert_eq!(
            (ok.phrase.as_str(), ok.replacement.as_deref()),
            ("virp", Some("Vyrp"))
        );
        // Vocabulário puro: expansão em branco vira "sem substituição".
        let vocab = save(&db, &draft("Kubernetes", Some(""), false), 1).unwrap();
        assert_eq!(vocab.replacement, None);
    }

    #[test]
    fn editing_changes_the_row_and_rejects_duplicates() {
        let db = db();
        let a = save(&db, &draft("alfa", Some("A"), false), 1).unwrap();
        let b = save(&db, &draft("beta", Some("B"), false), 1).unwrap();
        let edited = save(
            &db,
            &DictionaryDraft {
                id: Some(a.id),
                ..draft("alfa2", Some("A2"), true)
            },
            2,
        )
        .unwrap();
        assert_eq!(edited.id, a.id);
        assert!(edited.is_snippet);
        let clash = save(
            &db,
            &DictionaryDraft {
                id: Some(b.id),
                ..draft("ALFA2", Some("x"), false)
            },
            3,
        );
        let clash = clash.unwrap_err();
        assert_eq!(clash.code, "dictionary.duplicate");
        assert_eq!(clash.to_string(), "Já existe uma entrada com essa frase.");
        let gone = save(
            &db,
            &DictionaryDraft {
                id: Some(999),
                ..draft("x", None, false)
            },
            3,
        );
        assert_eq!(gone.unwrap_err().code, "dictionary.gone");
    }

    #[test]
    fn validation_errors_have_stable_codes() {
        let db = db();
        let empty = save(&db, &draft("  ", None, false), 1).unwrap_err();
        assert_eq!(empty.code, "dictionary.phrase_required");
        let snippet = save(&db, &draft("atalho", None, true), 1).unwrap_err();
        assert_eq!(snippet.code, "dictionary.snippet_needs_text");
    }

    #[test]
    fn list_filters_by_phrase_or_expansion() {
        let db = db();
        save(&db, &draft("virp", Some("Vyrp"), false), 1).unwrap();
        save(&db, &draft("meu e-mail", Some("ana@x.com"), true), 1).unwrap();
        assert_eq!(list(&db, None).unwrap().len(), 2);
        assert_eq!(list(&db, Some("VIRP")).unwrap().len(), 1);
        assert_eq!(list(&db, Some("ana@")).unwrap().len(), 1);
        assert_eq!(list(&db, Some("zzz")).unwrap().len(), 0);
        assert_eq!(list(&db, Some("  ")).unwrap().len(), 2);
    }

    #[test]
    fn seed_runs_once_even_after_the_user_deletes_everything() {
        let db = db();
        assert!(seed_if_empty(&db, 1).unwrap());
        let seeded = list(&db, None).unwrap();
        assert_eq!(seeded.len(), 3);
        assert!(seeded.iter().any(|e| e.is_snippet));
        for e in seeded {
            db.dictionary().soft_delete(e.id).unwrap();
        }
        assert!(!seed_if_empty(&db, 2).unwrap());
        assert!(list(&db, None).unwrap().is_empty());
    }

    #[test]
    fn csv_round_trips_quotes_commas_and_newlines() {
        let db = db();
        let tricky = save(
            &db,
            &draft(
                "diz \"oi\", ok",
                Some("linha 1\nlinha 2, com vírgula"),
                true,
            ),
            1,
        )
        .unwrap();
        db.dictionary().set_favorite(tricky.id, true).unwrap();
        let csv = to_csv(&db.dictionary().list().unwrap());
        let other = Db::open_in_memory().unwrap();
        let report = import_csv(&other, &csv, 5).unwrap();
        assert_eq!(
            report,
            ImportReport {
                added: 1,
                updated: 0,
                skipped: 0
            }
        );
        let back = other.dictionary().list().unwrap().remove(0);
        assert_eq!(back.phrase, "diz \"oi\", ok");
        assert_eq!(
            back.replacement.as_deref(),
            Some("linha 1\nlinha 2, com vírgula")
        );
        assert!(back.is_snippet && back.favorite);
    }

    #[test]
    fn import_reports_added_updated_and_skipped() {
        let db = db();
        save(&db, &draft("virp", Some("velho"), false), 1).unwrap();
        let csv = "\u{feff}phrase,replacement,is_snippet,favorite\r\n\
                   virp,Vyrp,0,0\r\n\
                   novo,Novo,0,1\r\n\
                   ,sem frase,0,0\r\n\
                   gatilho,,1,0\r\n";
        let report = import_csv(&db, csv, 9).unwrap();
        assert_eq!(
            report,
            ImportReport {
                added: 1,
                updated: 1,
                skipped: 2
            }
        );
        let live = list(&db, None).unwrap();
        assert_eq!(live.len(), 2);
        let virp = live.iter().find(|e| e.phrase == "virp").unwrap();
        assert_eq!(virp.replacement.as_deref(), Some("Vyrp"));
        assert!(live.iter().find(|e| e.phrase == "novo").unwrap().favorite);
    }

    #[test]
    fn mutations_reach_the_post_processor() {
        let db = db();
        let post = PostProcessor::new();
        save(&db, &draft("virp", Some("Vyrp"), false), 1).unwrap();
        post.reload_from_db(&db).unwrap();
        let opts = crate::post::PostOptions {
            format: crate::post::TextFormat::Raw,
            ..Default::default()
        };
        assert_eq!(post.process("o virp", &opts).text, "o Vyrp");
        let id = list(&db, None).unwrap()[0].id;
        db.dictionary().soft_delete(id).unwrap();
        post.reload_from_db(&db).unwrap();
        assert_eq!(post.process("o virp", &opts).text, "o virp");
    }
}
