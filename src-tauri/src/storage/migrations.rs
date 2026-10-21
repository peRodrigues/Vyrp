//! Migrations versionadas: cada item de [`MIGRATIONS`] leva o banco da versão
//! `i` para `i + 1`. Nunca edite um item já publicado; acrescente um novo.

use rusqlite::Connection;

use super::{Result, StorageError};

pub const MIGRATIONS: &[&str] = &[
    // v1: esquema inicial.
    r#"
    CREATE TABLE history (
        id                  TEXT PRIMARY KEY,
        created_at          INTEGER NOT NULL,
        raw_text            TEXT NOT NULL,
        clean_text          TEXT,
        inserted_text       TEXT,
        edited_text         TEXT,
        app                 TEXT,
        url                 TEXT,
        category            TEXT,
        language            TEXT,
        command             TEXT,
        style               TEXT,
        engine              TEXT,
        llm_provider        TEXT,
        mic                 TEXT,
        duration_ms         INTEGER NOT NULL DEFAULT 0,
        word_count          INTEGER NOT NULL DEFAULT 0,
        dict_replacements   INTEGER NOT NULL DEFAULT 0,
        latency_total_ms    INTEGER,
        latency_stt_ms      INTEGER,
        latency_llm_ms      INTEGER,
        fallback_level      INTEGER NOT NULL DEFAULT 0,
        status              TEXT NOT NULL DEFAULT 'ok',
        reverted_ai         INTEGER NOT NULL DEFAULT 0,
        audio               BLOB
    );
    CREATE INDEX history_created_at ON history (created_at DESC);

    CREATE TABLE dictionary (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        phrase              TEXT NOT NULL,
        replacement         TEXT,
        replacement_html    TEXT,
        is_snippet          INTEGER NOT NULL DEFAULT 0,
        manual_entry        INTEGER NOT NULL DEFAULT 1,
        origin              TEXT,
        observed_asr_error  TEXT,
        frequency           INTEGER NOT NULL DEFAULT 0,
        last_used_at        INTEGER,
        favorite            INTEGER NOT NULL DEFAULT 0,
        is_deleted          INTEGER NOT NULL DEFAULT 0,
        created_at          INTEGER NOT NULL
    );
    -- Uma frase viva por vez (sem diferenciar maiúsculas); apagadas não contam.
    CREATE UNIQUE INDEX dictionary_phrase_live
        ON dictionary (phrase COLLATE NOCASE) WHERE is_deleted = 0;

    CREATE TABLE kv_notifications (
        kind            TEXT PRIMARY KEY,
        last_shown_at   INTEGER NOT NULL,
        times_shown     INTEGER NOT NULL DEFAULT 0
    );
    "#,
    // v2: V25. Somente seleção/instrução/resultado; nunca contexto de tela.
    r#"
    CREATE TABLE transform_prompts (
        slot INTEGER PRIMARY KEY CHECK(slot BETWEEN 1 AND 9),
        title TEXT NOT NULL DEFAULT '',
        instruction TEXT NOT NULL DEFAULT ''
    );
    INSERT INTO transform_prompts(slot) VALUES (1),(2),(3),(4),(5),(6),(7),(8),(9);
    CREATE TABLE transformations (
        id TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        original_text TEXT NOT NULL,
        instruction TEXT NOT NULL,
        transformed_text TEXT NOT NULL,
        prompt_slot INTEGER,
        status TEXT NOT NULL,
        provider TEXT NOT NULL
    );
    CREATE INDEX transformations_created_at ON transformations(created_at DESC);
    "#,
];

/// Leva `conn` até a última versão de [`MIGRATIONS`].
pub fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_with(conn, MIGRATIONS)
}

/// Mesma coisa com uma lista explícita (os testes simulam versões futuras).
/// Cada passo roda numa transação junto com o `user_version`.
pub fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let supported = migrations.len() as u32;
    if current > supported {
        return Err(StorageError::NewerSchema {
            found: current,
            supported,
        });
    }
    for (index, sql) in migrations.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn fresh_database_reaches_latest_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), MIGRATIONS.len() as u32);
        for table in ["history", "dictionary", "kv_notifications"] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "tabela {table}");
        }
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), MIGRATIONS.len() as u32);
    }

    #[test]
    fn only_missing_steps_run_on_an_existing_database() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_with(&mut conn, &["CREATE TABLE a (x INTEGER);"]).unwrap();
        conn.execute("INSERT INTO a VALUES (7)", []).unwrap();
        migrate_with(
            &mut conn,
            &[
                "CREATE TABLE a (x INTEGER);",
                "ALTER TABLE a ADD COLUMN y INTEGER DEFAULT 1;",
            ],
        )
        .unwrap();
        assert_eq!(version(&conn), 2);
        let (x, y): (i64, i64) = conn
            .query_row("SELECT x, y FROM a", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((x, y), (7, 1));
    }

    #[test]
    fn failed_step_rolls_back_and_keeps_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        let bad = ["CREATE TABLE a (x INTEGER); NÃO É SQL;"];
        assert!(migrate_with(&mut conn, &bad).is_err());
        assert_eq!(version(&conn), 0);
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as u32 + 1)
            .unwrap();
        assert!(matches!(
            migrate(&mut conn),
            Err(StorageError::NewerSchema { .. })
        ));
    }
}
