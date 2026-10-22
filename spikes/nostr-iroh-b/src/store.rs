//! The store question from `PLAN.md` S1: our own SQLite behind `sqlx`, versus
//! `nostr-lmdb`. This is the SQLite side: events as JSON plus the columns a
//! chat actually queries on, and single-letter tags in their own table so a
//! `#h` filter (a group) is an indexed lookup, not a scan.
//!
//! Runtime-checked queries (`sqlx::query`) because a spike has no
//! `DATABASE_URL`; the product uses the compile-checked macros.

use std::path::Path;

use anyhow::Result;
use nostr::prelude::{Event, EventId, Filter};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS events (
        id         BLOB PRIMARY KEY,
        pubkey     BLOB    NOT NULL,
        created_at INTEGER NOT NULL,
        kind       INTEGER NOT NULL,
        json       TEXT    NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS events_time ON events (created_at, id)",
    "CREATE INDEX IF NOT EXISTS events_author ON events (pubkey, created_at)",
    "CREATE INDEX IF NOT EXISTS events_kind ON events (kind, created_at)",
    "CREATE TABLE IF NOT EXISTS tags (
        event_id BLOB NOT NULL REFERENCES events(id) ON DELETE CASCADE,
        name     TEXT NOT NULL,
        value    TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS tags_lookup ON tags (name, value, event_id)",
];

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
        for stmt in SCHEMA {
            sqlx::query(*stmt).execute(&pool).await?;
        }
        Ok(Self { pool })
    }

    /// `true` if the event was new. The caller has already verified it.
    pub async fn insert(&self, ev: &Event) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let new = insert_in(&mut tx, ev).await?;
        tx.commit().await?;
        Ok(new)
    }

    pub async fn insert_many(&self, evs: &[Event]) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0;
        for ev in evs {
            n += usize::from(insert_in(&mut tx, ev).await?);
        }
        tx.commit().await?;
        Ok(n)
    }

    pub async fn count(&self) -> Result<i64> {
        Ok(sqlx::query("SELECT COUNT(*) FROM events").fetch_one(&self.pool).await?.get(0))
    }

    pub async fn query(&self, filter: &Filter) -> Result<Vec<Event>> {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT json FROM events WHERE 1=1");
        push_filter(&mut qb, filter);
        push_order_limit(&mut qb, filter);
        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|r| Ok(Event::from_json(r.get::<String, _>(0))?))
            .collect()
    }

    /// `(created_at, id)` for every event matching `filter` — what negentropy
    /// reconciles over.
    pub async fn id_times(&self, filter: &Filter) -> Result<Vec<(u64, [u8; 32])>> {
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT created_at, id FROM events WHERE 1=1");
        push_filter(&mut qb, filter);
        let rows = qb.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|r| {
                let id: Vec<u8> = r.get(1);
                Ok((r.get::<i64, _>(0) as u64, id.as_slice().try_into()?))
            })
            .collect()
    }

    pub async fn by_ids(&self, ids: &[EventId]) -> Result<Vec<Event>> {
        self.query(&Filter::new().ids(ids.iter().copied())).await
    }
}

async fn insert_in(tx: &mut sqlx::SqliteConnection, ev: &Event) -> Result<bool> {
    let res = sqlx::query(
        "INSERT OR IGNORE INTO events (id, pubkey, created_at, kind, json) VALUES (?,?,?,?,?)",
    )
    .bind(ev.id.as_bytes().as_slice())
    .bind(ev.pubkey.as_bytes().as_slice())
    .bind(ev.created_at.as_secs() as i64)
    .bind(i64::from(ev.kind.as_u16()))
    .bind(ev.as_json())
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    for tag in ev.tags.iter() {
        let parts = tag.as_slice();
        if let [name, value, ..] = parts {
            if name.len() == 1 {
                sqlx::query("INSERT INTO tags (event_id, name, value) VALUES (?,?,?)")
                    .bind(ev.id.as_bytes().as_slice())
                    .bind(name.as_str())
                    .bind(value.as_str())
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    Ok(true)
}

fn push_filter(qb: &mut QueryBuilder<Sqlite>, f: &Filter) {
    if let Some(ids) = &f.ids {
        qb.push(" AND id IN (");
        let mut sep = qb.separated(", ");
        for id in ids {
            sep.push_bind(id.as_bytes().as_slice());
        }
        qb.push(")");
    }
    if let Some(authors) = &f.authors {
        qb.push(" AND pubkey IN (");
        let mut sep = qb.separated(", ");
        for a in authors {
            sep.push_bind(a.as_bytes().as_slice());
        }
        qb.push(")");
    }
    if let Some(kinds) = &f.kinds {
        qb.push(" AND kind IN (");
        let mut sep = qb.separated(", ");
        for k in kinds {
            sep.push_bind(i64::from(k.as_u16()));
        }
        qb.push(")");
    }
    if let Some(since) = f.since {
        qb.push(" AND created_at >= ").push_bind(since.as_secs() as i64);
    }
    if let Some(until) = f.until {
        qb.push(" AND created_at <= ").push_bind(until.as_secs() as i64);
    }
    for (tag, values) in &f.generic_tags {
        qb.push(" AND id IN (SELECT event_id FROM tags WHERE name = ")
            .push_bind(tag.to_string())
            .push(" AND value IN (");
        let mut sep = qb.separated(", ");
        for v in values {
            sep.push_bind(v.as_str());
        }
        qb.push("))");
    }
}

fn push_order_limit(qb: &mut QueryBuilder<Sqlite>, f: &Filter) {
    qb.push(" ORDER BY created_at DESC, id");
    if let Some(limit) = f.limit {
        qb.push(" LIMIT ").push_bind(limit as i64);
    }
}
