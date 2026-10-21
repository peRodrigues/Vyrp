//! Histórico separado do ditado: só o texto selecionado, a instrução e o resultado.
//! Título, URL, conteúdo antes/depois do cursor, áudio e identidade UIA não entram.
use super::{Db, Result};
use crate::settings::HistoryRetention;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPrompt {
    pub slot: u8,
    pub title: String,
    pub instruction: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformStatus {
    Inserted,
    InsertFailed,
    Undone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transformation {
    pub id: String,
    pub created_at: i64,
    pub original_text: String,
    pub instruction: String,
    pub transformed_text: String,
    pub prompt_slot: Option<u8>,
    pub status: TransformStatus,
    pub provider: String,
}

pub struct TransformRepo<'a>(&'a Db);
impl Db {
    pub fn transformations(&self) -> TransformRepo<'_> {
        TransformRepo(self)
    }
}

impl TransformRepo<'_> {
    pub fn prompts(&self) -> Result<Vec<SavedPrompt>> {
        let conn = self.0.conn();
        let mut stmt =
            conn.prepare("SELECT slot,title,instruction FROM transform_prompts ORDER BY slot")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(SavedPrompt {
                    slot: r.get(0)?,
                    title: r.get(1)?,
                    instruction: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn save_prompt(&self, prompt: &SavedPrompt) -> Result<()> {
        self.0.conn().execute(
            "UPDATE transform_prompts SET title=?2,instruction=?3 WHERE slot=?1",
            params![prompt.slot, prompt.title, prompt.instruction],
        )?;
        Ok(())
    }

    pub fn insert(&self, entry: &Transformation, retention: HistoryRetention) -> Result<()> {
        self.purge(retention, entry.created_at)?;
        if retention == HistoryRetention::Never {
            return Ok(());
        }
        self.0.conn().execute("INSERT INTO transformations(id,created_at,original_text,instruction,transformed_text,prompt_slot,status,provider) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![entry.id, entry.created_at, entry.original_text, entry.instruction, entry.transformed_text, entry.prompt_slot, status_name(&entry.status), entry.provider])?;
        Ok(())
    }

    pub fn list(&self, limit: u32, offset: u32) -> Result<Vec<Transformation>> {
        let conn = self.0.conn();
        let mut stmt = conn.prepare("SELECT id,created_at,original_text,instruction,transformed_text,prompt_slot,status,provider FROM transformations ORDER BY created_at DESC,id DESC LIMIT ?1 OFFSET ?2")?;
        Ok(stmt
            .query_map(params![limit.clamp(1, 100), offset], |r| {
                let status: String = r.get(6)?;
                Ok(Transformation {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    original_text: r.get(2)?,
                    instruction: r.get(3)?,
                    transformed_text: r.get(4)?,
                    prompt_slot: r.get(5)?,
                    status: match status.as_str() {
                        "inserted" => TransformStatus::Inserted,
                        "undone" => TransformStatus::Undone,
                        _ => TransformStatus::InsertFailed,
                    },
                    provider: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get(&self, id: &str) -> Result<Option<Transformation>> {
        // Lookup is deliberately separate from the page limit.
        let conn = self.0.conn();
        Ok(conn.query_row("SELECT id,created_at,original_text,instruction,transformed_text,prompt_slot,status,provider FROM transformations WHERE id=?1", [id], |r| {
            let status: String = r.get(6)?;
            Ok(Transformation { id:r.get(0)?,created_at:r.get(1)?,original_text:r.get(2)?,instruction:r.get(3)?,transformed_text:r.get(4)?,prompt_slot:r.get(5)?,status:match status.as_str(){ "inserted"=>TransformStatus::Inserted,"undone"=>TransformStatus::Undone,_=>TransformStatus::InsertFailed },provider:r.get(7)? })
        }).optional()?)
    }

    pub fn mark_undone(&self, id: &str) -> Result<()> {
        self.0.conn().execute(
            "UPDATE transformations SET status='undone' WHERE id=?1 AND status='inserted'",
            [id],
        )?;
        Ok(())
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        self.0
            .conn()
            .execute("DELETE FROM transformations WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn clear(&self) -> Result<()> {
        self.0.conn().execute("DELETE FROM transformations", [])?;
        Ok(())
    }
    pub fn purge(&self, retention: HistoryRetention, now: i64) -> Result<()> {
        match retention {
            HistoryRetention::Never => self.clear()?,
            HistoryRetention::Day => {
                self.0.conn().execute(
                    "DELETE FROM transformations WHERE created_at < ?1",
                    [now.saturating_sub(86_400_000)],
                )?;
            }
            HistoryRetention::Keep => {}
        }
        Ok(())
    }
}

fn status_name(status: &TransformStatus) -> &'static str {
    match status {
        TransformStatus::Inserted => "inserted",
        TransformStatus::InsertFailed => "insert_failed",
        TransformStatus::Undone => "undone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_prompts_and_retention_do_not_touch_dictation() {
        let db = Db::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO history(id,created_at,raw_text) VALUES ('dictation',1,'kept')",
                [],
            )
            .unwrap();
        let repo = db.transformations();
        assert_eq!(repo.prompts().unwrap().len(), 9);
        repo.save_prompt(&SavedPrompt {
            slot: 9,
            title: "Résumé".into(),
            instruction: "Resuma em uma linha".into(),
        })
        .unwrap();
        assert_eq!(repo.prompts().unwrap()[8].title, "Résumé");
        assert_eq!(
            db.conn()
                .query_row(
                    "SELECT raw_text FROM history WHERE id='dictation'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "kept"
        );
        let entry = Transformation {
            id: "t1".into(),
            created_at: 10,
            original_text: "seleção".into(),
            instruction: "resuma".into(),
            transformed_text: "resumo".into(),
            prompt_slot: Some(9),
            status: TransformStatus::Inserted,
            provider: "fake".into(),
        };
        repo.insert(&entry, HistoryRetention::Keep).unwrap();
        repo.mark_undone("t1").unwrap();
        assert_eq!(
            repo.get("t1").unwrap().unwrap().status,
            TransformStatus::Undone
        );
        repo.purge(HistoryRetention::Day, 86_400_011).unwrap();
        assert!(repo.list(20, 0).unwrap().is_empty());
        repo.insert(&entry, HistoryRetention::Never).unwrap();
        assert!(repo.list(20, 0).unwrap().is_empty());
        assert_eq!(repo.prompts().unwrap()[8].title, "Résumé");
    }
    #[test]
    fn transformation_schema_cannot_store_screen_context() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn();
        let mut stmt = conn.prepare("PRAGMA table_info(transformations)").unwrap();
        let names = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for forbidden in ["title", "url", "before", "after", "audio", "context"] {
            assert!(!names.iter().any(|n| n == forbidden));
        }
    }
}
