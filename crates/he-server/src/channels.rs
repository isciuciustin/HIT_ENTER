//! Channels, and the one that exists before anybody makes one.
//!
//! A space with no channel has nowhere to put a message, so the first run
//! creates `#general` — and [`delete`] refuses to take the last one away for
//! exactly the same reason. That invariant is the whole of this module's
//! opinion; who is allowed to call these is the caller's business, because
//! nothing here knows what an owner is.

use he_proto::Channel;
use he_proto::limits;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{new_id, now_unix};
use crate::error::{Result, ServerError};

/// The channel a brand new space starts with.
pub const DEFAULT_CHANNEL_NAME: &str = "general";

pub(crate) async fn create(
    conn: &mut SqliteConnection,
    name: &str,
    topic: Option<&str>,
    position: i64,
) -> Result<Channel> {
    limits::validate_channel_name(name)?;
    let id = new_id();
    let created_at = now_unix();

    sqlx::query!(
        "INSERT INTO channels (id, name, topic, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        id,
        name,
        topic,
        position,
        created_at,
    )
    .execute(&mut *conn)
    .await?;

    Ok(Channel {
        id,
        name: name.to_owned(),
        topic: topic.map(str::to_owned),
        position,
    })
}

/// Every channel, in the order a client should render them.
pub async fn list(pool: &SqlitePool) -> Result<Vec<Channel>> {
    let rows = sqlx::query_as!(
        Channel,
        "SELECT id, name, topic, position FROM channels ORDER BY position, id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Creates a channel at the end of the list.
///
/// `position` is handed out rather than chosen by the caller so that two
/// channels made in the same second cannot land on the same number and sort
/// arbitrarily against each other.
pub async fn append(pool: &SqlitePool, name: &str, topic: Option<&str>) -> Result<Channel> {
    if let Some(topic) = topic {
        limits::validate_channel_topic(topic)?;
    }
    let next = sqlx::query_scalar!(r#"SELECT COALESCE(MAX(position), -1) + 1 FROM channels"#)
        .fetch_one(pool)
        .await?;

    let mut conn = pool.acquire().await?;
    create(&mut conn, name, topic, next).await
}

/// Deletes a channel and, by the schema's cascade, every message in it.
///
/// Refuses the last one. A space with no channel has nowhere to put a message,
/// so the next person to speak would find a dead end — and the owner who did
/// it would have no way back but editing the database by hand.
pub async fn delete(pool: &SqlitePool, channel_id: &str) -> Result<()> {
    require(pool, channel_id).await?;

    let remaining = sqlx::query_scalar!("SELECT COUNT(*) FROM channels")
        .fetch_one(pool)
        .await?;
    if remaining <= 1 {
        return Err(ServerError::LastChannel);
    }

    // ON DELETE CASCADE on messages.channel_id takes the history with it. That
    // is the honest behaviour: history in a channel nobody can open is not
    // kept, it is stranded.
    sqlx::query!("DELETE FROM channels WHERE id = ?1", channel_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Resolves a channel id, failing with [`ServerError::UnknownChannel`] rather
/// than letting a bad id become a foreign-key error three layers down.
pub(crate) async fn require(pool: &SqlitePool, channel_id: &str) -> Result<()> {
    let found = sqlx::query_scalar!("SELECT 1 FROM channels WHERE id = ?1", channel_id)
        .fetch_optional(pool)
        .await?;
    found.map(|_| ()).ok_or(ServerError::UnknownChannel)
}
