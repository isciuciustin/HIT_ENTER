//! Channels, and the one that exists before anybody makes one.
//!
//! A space with no channel has nowhere to put a message, so the first run
//! creates `#general`. That is the only channel M2 can produce; creating and
//! deleting them from the UI is M6's owner tooling.

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

/// Resolves a channel id, failing with [`ServerError::UnknownChannel`] rather
/// than letting a bad id become a foreign-key error three layers down.
pub(crate) async fn require(pool: &SqlitePool, channel_id: &str) -> Result<()> {
    let found = sqlx::query_scalar!("SELECT 1 FROM channels WHERE id = ?1", channel_id)
        .fetch_optional(pool)
        .await?;
    found.map(|_| ()).ok_or(ServerError::UnknownChannel)
}
