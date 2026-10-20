//! Contadores de notificações por tipo, para o rate-limit (spec §1.9 e §3.2).

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{Db, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationState {
    pub last_shown_at: i64,
    pub times_shown: u32,
}

pub struct NotificationsRepo<'a>(&'a Db);

impl<'a> NotificationsRepo<'a> {
    pub(super) fn new(db: &'a Db) -> Self {
        Self(db)
    }

    pub fn get(&self, kind: &str) -> Result<Option<NotificationState>> {
        Ok(self
            .0
            .conn()
            .query_row(
                "SELECT last_shown_at, times_shown FROM kv_notifications WHERE kind = ?1",
                [kind],
                |r| {
                    Ok(NotificationState {
                        last_shown_at: r.get(0)?,
                        times_shown: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Registra uma exibição agora.
    pub fn record_shown(&self, kind: &str, now_ms: i64) -> Result<NotificationState> {
        self.0.conn().execute(
            "INSERT INTO kv_notifications (kind, last_shown_at, times_shown) VALUES (?1, ?2, 1) \
             ON CONFLICT(kind) DO UPDATE SET last_shown_at = ?2, times_shown = times_shown + 1",
            params![kind, now_ms],
        )?;
        Ok(self.get(kind)?.expect("linha acabou de ser gravada"))
    }

    /// Pode mostrar? Só se passou `min_interval_ms` desde a última e, quando
    /// houver, o teto `max_times` ainda não foi atingido.
    pub fn should_show(
        &self,
        kind: &str,
        now_ms: i64,
        min_interval_ms: i64,
        max_times: Option<u32>,
    ) -> Result<bool> {
        let Some(state) = self.get(kind)? else {
            return Ok(true);
        };
        if max_times.is_some_and(|max| state.times_shown >= max) {
            return Ok(false);
        }
        Ok(now_ms - state.last_shown_at >= min_interval_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_remembers_the_last_time() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.notifications().get("mic").unwrap(), None);
        db.notifications().record_shown("mic", 100).unwrap();
        let s = db.notifications().record_shown("mic", 250).unwrap();
        assert_eq!(
            s,
            NotificationState {
                last_shown_at: 250,
                times_shown: 2
            }
        );
        assert_eq!(db.notifications().get("outro").unwrap(), None);
    }

    #[test]
    fn rate_limit_by_interval_and_by_total() {
        let db = Db::open_in_memory().unwrap();
        let n = db.notifications();
        assert!(n.should_show("dica", 0, 1_000, Some(2)).unwrap());
        n.record_shown("dica", 0).unwrap();
        assert!(!n.should_show("dica", 999, 1_000, Some(2)).unwrap());
        assert!(n.should_show("dica", 1_000, 1_000, Some(2)).unwrap());
        n.record_shown("dica", 1_000).unwrap();
        // Teto de 2 exibições atingido, mesmo passado o intervalo.
        assert!(!n.should_show("dica", 9_999, 1_000, Some(2)).unwrap());
        assert!(n.should_show("dica", 9_999, 1_000, None).unwrap());
    }
}
