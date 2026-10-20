//! Dicionário e snippets (spec §3.1, tabela Dictionary). Remoção é soft delete.

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Db, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: i64,
    /// O que o usuário fala (ou o termo a reconhecer).
    pub phrase: String,
    /// Texto que entra no lugar; `None` = só ensina o STT a grafar a frase.
    pub replacement: Option<String>,
    pub replacement_html: Option<String>,
    pub is_snippet: bool,
    /// `false` = aprendido automaticamente.
    pub manual_entry: bool,
    pub origin: Option<String>,
    /// Como o STT errou, quando a entrada nasceu de uma correção.
    pub observed_asr_error: Option<String>,
    pub frequency: i64,
    pub last_used_at: Option<i64>,
    pub favorite: bool,
    pub is_deleted: bool,
    pub created_at: i64,
}

/// Dados de uma nova entrada.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewDictionaryEntry {
    pub phrase: String,
    pub replacement: Option<String>,
    pub replacement_html: Option<String>,
    pub is_snippet: bool,
    pub manual_entry: bool,
    pub origin: Option<String>,
    pub observed_asr_error: Option<String>,
}

const COLUMNS: &str = "id, phrase, replacement, replacement_html, is_snippet, manual_entry, \
    origin, observed_asr_error, frequency, last_used_at, favorite, is_deleted, created_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<DictionaryEntry> {
    Ok(DictionaryEntry {
        id: r.get(0)?,
        phrase: r.get(1)?,
        replacement: r.get(2)?,
        replacement_html: r.get(3)?,
        is_snippet: r.get(4)?,
        manual_entry: r.get(5)?,
        origin: r.get(6)?,
        observed_asr_error: r.get(7)?,
        frequency: r.get(8)?,
        last_used_at: r.get(9)?,
        favorite: r.get(10)?,
        is_deleted: r.get(11)?,
        created_at: r.get(12)?,
    })
}

pub struct DictionaryRepo<'a>(&'a Db);

impl<'a> DictionaryRepo<'a> {
    pub(super) fn new(db: &'a Db) -> Self {
        Self(db)
    }

    /// Cria a entrada, ou atualiza a viva com a mesma frase (sem diferenciar
    /// maiúsculas). Uma entrada apagada com a mesma frase não bloqueia: a nova
    /// nasce ao lado dela.
    pub fn upsert(&self, new: &NewDictionaryEntry, now_ms: i64) -> Result<DictionaryEntry> {
        let phrase = new.phrase.trim();
        let conn = self.0.conn();
        let live: Option<i64> = conn
            .query_row(
                "SELECT id FROM dictionary WHERE phrase = ?1 COLLATE NOCASE AND is_deleted = 0",
                [phrase],
                |r| r.get(0),
            )
            .optional()?;
        let id = match live {
            Some(id) => {
                conn.execute(
                    "UPDATE dictionary SET replacement = ?2, replacement_html = ?3, is_snippet = ?4, \
                     manual_entry = ?5, origin = ?6, observed_asr_error = ?7 WHERE id = ?1",
                    params![
                        id,
                        new.replacement,
                        new.replacement_html,
                        new.is_snippet,
                        new.manual_entry,
                        new.origin,
                        new.observed_asr_error
                    ],
                )?;
                id
            }
            None => {
                conn.execute(
                    "INSERT INTO dictionary (phrase, replacement, replacement_html, is_snippet, \
                     manual_entry, origin, observed_asr_error, created_at) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        phrase,
                        new.replacement,
                        new.replacement_html,
                        new.is_snippet,
                        new.manual_entry,
                        new.origin,
                        new.observed_asr_error,
                        now_ms
                    ],
                )?;
                conn.last_insert_rowid()
            }
        };
        Ok(conn.query_row(
            &format!("SELECT {COLUMNS} FROM dictionary WHERE id = ?1"),
            [id],
            from_row,
        )?)
    }

    pub fn get(&self, id: i64) -> Result<Option<DictionaryEntry>> {
        Ok(self
            .0
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM dictionary WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?)
    }

    /// Entradas vivas: favoritas primeiro, depois as mais usadas, depois A–Z.
    pub fn list(&self) -> Result<Vec<DictionaryEntry>> {
        let conn = self.0.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM dictionary WHERE is_deleted = 0 \
             ORDER BY favorite DESC, frequency DESC, phrase COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Frases (não snippets) para o prompt do STT e da limpeza, mais usadas primeiro.
    pub fn vocabulary(&self) -> Result<Vec<String>> {
        let conn = self.0.conn();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(replacement, phrase) FROM dictionary \
             WHERE is_deleted = 0 AND is_snippet = 0 \
             ORDER BY favorite DESC, frequency DESC, phrase COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<bool> {
        let n = self.0.conn().execute(
            "UPDATE dictionary SET favorite = ?2 WHERE id = ?1 AND is_deleted = 0",
            params![id, favorite],
        )?;
        Ok(n > 0)
    }

    /// Conta um uso: frequência +1 e data do último uso.
    pub fn touch_used(&self, id: i64, now_ms: i64) -> Result<bool> {
        let n = self.0.conn().execute(
            "UPDATE dictionary SET frequency = frequency + 1, last_used_at = ?2 \
             WHERE id = ?1 AND is_deleted = 0",
            params![id, now_ms],
        )?;
        Ok(n > 0)
    }

    /// Todas as linhas, inclusive as apagadas (para saber se o seed já rodou).
    pub fn count_all(&self) -> Result<u64> {
        Ok(self
            .0
            .conn()
            .query_row("SELECT count(*) FROM dictionary", [], |r| r.get(0))?)
    }

    /// Edita frase, expansão e tipo de uma entrada viva. `None` = não existe.
    /// Frase igual à de outra entrada viva falha por violar a unicidade.
    pub fn update(&self, id: i64, new: &NewDictionaryEntry) -> Result<Option<DictionaryEntry>> {
        let n = self.0.conn().execute(
            "UPDATE dictionary SET phrase = ?2, replacement = ?3, replacement_html = ?4, \
             is_snippet = ?5 WHERE id = ?1 AND is_deleted = 0",
            params![
                id,
                new.phrase.trim(),
                new.replacement,
                new.replacement_html,
                new.is_snippet
            ],
        )?;
        if n == 0 {
            return Ok(None);
        }
        self.get(id)
    }

    pub fn soft_delete(&self, id: i64) -> Result<bool> {
        let n = self.0.conn().execute(
            "UPDATE dictionary SET is_deleted = 1 WHERE id = ?1 AND is_deleted = 0",
            [id],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn word(phrase: &str) -> NewDictionaryEntry {
        NewDictionaryEntry {
            phrase: phrase.into(),
            manual_entry: true,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_creates_and_reads_back() {
        let db = db();
        let e = db
            .dictionary()
            .upsert(
                &NewDictionaryEntry {
                    replacement: Some("Vyrp".into()),
                    origin: Some("manual".into()),
                    ..word("virp")
                },
                50,
            )
            .unwrap();
        assert_eq!(e.phrase, "virp");
        assert_eq!(e.replacement.as_deref(), Some("Vyrp"));
        assert_eq!((e.frequency, e.created_at, e.is_deleted), (0, 50, false));
        assert_eq!(db.dictionary().get(e.id).unwrap(), Some(e));
    }

    #[test]
    fn same_phrase_updates_instead_of_duplicating() {
        let db = db();
        let a = db.dictionary().upsert(&word("Tauri"), 1).unwrap();
        let b = db
            .dictionary()
            .upsert(
                &NewDictionaryEntry {
                    replacement: Some("Tauri 2".into()),
                    ..word("  tauri ")
                },
                2,
            )
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.replacement.as_deref(), Some("Tauri 2"));
        assert_eq!(db.dictionary().list().unwrap().len(), 1);
    }

    #[test]
    fn soft_delete_hides_but_keeps_the_row_and_frees_the_phrase() {
        let db = db();
        let a = db.dictionary().upsert(&word("Rust"), 1).unwrap();
        assert!(db.dictionary().soft_delete(a.id).unwrap());
        assert!(!db.dictionary().soft_delete(a.id).unwrap());
        assert!(db.dictionary().list().unwrap().is_empty());
        assert!(db.dictionary().get(a.id).unwrap().unwrap().is_deleted);
        let b = db.dictionary().upsert(&word("Rust"), 2).unwrap();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn usage_and_favorites_drive_the_order() {
        let db = db();
        let a = db.dictionary().upsert(&word("alfa"), 1).unwrap();
        let b = db.dictionary().upsert(&word("beta"), 1).unwrap();
        let c = db.dictionary().upsert(&word("gama"), 1).unwrap();
        db.dictionary().touch_used(b.id, 10).unwrap();
        db.dictionary().touch_used(b.id, 11).unwrap();
        db.dictionary().touch_used(a.id, 12).unwrap();
        let names = |v: Vec<DictionaryEntry>| v.into_iter().map(|e| e.phrase).collect::<Vec<_>>();
        assert_eq!(
            names(db.dictionary().list().unwrap()),
            ["beta", "alfa", "gama"]
        );
        db.dictionary().set_favorite(c.id, true).unwrap();
        assert_eq!(
            names(db.dictionary().list().unwrap()),
            ["gama", "beta", "alfa"]
        );
        let beta = db.dictionary().get(b.id).unwrap().unwrap();
        assert_eq!((beta.frequency, beta.last_used_at), (2, Some(11)));
    }

    #[test]
    fn vocabulary_skips_snippets_and_deleted() {
        let db = db();
        db.dictionary().upsert(&word("Wispr"), 1).unwrap();
        db.dictionary()
            .upsert(
                &NewDictionaryEntry {
                    is_snippet: true,
                    replacement: Some("meu e-mail".into()),
                    ..word("email")
                },
                1,
            )
            .unwrap();
        let gone = db.dictionary().upsert(&word("apagada"), 1).unwrap();
        db.dictionary().soft_delete(gone.id).unwrap();
        assert_eq!(db.dictionary().vocabulary().unwrap(), ["Wispr"]);
    }

    #[test]
    fn changes_on_missing_or_deleted_rows_report_false() {
        let db = db();
        assert!(!db.dictionary().touch_used(99, 1).unwrap());
        assert!(!db.dictionary().set_favorite(99, true).unwrap());
    }
}
