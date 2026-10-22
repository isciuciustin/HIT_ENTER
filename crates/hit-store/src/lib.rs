//! The event store: our own SQLite behind `sqlx`, NIP-01 filter queries, and
//! the `(created_at, id)` sets negentropy reconciles over (M2,
//! `docs/PLAN.md`; spike S1).
//!
//! - **Only verified events reach it.** [`Store::insert`] takes a
//!   [`Verified`], which `hit-event` constructs only by checking the
//!   signature. Permissions (`hit-group`) are the caller's, before this.
//! - **Replaceable and addressable kinds keep one event per slot**, decided
//!   by `hit_event::replaces` — the same function the binding book uses, so
//!   the two never disagree. Ephemeral kinds are not stored.
//! - **SQL is checked at compile time** (`sqlx::query!`, cache in `.sqlx/`),
//!   except NIP-01 filters, whose shape is the peer's choice: those are built
//!   with `QueryBuilder` and every value is a bound parameter, never text.
//! - Migrations in `migrations/` are append-only.

use std::path::Path;

use hit_event::{Slot, Verified, replaces, slot};
use nostr::prelude::{Event, EventId, Filter, PublicKey, Timestamp};
use sqlx::{
    QueryBuilder, Row, Sqlite, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A row that does not parse back into the event it was stored as. Only
    /// someone editing the file by hand gets here.
    #[error("the store holds a corrupt event")]
    Corrupt,
}

/// What [`Store::insert`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    /// Stored now — new, or replacing an older event in its slot.
    New,
    /// Already held.
    Duplicate,
    /// Its slot holds a newer event; not stored.
    Stale,
    /// An ephemeral kind; never stored.
    Ephemeral,
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn insert(&self, ev: &Verified) -> Result<Stored, StoreError> {
        let slot_d = match slot(ev) {
            Slot::Regular => None,
            Slot::Replaceable => Some(String::new()),
            Slot::Addressable(d) => Some(d),
            Slot::Ephemeral => return Ok(Stored::Ephemeral),
        };
        let id = ev.id.as_bytes().to_vec();
        let pubkey = ev.pubkey.to_bytes().to_vec();
        let created_at = ev.created_at.as_secs() as i64;
        let kind = i64::from(ev.kind.as_u16());

        let mut tx = self.pool.begin().await?;
        if let Some(d) = &slot_d {
            let current = sqlx::query!(
                "SELECT json FROM events WHERE kind = ? AND pubkey = ? AND slot_d = ?",
                kind,
                pubkey,
                d
            )
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(row) = current {
                let current = parse(&row.json)?;
                if current.id == ev.id {
                    return Ok(Stored::Duplicate);
                }
                if !replaces(ev, &current) {
                    return Ok(Stored::Stale);
                }
                let old = current.id.as_bytes().to_vec();
                sqlx::query!("DELETE FROM events WHERE id = ?", old)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        let json = serde_json::to_string(&**ev).map_err(|_| StoreError::Corrupt)?;
        let inserted = sqlx::query!(
            "INSERT OR IGNORE INTO events (id, pubkey, created_at, kind, slot_d, json)
             VALUES (?, ?, ?, ?, ?, ?)",
            id,
            pubkey,
            created_at,
            kind,
            slot_d,
            json
        )
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            return Ok(Stored::Duplicate);
        }
        // Files a message carries (M8): indexed by hash. A malformed set was
        // refused by `hit-group` before this; here nothing is indexed then.
        if ev.kind == hit_event::group::CHAT_MESSAGE
            && let Ok(files) = hit_event::group::attachments_of(ev)
        {
            for a in files {
                let size = i64::try_from(a.size).unwrap_or(i64::MAX);
                sqlx::query!(
                    "INSERT INTO attachments (hash, event_id, size) VALUES (?, ?, ?)",
                    a.hash,
                    id,
                    size
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        for tag in ev.tags.iter() {
            if let [name, value, ..] = tag.as_slice()
                && name.len() == 1
            {
                sqlx::query!(
                    "INSERT INTO tags (event_id, name, value) VALUES (?, ?, ?)",
                    id,
                    name,
                    value
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(Stored::New)
    }

    pub async fn count(&self) -> Result<i64, StoreError> {
        Ok(sqlx::query_scalar!("SELECT COUNT(*) FROM events")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Events matching `filter`, newest first, at most `filter.limit` of them
    /// (all, if none). The caller caps the limit for a peer's request.
    pub async fn query(&self, filter: &Filter) -> Result<Vec<Event>, StoreError> {
        if filter.search.is_some() {
            // NIP-50 search is not supported. Answering as if it were absent
            // would hand back everything else the filter matches.
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT json FROM events WHERE 1=1");
        push_filter(&mut qb, filter);
        qb.push(" ORDER BY created_at DESC, id");
        if let Some(limit) = filter.limit {
            qb.push(" LIMIT ")
                .push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
        }
        qb.build()
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| parse(&r.get::<String, _>(0)))
            .collect()
    }

    /// `(created_at, id)` of every event matching `filter`, ignoring its
    /// limit — the set negentropy reconciles over.
    pub async fn id_times(&self, filter: &Filter) -> Result<Vec<(u64, [u8; 32])>, StoreError> {
        if filter.search.is_some() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT created_at, id FROM events WHERE 1=1");
        push_filter(&mut qb, filter);
        qb.build()
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| {
                let id: Vec<u8> = r.get(1);
                let id: [u8; 32] = id.try_into().map_err(|_| StoreError::Corrupt)?;
                Ok((u64::try_from(r.get::<i64, _>(0)).unwrap_or(0), id))
            })
            .collect()
    }

    /// Starts carrying `group`. Carrying it again is not an error.
    pub async fn carry_group(&self, group: &PublicKey, at: Timestamp) -> Result<(), StoreError> {
        let id = group.to_bytes().to_vec();
        let at = at.as_secs() as i64;
        sqlx::query!(
            "INSERT INTO groups (id, added_at) VALUES (?, ?) ON CONFLICT DO NOTHING",
            id,
            at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every group this node carries, oldest first.
    pub async fn carried_groups(&self) -> Result<Vec<PublicKey>, StoreError> {
        sqlx::query_scalar!("SELECT id FROM groups ORDER BY added_at, id")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|b| PublicKey::from_slice(b).map_err(|_| StoreError::Corrupt))
            .collect()
    }

    /// Has `author` withdrawn `id` (NIP-09)? A deletion held for it means
    /// the event is refused, whichever arrived first.
    pub async fn is_deleted(&self, id: &EventId, author: &PublicKey) -> Result<bool, StoreError> {
        let id = id.to_hex();
        let author = author.to_bytes().to_vec();
        let found = sqlx::query_scalar!(
            "SELECT 1 FROM tags t JOIN events e ON e.id = t.event_id
             WHERE t.name = 'e' AND t.value = ? AND e.kind = 5 AND e.pubkey = ?
             LIMIT 1",
            id,
            author
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(found.is_some())
    }

    /// Every event of `kind` that names `id` in an `e` tag — the moderator
    /// removals (`9005`) a node checks an arriving message against.
    pub async fn naming(
        &self,
        id: &EventId,
        kind: nostr::prelude::Kind,
    ) -> Result<Vec<Event>, StoreError> {
        let id = id.to_hex();
        let kind = i64::from(kind.as_u16());
        sqlx::query_scalar!(
            "SELECT e.json FROM tags t JOIN events e ON e.id = t.event_id
             WHERE t.name = 'e' AND t.value = ? AND e.kind = ?",
            id,
            kind
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|j| parse(j))
        .collect()
    }

    /// Removes these events, whatever they are. The caller has decided —
    /// this module knows nothing of who may remove what.
    pub async fn remove(&self, ids: &[EventId]) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            let id = id.as_bytes().to_vec();
            sqlx::query!("DELETE FROM events WHERE id = ?", id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The ids of `author`'s events of `kinds` in `group` (its `h` tag)
    /// dated in `[from, until)` — what a ban or mute silences.
    pub async fn authored_in(
        &self,
        group: &PublicKey,
        author: &PublicKey,
        kinds: &[nostr::prelude::Kind],
        from: Timestamp,
        until: Option<Timestamp>,
    ) -> Result<Vec<EventId>, StoreError> {
        let mut f = Filter::new()
            .author(*author)
            .kinds(kinds.iter().copied())
            .custom_tag(nostr::prelude::SingleLetterTag::LOWERCASE_H, group.to_hex())
            .since(from);
        if let Some(u) = until {
            if u <= from {
                return Ok(Vec::new());
            }
            f = f.until(u - 1);
        }
        Ok(self.query(&f).await?.into_iter().map(|e| e.id).collect())
    }

    /// Applies `deletion` (kind 5, already stored): removes each target it
    /// names that is **its author's own** and of one of `kinds`. Returns
    /// the ids removed. Anything else it names stays — NIP-09 lets nobody
    /// withdraw somebody else's words.
    pub async fn apply_deletion(
        &self,
        deletion: &Event,
        kinds: &[nostr::prelude::Kind],
    ) -> Result<Vec<EventId>, StoreError> {
        let author = deletion.pubkey.to_bytes().to_vec();
        let mut gone = Vec::new();
        let mut tx = self.pool.begin().await?;
        for t in hit_event::group::targets_of(deletion) {
            let id = t.as_bytes().to_vec();
            let row = sqlx::query!("SELECT kind, pubkey FROM events WHERE id = ?", id)
                .fetch_optional(&mut *tx)
                .await?;
            let Some(row) = row else { continue };
            let kind = u16::try_from(row.kind).map_err(|_| StoreError::Corrupt)?;
            if row.pubkey == author && kinds.iter().any(|k| k.as_u16() == kind) {
                sqlx::query!("DELETE FROM events WHERE id = ?", id)
                    .execute(&mut *tx)
                    .await?;
                gone.push(t);
            }
        }
        tx.commit().await?;
        Ok(gone)
    }

    /// The messages held that carry the file `hash`.
    pub async fn carrying(&self, hash: &str) -> Result<Vec<Event>, StoreError> {
        sqlx::query_scalar!(
            "SELECT e.json FROM attachments a JOIN events e ON e.id = a.event_id WHERE a.hash = ?",
            hash
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|j| parse(j))
        .collect()
    }

    /// Every file some held message carries, with its size: the live set.
    pub async fn attached(&self) -> Result<Vec<(String, u64)>, StoreError> {
        Ok(sqlx::query!("SELECT DISTINCT hash, size FROM attachments")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| (r.hash, u64::try_from(r.size).unwrap_or(0)))
            .collect())
    }

    /// Records a use of `hash` now; `mine` once set stays set.
    pub async fn touch_blob(&self, hash: &str, mine: bool) -> Result<(), StoreError> {
        let now = Timestamp::now().as_secs() as i64;
        let mine = i64::from(mine);
        sqlx::query!(
            "INSERT INTO blob_use (hash, last_used, mine) VALUES (?, ?, ?)
             ON CONFLICT (hash) DO UPDATE SET last_used = excluded.last_used,
                 mine = MAX(blob_use.mine, excluded.mine)",
            hash,
            now,
            mine
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The files to keep within `cap` bytes (M8): every file of ours that a
    /// held message still carries, then the most recently used of the rest
    /// while they fit. A file no held message carries is never kept.
    pub async fn blobs_to_keep(&self, cap: u64) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query!(
            "SELECT a.hash AS \"hash!: String\", MAX(a.size) AS \"size!: i64\", COALESCE(u.mine, 0) AS \"mine!: i64\",
                    COALESCE(u.last_used, 0) AS \"last_used!: i64\"
             FROM attachments a LEFT JOIN blob_use u ON u.hash = a.hash
             GROUP BY a.hash
             ORDER BY mine DESC, last_used DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        let mut used = 0u64;
        let mut keep = Vec::new();
        for r in rows {
            let size = u64::try_from(r.size).unwrap_or(u64::MAX);
            if r.mine != 0 || used.saturating_add(size) <= cap {
                used = used.saturating_add(size);
                keep.push(r.hash);
            }
        }
        Ok(keep)
    }

    /// Files of ours used (attached, posted) at or after `since` — kept by
    /// the collector even before their message is sent.
    pub async fn mine_since(&self, since: u64) -> Result<Vec<String>, StoreError> {
        let since = since as i64;
        Ok(sqlx::query_scalar!(
            "SELECT hash FROM blob_use WHERE mine = 1 AND last_used >= ?",
            since
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Lets `person` be handed the file `hash` (a DM file, M8b).
    pub async fn grant_blob(&self, hash: &str, person: &PublicKey) -> Result<(), StoreError> {
        let person = person.to_bytes().to_vec();
        sqlx::query!(
            "INSERT INTO blob_grants (hash, person) VALUES (?, ?) ON CONFLICT DO NOTHING",
            hash,
            person
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Who may be handed the file `hash` by grant.
    pub async fn granted(&self, hash: &str) -> Result<Vec<PublicKey>, StoreError> {
        sqlx::query_scalar!("SELECT person FROM blob_grants WHERE hash = ?", hash)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|b| PublicKey::from_slice(b).map_err(|_| StoreError::Corrupt))
            .collect()
    }

    /// Every granted file: kept by the collector as long as the grant is.
    pub async fn granted_hashes(&self) -> Result<Vec<String>, StoreError> {
        Ok(sqlx::query_scalar!("SELECT DISTINCT hash FROM blob_grants")
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn by_ids(&self, ids: &[EventId]) -> Result<Vec<Event>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.query(&Filter::new().ids(ids.iter().copied())).await
    }
}

fn parse(json: &str) -> Result<Event, StoreError> {
    serde_json::from_str(json).map_err(|_| StoreError::Corrupt)
}

/// Appends a NIP-01 filter as `AND …` clauses. Every value is bound. An
/// empty list is `IN ()`, which SQLite accepts and which matches nothing.
fn push_filter(qb: &mut QueryBuilder<Sqlite>, f: &Filter) {
    if let Some(ids) = &f.ids {
        qb.push(" AND id IN (");
        let mut sep = qb.separated(", ");
        for id in ids {
            sep.push_bind(id.as_bytes().to_vec());
        }
        sep.push_unseparated(")");
    }
    if let Some(authors) = &f.authors {
        qb.push(" AND pubkey IN (");
        let mut sep = qb.separated(", ");
        for a in authors {
            sep.push_bind(a.to_bytes().to_vec());
        }
        sep.push_unseparated(")");
    }
    if let Some(kinds) = &f.kinds {
        qb.push(" AND kind IN (");
        let mut sep = qb.separated(", ");
        for k in kinds {
            sep.push_bind(i64::from(k.as_u16()));
        }
        sep.push_unseparated(")");
    }
    if let Some(since) = f.since {
        qb.push(" AND created_at >= ")
            .push_bind(since.as_secs() as i64);
    }
    if let Some(until) = f.until {
        qb.push(" AND created_at <= ")
            .push_bind(until.as_secs() as i64);
    }
    for (tag, values) in &f.generic_tags {
        qb.push(" AND id IN (SELECT event_id FROM tags WHERE name = ")
            .push_bind(tag.to_string())
            .push(" AND value IN (");
        let mut sep = qb.separated(", ");
        for v in values {
            sep.push_bind(v.clone());
        }
        sep.push_unseparated("))");
    }
}
