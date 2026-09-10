//! Device enrolment — the bridge between iroh's device identity and ours.
//!
//! PLAN §3: the transport authenticates the *device*, the password
//! authenticates the *person*. Enrolment records that a given `EndpointId`
//! belongs to a given account, and from then on iroh's key exchange is the
//! whole login. The password is only needed to enrol somewhere new.
//!
//! **Every `endpoint_id` reaching this module was read off an iroh connection.**
//! One that came out of a message body would let anyone claim to be anyone
//! (PLAN §11), so nothing here is callable without one in hand.

use iroh::EndpointId;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::now_unix;
use crate::error::{Result, ServerError};

/// One device, enrolled for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub endpoint_id: String,
    pub user_id: String,
    /// A human label so an owner can tell "Justin's laptop" from a key they do
    /// not recognise and should revoke.
    pub label: Option<String>,
    pub enrolled_at: i64,
    pub last_seen: i64,
    pub revoked_at: Option<i64>,
}

impl Device {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// Enrols a device for an account, or un-revokes and re-labels an existing
/// enrolment.
///
/// Re-enrolment after a revocation is intentional: revoking a device kicks it
/// off, it does not blacklist the key. Getting back in still requires the
/// password, which is the property that matters.
pub(crate) async fn enroll(
    conn: &mut SqliteConnection,
    endpoint_id: &EndpointId,
    user_id: &str,
    label: Option<&str>,
) -> Result<Device> {
    let endpoint = endpoint_id.to_string();
    let now = now_unix();

    sqlx::query!(
        "INSERT INTO devices (endpoint_id, user_id, label, enrolled_at, last_seen, revoked_at)
         VALUES (?1, ?2, ?3, ?4, ?4, NULL)
         ON CONFLICT (endpoint_id, user_id) DO UPDATE SET
             label      = COALESCE(excluded.label, devices.label),
             last_seen  = excluded.last_seen,
             revoked_at = NULL",
        endpoint,
        user_id,
        label,
        now,
    )
    .execute(&mut *conn)
    .await?;

    Ok(Device {
        endpoint_id: endpoint,
        user_id: user_id.to_owned(),
        label: label.map(str::to_owned),
        enrolled_at: now,
        last_seen: now,
        revoked_at: None,
    })
}

/// Every enrolment for a device, revoked ones included.
pub(crate) async fn for_endpoint(
    pool: &SqlitePool,
    endpoint_id: &EndpointId,
) -> Result<Vec<Device>> {
    let endpoint = endpoint_id.to_string();
    let rows = sqlx::query_as!(
        Device,
        "SELECT endpoint_id, user_id, label, enrolled_at, last_seen, revoked_at
         FROM devices WHERE endpoint_id = ?1 ORDER BY enrolled_at",
        endpoint,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Every device enrolled for an account. This is what the owner's "your
/// devices" list renders, so revoked entries stay visible.
pub async fn for_user(pool: &SqlitePool, user_id: &str) -> Result<Vec<Device>> {
    let rows = sqlx::query_as!(
        Device,
        "SELECT endpoint_id, user_id, label, enrolled_at, last_seen, revoked_at
         FROM devices WHERE user_id = ?1 ORDER BY enrolled_at",
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Resolves a connecting device to the account it may act as.
///
/// `username` disambiguates: one machine may hold accounts on the same server,
/// and the `EndpointId` alone cannot say which one this connection means.
pub(crate) async fn resolve(
    pool: &SqlitePool,
    endpoint_id: &EndpointId,
    username: Option<&str>,
) -> Result<Device> {
    let enrolments = for_endpoint(pool, endpoint_id).await?;
    if enrolments.is_empty() {
        return Err(ServerError::DeviceNotEnrolled);
    }

    let candidates: Vec<Device> = match username {
        Some(username) => {
            let Some((user, _)) = crate::auth::find_by_username(pool, username).await? else {
                return Err(ServerError::DeviceNotEnrolled);
            };
            enrolments
                .into_iter()
                .filter(|device| device.user_id == user.id)
                .collect()
        }
        None => enrolments,
    };

    match candidates.len() {
        0 => Err(ServerError::DeviceNotEnrolled),
        1 => {
            // `candidates` has exactly one element; `into_iter().next()` is the
            // way to take it by value without indexing.
            match candidates.into_iter().next() {
                Some(device) if device.is_active() => Ok(device),
                Some(_) => Err(ServerError::DeviceRevoked),
                None => Err(ServerError::DeviceNotEnrolled),
            }
        }
        _ => {
            // Several accounts on this server share this key. Refusing to guess
            // is the only safe answer; the client knows which account it means.
            let active: Vec<Device> = candidates.into_iter().filter(Device::is_active).collect();
            match active.len() {
                0 => Err(ServerError::DeviceRevoked),
                1 => active
                    .into_iter()
                    .next()
                    .ok_or(ServerError::DeviceNotEnrolled),
                _ => Err(ServerError::DeviceAmbiguous),
            }
        }
    }
}

/// Records that a device is currently connected. Cheap, and it is what makes
/// the owner's device list useful for spotting a key that should not be there.
pub(crate) async fn touch(
    pool: &SqlitePool,
    endpoint_id: &EndpointId,
    user_id: &str,
) -> Result<()> {
    let endpoint = endpoint_id.to_string();
    let now = now_unix();
    sqlx::query!(
        "UPDATE devices SET last_seen = ?3 WHERE endpoint_id = ?1 AND user_id = ?2",
        endpoint,
        user_id,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Revokes one enrolment. The device keeps its key and can still open a QUIC
/// connection — accepting a connection is not authorization (PLAN §11) — but
/// its `Hello` will now be refused.
pub async fn revoke(pool: &SqlitePool, endpoint_id: &str, user_id: &str) -> Result<()> {
    let now = now_unix();
    let result = sqlx::query!(
        "UPDATE devices SET revoked_at = ?3
         WHERE endpoint_id = ?1 AND user_id = ?2 AND revoked_at IS NULL",
        endpoint_id,
        user_id,
        now,
    )
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        // Either no such enrolment or it was already revoked. Both mean "this
        // device has no access", which is what the caller asked for.
        tracing::debug!(%user_id, "revoke was a no-op: no active enrolment");
    }
    Ok(())
}
