//! Storing and reading messages.
//!
//! `content` is a plaintext `TEXT` column, deliberately (PLAN §10). It is
//! never logged: plaintext in the database is a documented trust model,
//! plaintext in a log file that gets pasted into a bug report is an accident.
//!
//! Ids are UUIDv7, so `ORDER BY id DESC` is newest-first and a pagination
//! cursor is just an id. There is no separate timestamp column on a message
//! and no sequence for two writers to fight over.

use std::collections::BTreeMap;

use he_proto::Message;
use he_proto::limits;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{new_id, now_unix};
use crate::error::{Result, ServerError};

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

/// One message by id, with its author's name joined in.
pub(crate) async fn by_id(pool: &SqlitePool, id: &str) -> Result<Message> {
    sqlx::query_as!(
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
           WHERE m.id = ?1"#,
        id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(ServerError::UnknownMessage)
}

/// Rewrites a message, and stamps it as edited.
///
/// Only the author may. A message that has been deleted is gone as far as
/// everyone else is concerned, so it cannot be edited back into existence.
pub async fn edit(pool: &SqlitePool, author_id: &str, id: &str, content: &str) -> Result<Message> {
    limits::validate_message_content(content)?;
    let existing = by_id(pool, id).await?;
    if existing.author_id != author_id {
        return Err(ServerError::Forbidden);
    }
    if existing.deleted_at.is_some() {
        return Err(ServerError::UnknownMessage);
    }

    let edited_at = now_unix();
    sqlx::query!(
        "UPDATE messages SET content = ?1, edited_at = ?2 WHERE id = ?3",
        content,
        edited_at,
        id,
    )
    .execute(pool)
    .await?;

    Ok(Message {
        content: content.to_owned(),
        edited_at: Some(edited_at),
        ..existing
    })
}

/// Withdraws a message. Returns when it was withdrawn.
///
/// A **soft** delete, because every client that has the message on screen has
/// to be told to take it off — a row that simply vanished would stay on every
/// screen that already had it, which is the opposite of deleting it.
///
/// The content does not survive. Keeping the text while claiming the message
/// is deleted would make "deleted" mean "hidden", and the host reads this
/// database in plaintext by design (PLAN §10): the only way a delete means
/// anything here is if the text is actually gone.
pub async fn delete(pool: &SqlitePool, author_id: &str, id: &str) -> Result<(Message, i64)> {
    let existing = by_id(pool, id).await?;
    if existing.author_id != author_id {
        return Err(ServerError::Forbidden);
    }
    if let Some(deleted_at) = existing.deleted_at {
        // Already gone. Idempotent rather than an error: a client that missed
        // the event and tried again is not wrong.
        return Ok((existing, deleted_at));
    }

    let deleted_at = now_unix();
    sqlx::query!(
        "UPDATE messages SET content = '', deleted_at = ?1 WHERE id = ?2",
        deleted_at,
        id,
    )
    .execute(pool)
    .await?;

    Ok((
        Message {
            content: String::new(),
            deleted_at: Some(deleted_at),
            ..existing
        },
        deleted_at,
    ))
}

/// What a client missed while it was away.
pub struct Resumed {
    /// Oldest first, across every channel: the order a client applies them in.
    pub messages: Vec<Message>,
    /// Channels whose gap did not fit. The client pages those with `backfill`
    /// rather than believing it is caught up.
    pub truncated: Vec<String>,
}

/// Everything newer than each cursor, plus anything older that changed.
///
/// The second half is the part that is easy to forget: a message edited or
/// deleted while this client was offline keeps its id, so it is *older* than
/// the cursor and no amount of "give me what is new" would ever mention it.
/// Without `since` it would stay on that client's screen, with its original
/// text, forever.
pub async fn resume(
    pool: &SqlitePool,
    cursors: &BTreeMap<String, String>,
    since: Option<i64>,
) -> Result<Resumed> {
    let mut messages = Vec::new();
    let mut truncated = Vec::new();
    let mut budget = limits::RESUME_MAX_MESSAGES;

    for (channel_id, last_id) in cursors {
        if budget == 0 {
            truncated.push(channel_id.clone());
            continue;
        }
        // One more than the cap, so a full page is distinguishable from a page
        // that happened to land exactly on the limit.
        let want = i64::from(limits::RESUME_PER_CHANNEL_LIMIT.min(budget)) + 1;
        let mut fresh = sqlx::query_as!(
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
               WHERE m.channel_id = ?1 AND m.id > ?2
               ORDER BY m.id
               LIMIT ?3"#,
            channel_id,
            last_id,
            want,
        )
        .fetch_all(pool)
        .await?;

        if fresh.len() as i64 == want {
            // More than we are willing to carry. Dropping the overflow and
            // saying so is the honest answer; handing back a prefix silently
            // would leave a hole the client could not know about.
            fresh.truncate(want as usize - 1);
            truncated.push(channel_id.clone());
        }

        budget = budget.saturating_sub(fresh.len() as u32);
        messages.extend(fresh);

        if let Some(since) = since {
            let changed = sqlx::query_as!(
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
                     AND m.id <= ?2
                     AND (m.edited_at >= ?3 OR m.deleted_at >= ?3)
                   ORDER BY m.id
                   LIMIT ?4"#,
                channel_id,
                last_id,
                since,
                want,
            )
            .fetch_all(pool)
            .await?;
            budget = budget.saturating_sub(changed.len() as u32);
            messages.extend(changed);
        }
    }

    // One id-ordered stream out of per-channel pages, so a client applies them
    // in the order they happened rather than channel by channel.
    messages.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Resumed {
        messages,
        truncated,
    })
}
