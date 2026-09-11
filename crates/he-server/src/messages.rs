//! Storing and reading messages.
//!
//! `content` is a plaintext `TEXT` column, deliberately (PLAN §10). It is
//! never logged: plaintext in the database is a documented trust model,
//! plaintext in a log file that gets pasted into a bug report is an accident.
//!
//! Ids are UUIDv7, so `ORDER BY id DESC` is newest-first and a pagination
//! cursor is just an id. There is no separate timestamp column on a message
//! and no sequence for two writers to fight over.

use he_proto::Message;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::new_id;
use crate::error::Result;

/// Inserts a message and returns it in the shape the wire wants.
///
/// `author_name` is denormalised into the returned [`Message`] but not into
/// the row: the database keeps `author_id` and joins, so renaming an account
/// does not rewrite its history.
pub(crate) async fn insert(
    conn: &mut SqliteConnection,
    channel_id: &str,
    author_id: &str,
    author_name: &str,
    content: &str,
) -> Result<Message> {
    let id = new_id();

    sqlx::query!(
        "INSERT INTO messages (id, channel_id, author_id, content, edited_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL)",
        id,
        channel_id,
        author_id,
        content,
    )
    .execute(&mut *conn)
    .await?;

    Ok(Message {
        id,
        channel_id: channel_id.to_owned(),
        author_id: author_id.to_owned(),
        author_name: author_name.to_owned(),
        content: content.to_owned(),
        edited_at: None,
        deleted_at: None,
    })
}

/// A page of history, newest first, ending just before `before`.
///
/// `before` is exclusive so a client can pass the oldest id it already has and
/// page backwards without re-receiving it or skipping one.
pub async fn backfill(
    pool: &SqlitePool,
    channel_id: &str,
    before: Option<&str>,
    limit: u32,
) -> Result<Vec<Message>> {
    // Bounded by `limits::BACKFILL_MAX_LIMIT` before it reaches this far; the
    // cast is safe because that maximum is three digits.
    let limit = i64::from(limit);

    // One query with a sentinel rather than two near-identical ones. An empty
    // `before` never matches a real id, because ids are UUIDv7.
    let before = before.unwrap_or("");
    let rows = sqlx::query_as!(
        Message,
        r#"SELECT m.id            AS "id!",
                  m.channel_id    AS "channel_id!",
                  m.author_id     AS "author_id!",
                  u.username      AS "author_name!",
                  m.content       AS "content!",
                  m.edited_at,
                  m.deleted_at
           FROM messages m
           JOIN users u ON u.id = m.author_id
           WHERE m.channel_id = ?1
             AND (?2 = '' OR m.id < ?2)
           ORDER BY m.id DESC
           LIMIT ?3"#,
        channel_id,
        before,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
