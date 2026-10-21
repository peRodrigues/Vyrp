//! SQLite (histórico, dicionário, notificações) com migrations versionadas.
//!
//! Card V4 (spec `docs/re/wispr-flow-spec.md` §3 e §6). Um arquivo,
//! `<AppData>/vyrp.sqlite`, uma conexão atrás de um `Mutex` (as escritas são
//! pequenas e raras). A versão do esquema fica em `PRAGMA user_version`.

pub mod dictionary;
pub mod dictionary_commands;
pub mod history;
pub mod migrations;
pub mod notifications;
pub mod transformations;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tauri::{AppHandle, Manager};

use crate::settings::SettingsStore;

pub use dictionary::{DictionaryEntry, DictionaryRepo};
pub use history::{HistoryEntry, HistoryRepo};
pub use notifications::{NotificationState, NotificationsRepo};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("o banco está na versão {found}, mais nova que a {supported} deste Vyrp")]
    NewerSchema { found: u32, supported: u32 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Milissegundos desde a época Unix. Os repositórios recebem o "agora" como
/// parâmetro; só quem chama de fora usa isto.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Abre (criando) o arquivo e leva o esquema até a versão atual.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        // WAL é ignorado em memória; em arquivo evita travar leitura durante escrita.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn()
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    pub fn history(&self) -> HistoryRepo<'_> {
        HistoryRepo::new(self)
    }

    pub fn dictionary(&self) -> DictionaryRepo<'_> {
        DictionaryRepo::new(self)
    }

    pub fn notifications(&self) -> NotificationsRepo<'_> {
        NotificationsRepo::new(self)
    }
}

/// Abre o banco no AppData e aplica a retenção do histórico configurada.
/// Chamar no `setup`, depois de `settings::init`.
pub fn init(app: &AppHandle) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = app.path().app_data_dir()?.join("vyrp.sqlite");
    let db = Db::open(&path)?;
    if let Some(settings) = app.try_state::<SettingsStore>() {
        let removed = db
            .history()
            .purge(settings.get().history_retention, now_ms())?;
        if removed > 0 {
            tracing::info!(removed, "histórico antigo removido pela retenção");
        }
    }
    if let Err(err) = dictionary_commands::seed_if_empty(&db, now_ms()) {
        tracing::warn!(error = %err, "seed do dicionário falhou");
    }
    app.manage(db);
    Ok(())
}
