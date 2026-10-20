//! Histórico de ditados (spec §3.1, tabela History).

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Db, Result};
use crate::settings::HistoryRetention;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub created_at: i64,
    /// Texto cru do STT.
    pub raw_text: String,
    /// Texto depois da limpeza por LLM (ou por regras).
    pub clean_text: Option<String>,
    /// O que de fato foi colado.
    pub inserted_text: Option<String>,
    /// Versão editada pelo usuário no histórico.
    pub edited_text: Option<String>,
    pub app: Option<String>,
    pub url: Option<String>,
    pub category: Option<String>,
    pub language: Option<String>,
    /// Como foi disparado: ptt, lock, comando.
    pub command: Option<String>,
    pub style: Option<String>,
    pub engine: Option<String>,
    pub llm_provider: Option<String>,
    pub mic: Option<String>,
    pub duration_ms: i64,
    pub word_count: i64,
    pub dict_replacements: i64,
    /// Soltar a tecla → texto inserido.
    pub latency_total_ms: Option<i64>,
    pub latency_stt_ms: Option<i64>,
    pub latency_llm_ms: Option<i64>,
    pub fallback_level: i64,
    pub status: String,
    /// O usuário desfez a limpeza da IA e voltou ao cru.
    pub reverted_ai: bool,
    /// Áudio opcional (WAV), só quando o usuário liga.
    pub audio: Option<Vec<u8>>,
}

impl HistoryEntry {
    pub fn new(raw_text: impl Into<String>, created_at: i64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            created_at,
            raw_text: raw_text.into(),
            clean_text: None,
            inserted_text: None,
            edited_text: None,
            app: None,
            url: None,
            category: None,
            language: None,
            command: None,
            style: None,
            engine: None,
            llm_provider: None,
            mic: None,
            duration_ms: 0,
            word_count: 0,
            dict_replacements: 0,
            latency_total_ms: None,
            latency_stt_ms: None,
            latency_llm_ms: None,
            fallback_level: 0,
            status: "ok".into(),
            reverted_ai: false,
            audio: None,
        }
    }
}

const COLUMNS: &str = "id, created_at, raw_text, clean_text, inserted_text, edited_text, app, \
    url, category, language, command, style, engine, llm_provider, mic, duration_ms, word_count, \
    dict_replacements, latency_total_ms, latency_stt_ms, latency_llm_ms, fallback_level, status, \
    reverted_ai, audio";

fn from_row(r: &Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: r.get(0)?,
        created_at: r.get(1)?,
        raw_text: r.get(2)?,
        clean_text: r.get(3)?,
        inserted_text: r.get(4)?,
        edited_text: r.get(5)?,
        app: r.get(6)?,
        url: r.get(7)?,
        category: r.get(8)?,
        language: r.get(9)?,
        command: r.get(10)?,
        style: r.get(11)?,
        engine: r.get(12)?,
        llm_provider: r.get(13)?,
        mic: r.get(14)?,
        duration_ms: r.get(15)?,
        word_count: r.get(16)?,
        dict_replacements: r.get(17)?,
        latency_total_ms: r.get(18)?,
        latency_stt_ms: r.get(19)?,
        latency_llm_ms: r.get(20)?,
        fallback_level: r.get(21)?,
        status: r.get(22)?,
        reverted_ai: r.get(23)?,
        audio: r.get(24)?,
    })
}

/// Escapa `%`, `_` e `\` para usar o texto digitado dentro de um `LIKE ... ESCAPE '\'`.
fn like_pattern(term: &str) -> String {
    let mut out = String::from("%");
    for c in term.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

pub struct HistoryRepo<'a>(&'a Db);

impl<'a> HistoryRepo<'a> {
    pub(super) fn new(db: &'a Db) -> Self {
        Self(db)
    }

    /// Grava o ditado respeitando a retenção. Com `Never` nada é escrito e o
    /// retorno é `false`. Com `Day`, aproveita para limpar o que passou de 24 h.
    pub fn record(&self, entry: &HistoryEntry, retention: HistoryRetention) -> Result<bool> {
        if retention == HistoryRetention::Never {
            return Ok(false);
        }
        self.insert(entry)?;
        self.purge(retention, entry.created_at)?;
        Ok(true)
    }

    pub fn insert(&self, e: &HistoryEntry) -> Result<()> {
        self.0.conn().execute(
            &format!(
                "INSERT INTO history ({COLUMNS}) VALUES \
                 (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)"
            ),
            params![
                e.id,
                e.created_at,
                e.raw_text,
                e.clean_text,
                e.inserted_text,
                e.edited_text,
                e.app,
                e.url,
                e.category,
                e.language,
                e.command,
                e.style,
                e.engine,
                e.llm_provider,
                e.mic,
                e.duration_ms,
                e.word_count,
                e.dict_replacements,
                e.latency_total_ms,
                e.latency_stt_ms,
                e.latency_llm_ms,
                e.fallback_level,
                e.status,
                e.reverted_ai,
                e.audio,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<HistoryEntry>> {
        Ok(self
            .0
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM history WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?)
    }

    /// Mais recentes primeiro. `search` procura nos textos cru, limpo, colado e editado.
    pub fn list(&self, search: Option<&str>, limit: u32, offset: u32) -> Result<Vec<HistoryEntry>> {
        let conn = self.0.conn();
        let pattern = search
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(like_pattern);
        let sql = format!(
            "SELECT {COLUMNS} FROM history \
             WHERE ?1 IS NULL OR raw_text LIKE ?1 ESCAPE '\\' OR clean_text LIKE ?1 ESCAPE '\\' \
                OR inserted_text LIKE ?1 ESCAPE '\\' OR edited_text LIKE ?1 ESCAPE '\\' \
             ORDER BY created_at DESC, rowid DESC LIMIT ?2 OFFSET ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, limit, offset], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn count(&self) -> Result<u64> {
        Ok(self
            .0
            .conn()
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))?)
    }

    /// `None` desfaz a edição.
    pub fn set_edited_text(&self, id: &str, text: Option<&str>) -> Result<bool> {
        let n = self.0.conn().execute(
            "UPDATE history SET edited_text = ?2 WHERE id = ?1",
            params![id, text],
        )?;
        Ok(n > 0)
    }

    pub fn set_reverted_ai(&self, id: &str, reverted: bool) -> Result<bool> {
        let n = self.0.conn().execute(
            "UPDATE history SET reverted_ai = ?2 WHERE id = ?1",
            params![id, reverted],
        )?;
        Ok(n > 0)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let n = self
            .0
            .conn()
            .execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(n > 0)
    }

    pub fn clear(&self) -> Result<u64> {
        Ok(self.0.conn().execute("DELETE FROM history", [])? as u64)
    }

    /// Aplica a retenção: `Keep` não apaga nada, `Day` apaga o que tem mais de
    /// 24 h em relação a `now_ms`, `Never` esvazia. Devolve quantas linhas saíram.
    pub fn purge(&self, retention: HistoryRetention, now_ms: i64) -> Result<u64> {
        let conn = self.0.conn();
        let n = match retention {
            HistoryRetention::Keep => 0,
            HistoryRetention::Day => conn.execute(
                "DELETE FROM history WHERE created_at < ?1",
                [now_ms - DAY_MS],
            )?,
            HistoryRetention::Never => conn.execute("DELETE FROM history", [])?,
        };
        Ok(n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn entry(raw: &str, at: i64) -> HistoryEntry {
        HistoryEntry::new(raw, at)
    }

    #[test]
    fn insert_and_get_round_trip_every_field() {
        let db = db();
        let mut e = entry("oi mundo", 1_000);
        e.clean_text = Some("Oi, mundo.".into());
        e.inserted_text = Some("Oi, mundo.".into());
        e.app = Some("notepad.exe".into());
        e.language = Some("pt".into());
        e.duration_ms = 1_800;
        e.word_count = 2;
        e.latency_total_ms = Some(420);
        e.reverted_ai = true;
        e.audio = Some(vec![1, 2, 3]);
        db.history().insert(&e).unwrap();
        assert_eq!(db.history().get(&e.id).unwrap(), Some(e));
        assert_eq!(db.history().get("nada").unwrap(), None);
    }

    #[test]
    fn list_is_newest_first_and_paginates() {
        let db = db();
        for (i, raw) in ["a", "b", "c"].iter().enumerate() {
            db.history().insert(&entry(raw, i as i64)).unwrap();
        }
        let all = db.history().list(None, 10, 0).unwrap();
        let raws: Vec<_> = all.iter().map(|e| e.raw_text.as_str()).collect();
        assert_eq!(raws, ["c", "b", "a"]);
        let page = db.history().list(None, 1, 1).unwrap();
        assert_eq!(page[0].raw_text, "b");
        assert_eq!(db.history().count().unwrap(), 3);
    }

    #[test]
    fn search_covers_all_texts_and_escapes_wildcards() {
        let db = db();
        let mut a = entry("reunião amanhã", 1);
        a.clean_text = Some("Reunião amanhã.".into());
        let mut b = entry("100% certo", 2);
        b.edited_text = Some("cem por cento".into());
        db.history().insert(&a).unwrap();
        db.history().insert(&b).unwrap();
        assert_eq!(db.history().list(Some("amanhã"), 10, 0).unwrap().len(), 1);
        assert_eq!(db.history().list(Some("cem por"), 10, 0).unwrap().len(), 1);
        // `%` literal, não curinga: só a entrada que contém "100%".
        assert_eq!(db.history().list(Some("100%"), 10, 0).unwrap().len(), 1);
        assert_eq!(db.history().list(Some("%"), 10, 0).unwrap().len(), 1);
        assert_eq!(db.history().list(Some("   "), 10, 0).unwrap().len(), 2);
    }

    #[test]
    fn edit_revert_and_delete() {
        let db = db();
        let e = entry("texto", 1);
        db.history().insert(&e).unwrap();
        assert!(
            db.history()
                .set_edited_text(&e.id, Some("editado"))
                .unwrap()
        );
        assert!(db.history().set_reverted_ai(&e.id, true).unwrap());
        let got = db.history().get(&e.id).unwrap().unwrap();
        assert_eq!(got.edited_text.as_deref(), Some("editado"));
        assert!(got.reverted_ai);
        assert!(!db.history().set_edited_text("nada", None).unwrap());
        assert!(db.history().delete(&e.id).unwrap());
        assert!(!db.history().delete(&e.id).unwrap());
    }

    #[test]
    fn retention_keep_never_deletes() {
        let db = db();
        db.history().insert(&entry("velho", 0)).unwrap();
        let now = 10 * DAY_MS;
        assert_eq!(db.history().purge(HistoryRetention::Keep, now).unwrap(), 0);
        assert_eq!(db.history().count().unwrap(), 1);
    }

    #[test]
    fn retention_day_drops_only_older_than_24h() {
        let db = db();
        let now = 5 * DAY_MS;
        db.history()
            .insert(&entry("velho", now - DAY_MS - 1))
            .unwrap();
        db.history().insert(&entry("limite", now - DAY_MS)).unwrap();
        db.history().insert(&entry("novo", now - 1)).unwrap();
        assert_eq!(db.history().purge(HistoryRetention::Day, now).unwrap(), 1);
        let left = db.history().list(None, 10, 0).unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|e| e.raw_text != "velho"));
    }

    #[test]
    fn retention_never_stores_nothing() {
        let db = db();
        let stored = db
            .history()
            .record(&entry("segredo", 1), HistoryRetention::Never)
            .unwrap();
        assert!(!stored);
        assert_eq!(db.history().count().unwrap(), 0);
    }

    #[test]
    fn record_with_day_cleans_old_rows_on_the_way() {
        let db = db();
        db.history().insert(&entry("velho", 0)).unwrap();
        let now = 3 * DAY_MS;
        assert!(
            db.history()
                .record(&entry("novo", now), HistoryRetention::Day)
                .unwrap()
        );
        let left = db.history().list(None, 10, 0).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].raw_text, "novo");
    }

    #[test]
    fn switching_to_never_clears_existing_rows() {
        let db = db();
        db.history().insert(&entry("a", 1)).unwrap();
        db.history().insert(&entry("b", 2)).unwrap();
        assert_eq!(db.history().purge(HistoryRetention::Never, 3).unwrap(), 2);
        assert_eq!(db.history().clear().unwrap(), 0);
    }
}
