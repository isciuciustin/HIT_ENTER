//! The local message mirror: `mirror.db`.
//!
//! Every message this client has ever seen is written here, in plaintext
//! (PLAN §10). That is what makes the app open on a train: the channel rail,
//! the scrollback and the member names all come from this file, and the
//! network is only ever an *update*, never a prerequisite.
//!
//! One mirror holds every server the client knows, keyed by `endpoint_id`.
//! Removing a server deletes its mirror, which the schema's `ON DELETE
//! CASCADE` does in one statement.
//!
//! Writes are idempotent. A message can arrive twice — once as a live event
//! and once in a backfill page that overlaps it — and `INSERT OR REPLACE` on
//! `(endpoint_id, id)` is what makes that a no-op rather than a duplicate
//! bubble.

use std::collections::BTreeMap;
use std::path::Path;

use he_proto::{Channel, Message};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::Result;

/// Embedded at compile time, so a released binary carries its own schema.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

const MAX_CONNECTIONS: u32 = 4;
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A space this client knows about, as the server rail renders it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MirroredServer {
    pub endpoint_id: String,
    pub name: String,
    pub relay_url: Option<String>,
    /// Which account this client uses here. Accounts are per-server (PLAN §3),
    /// so this is not a global identity and there is no global one to store.
    pub username: String,
    /// That account's id on that server, once a handshake has told us.
    ///
    /// Needed to answer "is this message mine?" without a name comparison —
    /// which is what keeps your own messages out of your own unread count.
    pub user_id: Option<String>,
    pub added_at: i64,
    pub last_seen: Option<i64>,
}

/// A message composed while offline, waiting for a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub nonce: String,
    pub channel_id: String,
    pub content: String,
    pub created_at: i64,
}

/// The client's local database.
#[derive(Debug, Clone)]
pub struct Mirror {
    pool: SqlitePool,
}

impl Mirror {
    /// Opens (creating if needed) the mirror at `path` and migrates it.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            // The schema's ON DELETE CASCADE is how "forget this server"
            // deletes its mirror. SQLite defaults this off, per connection.
            .foreign_keys(true)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options)
            .await?;

        MIGRATOR.run(&pool).await?;
        restrict_permissions(path);

        Ok(Self { pool })
    }

    /// An in-memory mirror, for tests.
    pub async fn in_memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---- servers ----------------------------------------------------------

    /// Records a server, or refreshes what we know about one.
    ///
    /// Called after every successful handshake: the name and the relay can
    /// both change under a stable `endpoint_id`, and the id is the only part
    /// an invite ticket guarantees.
    pub async fn upsert_server(
        &self,
        endpoint_id: &str,
        name: &str,
        username: &str,
        user_id: Option<&str>,
        relay_url: Option<&str>,
    ) -> Result<()> {
        let now = now_unix();
        sqlx::query!(
            "INSERT INTO servers
                 (endpoint_id, name, relay_url, username, user_id, added_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT (endpoint_id) DO UPDATE SET
                 name      = excluded.name,
                 relay_url = COALESCE(excluded.relay_url, servers.relay_url),
                 username  = excluded.username,
                 user_id   = COALESCE(excluded.user_id, servers.user_id),
                 last_seen = excluded.last_seen",
            endpoint_id,
            name,
            relay_url,
            username,
            user_id,
            now,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn servers(&self) -> Result<Vec<MirroredServer>> {
        let rows = sqlx::query_as!(
            MirroredServer,
            "SELECT endpoint_id, name, relay_url, username, user_id, added_at, last_seen
             FROM servers ORDER BY added_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn server(&self, endpoint_id: &str) -> Result<Option<MirroredServer>> {
        let row = sqlx::query_as!(
            MirroredServer,
            "SELECT endpoint_id, name, relay_url, username, user_id, added_at, last_seen
             FROM servers WHERE endpoint_id = ?1",
            endpoint_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Removes a server **and its entire mirror**.
    ///
    /// Leaving the history behind would be the surprising choice: "remove
    /// server" in a local-first app has to mean the messages go too, or the
    /// disk quietly keeps a conversation the user thought they deleted.
    pub async fn forget_server(&self, endpoint_id: &str) -> Result<()> {
        sqlx::query!("DELETE FROM servers WHERE endpoint_id = ?1", endpoint_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- channels ---------------------------------------------------------

    /// Replaces the cached channel list for a server.
    ///
    /// A replace rather than an upsert, because a channel deleted on the
    /// server has to disappear here too — and `Ready` carries the whole list,
    /// so there is never a partial one to merge.
    pub async fn replace_channels(&self, endpoint_id: &str, channels: &[Channel]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "DELETE FROM cached_channels WHERE endpoint_id = ?1",
            endpoint_id
        )
        .execute(&mut *tx)
        .await?;

        for channel in channels {
            sqlx::query!(
                "INSERT INTO cached_channels (endpoint_id, id, name, topic, position)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                endpoint_id,
                channel.id,
                channel.name,
                channel.topic,
                channel.position,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// The cached channel list. Answers with the network off.
    pub async fn channels(&self, endpoint_id: &str) -> Result<Vec<Channel>> {
        let rows = sqlx::query_as!(
            Channel,
            "SELECT id, name, topic, position FROM cached_channels
             WHERE endpoint_id = ?1 ORDER BY position, id",
            endpoint_id,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ---- members ----------------------------------------------------------

    /// Replaces the cached member list for a server.
    ///
    /// A replace, like the channels, and for the same reason: `Ready` carries
    /// the whole roster, so there is never a partial one to merge, and an
    /// account that left has to disappear from here too.
    pub async fn replace_members(
        &self,
        endpoint_id: &str,
        members: &[he_proto::Member],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "DELETE FROM cached_members WHERE endpoint_id = ?1",
            endpoint_id
        )
        .execute(&mut *tx)
        .await?;

        for member in members {
            sqlx::query!(
                "INSERT INTO cached_members
                     (endpoint_id, id, username, display_name, is_owner, banned)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                endpoint_id,
                member.id,
                member.username,
                member.display_name,
                member.is_owner,
                member.banned,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// The cached member list. Answers with the network off.
    pub async fn members(&self, endpoint_id: &str) -> Result<Vec<he_proto::Member>> {
        let rows = sqlx::query_as!(
            he_proto::Member,
            r#"SELECT id, username, display_name,
                      is_owner AS "is_owner!: bool",
                      banned   AS "banned!: bool"
               FROM cached_members WHERE endpoint_id = ?1
               ORDER BY is_owner DESC, username COLLATE NOCASE"#,
            endpoint_id,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ---- messages ---------------------------------------------------------

    /// Stores a message, overwriting any earlier copy of it.
    ///
    /// The same message arrives twice whenever a backfill page overlaps
    /// something already seen live. Replacing on the primary key makes that a
    /// no-op instead of a duplicate.
    pub async fn record_message(&self, endpoint_id: &str, message: &Message) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        record_one(&mut conn, endpoint_id, message).await
    }

    /// Stores a page of messages in one transaction.
    pub async fn record_messages(&self, endpoint_id: &str, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for message in messages {
            record_one(&mut tx, endpoint_id, message).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A page of cached history, newest first, ending just before `before`.
    ///
    /// Same shape as the server's `backfill`, deliberately: the UI pages
    /// through this and only reaches for the network when it runs out, so both
    /// sources have to answer the same question the same way.
    pub async fn messages(
        &self,
        endpoint_id: &str,
        channel_id: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>> {
        let limit = i64::from(limit);
        // An empty sentinel never matches a real id, because ids are UUIDv7.
        let before = before.unwrap_or("");
        let rows = sqlx::query_as!(
            Message,
            r#"SELECT id            AS "id!",
                      channel_id    AS "channel_id!",
                      author_id     AS "author_id!",
                      author_name   AS "author_name!",
                      content       AS "content!",
                      edited_at,
                      deleted_at
               FROM cached_messages
               WHERE endpoint_id = ?1
                 AND channel_id = ?2
                 AND (?3 = '' OR id < ?3)
               ORDER BY id DESC
               LIMIT ?4"#,
            endpoint_id,
            channel_id,
            before,
            limit,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Applies an edit to a message we may or may not already hold.
    ///
    /// The server sends the whole row rather than a patch, so this is the same
    /// idempotent write as any other message — which means a client that never
    /// saw the original still ends up with the right text.
    pub async fn apply_edit(&self, endpoint_id: &str, message: &Message) -> Result<()> {
        self.record_message(endpoint_id, message).await
    }

    /// Applies a deletion, taking the text with it.
    ///
    /// Blanking the content matters more here than on the server: this file is
    /// the *user's* copy, and a delete that left the words on their disk would
    /// make "deleted" mean "hidden" on the one machine they control.
    ///
    /// A no-op if the message was never mirrored. There is nothing to withdraw
    /// and nothing to remember — the next backfill will carry the tombstone.
    pub async fn apply_delete(&self, endpoint_id: &str, id: &str, deleted_at: i64) -> Result<()> {
        sqlx::query!(
            "UPDATE cached_messages SET content = '', deleted_at = ?1
             WHERE endpoint_id = ?2 AND id = ?3",
            deleted_at,
            endpoint_id,
            id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// How many messages are cached for a channel. Used to decide whether the
    /// pane has anything to show before the network answers.
    pub async fn message_count(&self, endpoint_id: &str, channel_id: &str) -> Result<i64> {
        let count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM cached_messages WHERE endpoint_id = ?1 AND channel_id = ?2",
            endpoint_id,
            channel_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    // ---- sync state -------------------------------------------------------

    /// Per-channel resume cursors: the newest message this client holds in
    /// each channel, which is exactly what `resume` asks the server about.
    pub async fn cursors(&self, endpoint_id: &str) -> Result<BTreeMap<String, String>> {
        let rows = sqlx::query!(
            "SELECT channel_id, last_id FROM sync_state WHERE endpoint_id = ?1",
            endpoint_id,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.channel_id, row.last_id))
            .collect())
    }

    /// When this client last finished a sync with a server.
    ///
    /// The other half of `resume`: the cursors say what is new, this says how
    /// far back to look for something that *changed*.
    pub async fn resumed_at(&self, endpoint_id: &str) -> Result<Option<i64>> {
        let row = sqlx::query_scalar!(
            "SELECT resumed_at FROM sync_marks WHERE endpoint_id = ?1",
            endpoint_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Records that this client is caught up with a server as of now.
    ///
    /// Written *after* everything a resume returned has been stored, never
    /// before: a mark that ran ahead of the writes would skip whatever the
    /// crash in between lost.
    pub async fn mark_resumed(&self, endpoint_id: &str) -> Result<()> {
        let now = now_unix();
        sqlx::query!(
            "INSERT INTO sync_marks (endpoint_id, resumed_at) VALUES (?1, ?2)
             ON CONFLICT (endpoint_id) DO UPDATE SET resumed_at = excluded.resumed_at",
            endpoint_id,
            now,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---- read markers -----------------------------------------------------

    /// Marks a channel read up to and including `last_read_id`.
    ///
    /// Only ever moves forward. Scrolling back through history must not make
    /// a channel unread again, and two windows on the same account would
    /// otherwise take turns undoing each other.
    pub async fn mark_read(
        &self,
        endpoint_id: &str,
        channel_id: &str,
        last_read_id: &str,
    ) -> Result<()> {
        sqlx::query!(
            "INSERT INTO read_state (endpoint_id, channel_id, last_read_id)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (endpoint_id, channel_id) DO UPDATE SET
                 last_read_id = MAX(read_state.last_read_id, excluded.last_read_id)",
            endpoint_id,
            channel_id,
            last_read_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Marks every channel read up to whatever is currently mirrored.
    ///
    /// Called when a space is *joined*, so that arriving somewhere new does
    /// not light up every channel with a badge counting history the user was
    /// never party to.
    pub async fn mark_all_read(&self, endpoint_id: &str) -> Result<()> {
        sqlx::query!(
            "INSERT INTO read_state (endpoint_id, channel_id, last_read_id)
             SELECT endpoint_id, channel_id, MAX(id)
             FROM cached_messages WHERE endpoint_id = ?1
             GROUP BY channel_id
             ON CONFLICT (endpoint_id, channel_id) DO UPDATE SET
                 last_read_id = MAX(read_state.last_read_id, excluded.last_read_id)",
            endpoint_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Everything unread on a server, for the dot on the server rail.
    pub async fn unread_total(&self, endpoint_id: &str) -> Result<i64> {
        Ok(self
            .unread(endpoint_id)
            .await?
            .into_iter()
            .map(|(_, count)| count)
            .sum())
    }

    /// Unread counts per channel, for the badge in the rail.
    ///
    /// Your own messages never count: a badge that goes up when *you* say
    /// something is a badge nobody can ever clear. Deleted messages do not
    /// count either — there is nothing left to read.
    pub async fn unread(&self, endpoint_id: &str) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query!(
            r#"SELECT c.id AS "channel_id!", COUNT(m.id) AS "unread!: i64"
               FROM cached_channels c
               LEFT JOIN read_state r
                      ON r.endpoint_id = c.endpoint_id AND r.channel_id = c.id
               LEFT JOIN cached_messages m
                      ON m.endpoint_id = c.endpoint_id
                     AND m.channel_id  = c.id
                     AND m.deleted_at IS NULL
                     AND m.id > COALESCE(r.last_read_id, '')
                     AND m.author_id <> COALESCE(
                           (SELECT user_id FROM servers WHERE endpoint_id = c.endpoint_id), '')
               WHERE c.endpoint_id = ?1
               GROUP BY c.id"#,
            endpoint_id,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.channel_id, row.unread))
            .collect())
    }

    // ---- outbox -----------------------------------------------------------

    /// Records a message that has been composed but not acknowledged.
    ///
    /// Written before the send is attempted, so that a message typed into a
    /// connection that is already dead is still on disk. M5 drains this on
    /// reconnect; M3 only has to make sure nothing is lost in the meantime.
    pub async fn queue(
        &self,
        nonce: &str,
        endpoint_id: &str,
        channel_id: &str,
        content: &str,
    ) -> Result<()> {
        let now = now_unix();
        sqlx::query!(
            "INSERT OR REPLACE INTO outbox (nonce, endpoint_id, channel_id, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            nonce,
            endpoint_id,
            channel_id,
            content,
            now,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clears an entry once the server has acknowledged it.
    pub async fn dequeue(&self, nonce: &str) -> Result<()> {
        sqlx::query!("DELETE FROM outbox WHERE nonce = ?1", nonce)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Everything still waiting to be sent, oldest first.
    pub async fn pending(&self, endpoint_id: &str) -> Result<Vec<Queued>> {
        let rows = sqlx::query_as!(
            Queued,
            "SELECT nonce, channel_id, content, created_at FROM outbox
             WHERE endpoint_id = ?1 ORDER BY created_at, nonce",
            endpoint_id,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

/// Stores one message and advances the channel's resume cursor.
///
/// The cursor only ever moves forward: a backfill page is full of ids *older*
/// than what we have, and letting one of those overwrite the cursor would make
/// a reconnect re-download everything since.
async fn record_one(
    conn: &mut sqlx::SqliteConnection,
    endpoint_id: &str,
    message: &Message,
) -> Result<()> {
    sqlx::query!(
        "INSERT OR REPLACE INTO cached_messages
             (endpoint_id, id, channel_id, author_id, author_name, content, edited_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        endpoint_id,
        message.id,
        message.channel_id,
        message.author_id,
        message.author_name,
        message.content,
        message.edited_at,
        message.deleted_at,
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "INSERT INTO sync_state (endpoint_id, channel_id, last_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (endpoint_id, channel_id) DO UPDATE SET
             last_id = MAX(sync_state.last_id, excluded.last_id)",
        endpoint_id,
        message.channel_id,
        message.id,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// The mirror holds every message this client has ever seen, in plaintext.
/// That is a documented trust model for *its owner*, not for every other
/// account on a shared machine.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            let sidecar = std::path::PathBuf::from(sidecar);
            if sidecar.exists()
                && let Err(err) =
                    std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600))
            {
                tracing::warn!(path = %sidecar.display(), %err, "could not restrict mirror permissions");
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, channel: &str, content: &str) -> Message {
        Message {
            id: id.to_owned(),
            channel_id: channel.to_owned(),
            author_id: "u1".to_owned(),
            author_name: "justin".to_owned(),
            content: content.to_owned(),
            edited_at: None,
            deleted_at: None,
        }
    }

    async fn seeded() -> Mirror {
        let mirror = Mirror::in_memory().await.expect("open");
        mirror
            .upsert_server("server-key", "Test Space", "justin", Some("u1"), None)
            .await
            .expect("upsert");
        mirror
    }

    #[tokio::test]
    async fn a_mirror_answers_with_the_network_off() {
        // The whole point of the file: close the app, unplug, open it again,
        // and the conversation is still there.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mirror.db");

        {
            let mirror = Mirror::open(&path).await.expect("create");
            mirror
                .upsert_server("server-key", "Test Space", "justin", Some("u1"), None)
                .await
                .expect("upsert");
            mirror
                .replace_channels(
                    "server-key",
                    &[Channel {
                        id: "c1".into(),
                        name: "general".into(),
                        topic: None,
                        position: 0,
                    }],
                )
                .await
                .expect("channels");
            mirror
                .record_message("server-key", &message("m1", "c1", "hit enter"))
                .await
                .expect("record");
        }

        let reopened = Mirror::open(&path).await.expect("reopen");
        let servers = reopened.servers().await.expect("servers");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "Test Space");
        // A message pane with no channel rail is not an app you can open on a
        // train, so the channels have to survive too.
        assert_eq!(
            reopened
                .channels("server-key")
                .await
                .expect("channels")
                .len(),
            1
        );
        let history = reopened
            .messages("server-key", "c1", None, 50)
            .await
            .expect("messages");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "hit enter");
    }

    #[tokio::test]
    async fn the_same_message_twice_is_one_message() {
        // A backfill page overlapping a live event is the normal case, not an
        // edge case: it happens on every reconnect.
        let mirror = seeded().await;
        let live = message("m1", "c1", "hit enter");
        mirror
            .record_message("server-key", &live)
            .await
            .expect("live");
        mirror
            .record_messages("server-key", std::slice::from_ref(&live))
            .await
            .expect("backfilled");

        assert_eq!(
            mirror
                .message_count("server-key", "c1")
                .await
                .expect("count"),
            1
        );
    }

    #[tokio::test]
    async fn history_pages_backwards_without_skipping_or_repeating() {
        let mirror = seeded().await;
        let ids = ["m1", "m2", "m3", "m4", "m5"];
        for id in ids {
            mirror
                .record_message("server-key", &message(id, "c1", id))
                .await
                .expect("record");
        }

        let newest = mirror
            .messages("server-key", "c1", None, 2)
            .await
            .expect("page 1");
        assert_eq!(
            newest.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m5", "m4"],
            "newest first, like the server's backfill"
        );

        // The cursor is exclusive, so paging with the oldest id in hand does
        // not re-fetch it and does not step over the next one.
        let older = mirror
            .messages("server-key", "c1", Some("m4"), 2)
            .await
            .expect("page 2");
        assert_eq!(
            older.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m3", "m2"]
        );
    }

    #[tokio::test]
    async fn a_resume_cursor_only_moves_forward() {
        // Backfilling old history must not rewind the cursor, or the next
        // reconnect re-downloads everything since.
        let mirror = seeded().await;
        mirror
            .record_message("server-key", &message("m9", "c1", "newest"))
            .await
            .expect("live");
        mirror
            .record_messages("server-key", &[message("m1", "c1", "ancient")])
            .await
            .expect("backfill");

        let cursors = mirror.cursors("server-key").await.expect("cursors");
        assert_eq!(cursors.get("c1").map(String::as_str), Some("m9"));
    }

    #[tokio::test]
    async fn forgetting_a_server_deletes_its_mirror() {
        // "Remove server" in a local-first app has to take the messages with
        // it, or the disk quietly keeps a conversation the user deleted.
        let mirror = seeded().await;
        mirror
            .record_message("server-key", &message("m1", "c1", "hit enter"))
            .await
            .expect("record");
        mirror
            .queue("n1", "server-key", "c1", "unsent")
            .await
            .expect("queue");

        mirror.forget_server("server-key").await.expect("forget");

        assert!(mirror.servers().await.expect("servers").is_empty());
        assert_eq!(
            mirror
                .message_count("server-key", "c1")
                .await
                .expect("count"),
            0
        );
        assert!(
            mirror
                .pending("server-key")
                .await
                .expect("pending")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_outbox_holds_a_message_until_it_is_acknowledged() {
        let mirror = seeded().await;
        mirror
            .queue("n1", "server-key", "c1", "composed offline")
            .await
            .expect("queue");

        let pending = mirror.pending("server-key").await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "composed offline");

        mirror.dequeue("n1").await.expect("dequeue");
        assert!(
            mirror
                .pending("server-key")
                .await
                .expect("pending")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_renamed_space_keeps_its_history() {
        // The name and the relay can both change under a stable EndpointId.
        // Only the id is what an invite ticket guarantees (PLAN §4).
        let mirror = seeded().await;
        mirror
            .record_message("server-key", &message("m1", "c1", "hit enter"))
            .await
            .expect("record");

        mirror
            .upsert_server(
                "server-key",
                "Renamed Space",
                "justin",
                Some("u1"),
                Some("https://relay"),
            )
            .await
            .expect("rename");

        let server = mirror
            .server("server-key")
            .await
            .expect("server")
            .expect("present");
        assert_eq!(server.name, "Renamed Space");
        assert_eq!(server.relay_url.as_deref(), Some("https://relay"));
        assert_eq!(
            mirror
                .message_count("server-key", "c1")
                .await
                .expect("count"),
            1
        );
    }

    #[tokio::test]
    async fn channels_are_replaced_not_merged() {
        // A channel deleted on the server has to disappear here too, and
        // `Ready` always carries the whole list.
        let mirror = seeded().await;
        let general = Channel {
            id: "c1".into(),
            name: "general".into(),
            topic: None,
            position: 0,
        };
        let scratch = Channel {
            id: "c2".into(),
            name: "scratch".into(),
            topic: None,
            position: 1,
        };
        mirror
            .replace_channels("server-key", &[general.clone(), scratch])
            .await
            .expect("two");
        mirror
            .replace_channels("server-key", &[general])
            .await
            .expect("one");

        let channels = mirror.channels("server-key").await.expect("channels");
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "general");
    }
}
