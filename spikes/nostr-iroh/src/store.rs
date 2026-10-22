//! SQLite event store. Runtime queries on purpose: this spike tests whether
//! "our own SQLite" is a good enough store, not the compile-checked macros.

use anyhow::Result;
use nostr::prelude::{Event, EventId, Filter, MatchEventOptions};
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::path::Path;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let opts = SqliteConnectOptions::new()
            .filename(dir.join("events.sqlite"))
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS events (
                id         BLOB PRIMARY KEY,
                pubkey     BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                kind       INTEGER NOT NULL,
                raw        TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS events_created ON events (created_at)")
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    /// Verify, then store. Verification is not optional here: there is no
    /// path into the table that skips it (DESIGN.md §14).
    /// Returns `Ok(true)` if the event was new.
    pub async fn insert(&self, event: &Event) -> Result<bool> {
        event.verify()?;
        let res = sqlx::query(
            "INSERT OR IGNORE INTO events (id, pubkey, created_at, kind, raw) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(event.id.as_bytes().to_vec())
        .bind(event.pubkey.as_bytes().to_vec())
        .bind(event.created_at.as_secs() as i64)
        .bind(event.kind.as_u16() as i64)
        .bind(serde_json::to_string(event)?)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Events matching `filter`, newest first. SQL narrows by time and kind;
    /// the exact NIP-01 match is done in Rust (fine for a spike).
    pub async fn query(&self, filter: &Filter) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT raw FROM events ORDER BY created_at DESC, id ASC")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::new();
        for row in rows {
            let event = serde_json::from_str::<Event>(&row.get::<String, _>("raw"))?;
            if filter.match_event(&event, MatchEventOptions::default()) {
                out.push(event);
                if let Some(limit) = filter.limit {
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    pub async fn get(&self, ids: &[EventId]) -> Result<Vec<Event>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let row = sqlx::query("SELECT raw FROM events WHERE id = ?")
                .bind(id.as_bytes().to_vec())
                .fetch_optional(&self.pool)
                .await?;
            if let Some(row) = row {
                out.push(serde_json::from_str::<Event>(&row.get::<String, _>("raw"))?);
            }
        }
        Ok(out)
    }

    pub async fn count(&self) -> Result<i64> {
        Ok(sqlx::query("SELECT COUNT(*) AS n FROM events")
            .fetch_one(&self.pool)
            .await?
            .get("n"))
    }
}
