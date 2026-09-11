//! The HIT_ENTER server.
//!
//! Deliberately free of any Tauri dependency: the same library backs the
//! desktop app's "host a space" switch and the headless `he-serverd` binary.
//! See `docs/PLAN.md` §2.1 — the host's own client dials this over iroh exactly
//! as a remote client does, so there is only ever one network path.
//!
//! M1 status: everything below is reachable by a direct function call and has
//! no idea a network exists. [`Server`] owns the database, the server's
//! identity, the password hasher and the rate limiter; M2 puts an iroh
//! `ProtocolHandler` in front of it that does nothing but translate frames into
//! these calls.
//!
//! ## The shape of authentication
//!
//! Three layers, never conflated (PLAN §3):
//!
//! - **`EndpointId`** identifies a *device*, and iroh proves it during the QUIC
//!   handshake. Every method here takes it as an argument precisely because it
//!   must come from the connection and never from a message body.
//! - **Account** identifies a *person*, and the password proves it.
//! - **Session** is one live connection, and does not exist until M2.
//!
//! Which gives the login story: [`Server::register`] or [`Server::login`] costs
//! a password once and enrols the device; every later connection is
//! [`Server::authenticate_device`], which costs nothing and prompts for
//! nothing.

pub mod accept;
pub mod auth;
pub mod channels;
pub mod db;
pub mod devices;
pub mod error;
pub mod invite;
pub mod messages;
pub mod ratelimit;
pub mod rpc;

use std::path::Path;
use std::time::Duration;

use he_proto::{Password, limits};
use iroh::SecretKey;
use sqlx::SqlitePool;

pub use accept::{ChatProtocol, Limits, Session, bind_endpoint, serve, serve_on};
pub use auth::User;
pub use devices::Device;
pub use error::{Result, ServerError};
pub use invite::Invite;
use ratelimit::{Key, RateLimiter};

/// A server's long-lived cryptographic identity.
///
/// The public half is the `EndpointId` that invite tickets point at, so this
/// key **is** the server: rotating it invalidates every invite ever issued.
/// Treat it like an SSH host key — see `docs/PLAN.md` §11.
#[derive(Debug)]
pub struct ServerIdentity {
    secret: SecretKey,
}

impl ServerIdentity {
    /// Generates a brand new identity. Called once, on first run.
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::generate(),
        }
    }

    /// Restores an identity previously persisted with [`Self::to_bytes`].
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            secret: SecretKey::from_bytes(&bytes),
        }
    }

    /// Serialises the secret for storage.
    ///
    /// The result is as sensitive as the key itself: never log it, and never
    /// include it in a bug report.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// The public identity peers dial. Safe to print and to share.
    pub fn endpoint_id(&self) -> iroh::EndpointId {
        self.secret.public()
    }

    /// The key itself, for handing to iroh's `Endpoint` builder in M2.
    pub fn secret_key(&self) -> &SecretKey {
        &self.secret
    }
}

/// One space: one database, one `EndpointId`, one set of accounts.
///
/// Hosting two spaces means running two of these (PLAN §15, question 1).
#[derive(Debug)]
pub struct Server {
    pool: SqlitePool,
    identity: ServerIdentity,
    name: String,
    hasher: auth::Hasher,
    limiter: RateLimiter,
}

impl Server {
    /// Opens the database at `path`, migrating it forward, and loads the
    /// server's identity — generating and storing one if this is a first run.
    ///
    /// `name_if_new` is used only when creating the space; an existing
    /// database keeps the name it has.
    pub async fn open(path: &Path, name_if_new: &str) -> Result<Self> {
        let pool = db::open(path).await?;
        let (identity, name, is_new) = load_or_create_identity(&pool, name_if_new).await?;

        if is_new {
            // A space with no channel has nowhere to put a message, and the
            // first person to join would arrive at a dead end.
            let mut conn = pool.acquire().await?;
            channels::create(&mut conn, channels::DEFAULT_CHANNEL_NAME, None, 0).await?;
        }

        tracing::info!(endpoint_id = %identity.endpoint_id(), %name, "server database ready");

        Ok(Self {
            pool,
            identity,
            name,
            hasher: auth::Hasher::new(),
            limiter: RateLimiter::default(),
        })
    }

    pub fn identity(&self) -> &ServerIdentity {
        &self.identity
    }

    /// The address to put in an invite ticket.
    pub fn endpoint_id(&self) -> iroh::EndpointId {
        self.identity.endpoint_id()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The connection pool, for the protocol handler that lands in M2.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Creates the owner account.
    ///
    /// **Local call only, and the one account not gated by an invite.** It is
    /// not reachable over the network: the person running the process already
    /// owns the machine, the database and the secret key, so there is nothing
    /// an invite would be protecting. Everyone else registers with a code the
    /// owner hands out.
    pub async fn create_owner(&self, username: &str, password: &Password) -> Result<User> {
        if auth::owner_exists(&self.pool).await? {
            return Err(ServerError::OwnerExists);
        }
        limits::validate_username(username)?;
        limits::validate_password(password.expose())?;

        let password_hash = self.hasher.hash(password).await?;
        let mut conn = self.pool.acquire().await?;
        let user = auth::create_user(&mut conn, username, &password_hash, true).await?;

        tracing::info!(user_id = %user.id, username = %user.username, "owner account created");
        Ok(user)
    }

    /// Issues an invite code.
    ///
    /// Any account may invite; the row records who did, which is what the
    /// owner's "revoke invite" tool in M6 works from. Restricting this to the
    /// owner is a product decision, and it is not made here.
    pub async fn create_invite(
        &self,
        created_by: &str,
        expires_in: Option<Duration>,
        max_uses: Option<i64>,
    ) -> Result<Invite> {
        if auth::find_by_id(&self.pool, created_by).await?.is_none() {
            return Err(ServerError::UnknownUser);
        }
        let expires_at = expires_in.map(|d| db::now_unix().saturating_add(d.as_secs() as i64));
        invite::create(&self.pool, created_by, expires_at, max_uses).await
    }

    /// Creates an account from an invite and enrols the calling device.
    ///
    /// `endpoint_id` **must** come from the iroh connection (PLAN §11). The
    /// whole operation is one transaction: a registration that fails on a
    /// taken username does not quietly burn a use of the invite.
    pub async fn register(
        &self,
        endpoint_id: &iroh::EndpointId,
        invite_code: &str,
        username: &str,
        password: &Password,
        label: Option<&str>,
    ) -> Result<(User, Device)> {
        let keys = rate_limit_keys(endpoint_id, Some(username));
        self.limiter.check(&keys)?;

        // Validate before touching the invite, so a malformed username cannot
        // be used to burn codes.
        limits::validate_username(username)?;
        limits::validate_password(password.expose())?;

        // Refuse an unrecognised code before paying for a hash: Argon2id is
        // expensive on purpose, and guessing invite codes must not be a way to
        // spend this machine's CPU. `redeem` below is still the authority.
        if let Err(err) = invite::exists(&self.pool, invite_code).await {
            self.limiter.record_failure(&keys);
            return Err(err);
        }

        let password_hash = self.hasher.hash(password).await?;

        let mut tx = self.pool.begin().await?;
        let result = async {
            invite::redeem(&mut tx, invite_code).await?;
            let user = auth::create_user(&mut tx, username, &password_hash, false).await?;
            let device = devices::enroll(&mut tx, endpoint_id, &user.id, label).await?;
            Ok::<_, ServerError>((user, device))
        }
        .await;

        match result {
            Ok((user, device)) => {
                tx.commit().await?;
                self.limiter.record_success(&keys);
                tracing::info!(
                    user_id = %user.id,
                    username = %user.username,
                    endpoint_id = %endpoint_id,
                    "account registered and device enrolled"
                );
                Ok((user, device))
            }
            Err(err) => {
                // Guessing invite codes is an attack; a taken username is not.
                // Only the former earns backoff.
                if matches!(err, ServerError::InviteInvalid) {
                    self.limiter.record_failure(&keys);
                }
                Err(err)
            }
        }
    }

    /// Verifies a password and enrols the calling device.
    ///
    /// This is the *only* thing a password is for after the first join: it
    /// enrols a new machine (PLAN §3). An enrolled device uses
    /// [`Server::authenticate_device`] instead and is never prompted again.
    pub async fn login(
        &self,
        endpoint_id: &iroh::EndpointId,
        username: &str,
        password: &Password,
        label: Option<&str>,
    ) -> Result<(User, Device)> {
        let keys = rate_limit_keys(endpoint_id, Some(username));
        // Checked before hashing: the point of backoff is to *not* spend
        // 100 ms of CPU on an attempt already destined to be refused.
        self.limiter.check(&keys)?;

        let Some((user, password_hash)) = auth::find_by_username(&self.pool, username).await?
        else {
            // Spend the same time as a real verification. Otherwise "no such
            // user" answers in microseconds and anyone with a stopwatch can
            // enumerate the accounts on this server.
            self.hasher.verify_dummy(password).await;
            self.limiter.record_failure(&keys);
            return Err(ServerError::BadCredentials);
        };

        if !self.hasher.verify(password, &password_hash).await? {
            self.limiter.record_failure(&keys);
            return Err(ServerError::BadCredentials);
        }

        let mut conn = self.pool.acquire().await?;
        let device = devices::enroll(&mut conn, endpoint_id, &user.id, label).await?;
        self.limiter.record_success(&keys);

        tracing::info!(
            user_id = %user.id,
            endpoint_id = %endpoint_id,
            "password login; device enrolled"
        );
        Ok((user, device))
    }

    /// Authenticates an already-enrolled device. No password involved.
    ///
    /// `username` disambiguates a machine that holds more than one account on
    /// this server; with one enrolment it can be `None`.
    pub async fn authenticate_device(
        &self,
        endpoint_id: &iroh::EndpointId,
        username: Option<&str>,
    ) -> Result<(User, Device)> {
        // A device serving a backoff for password guessing does not get to
        // skip the queue by trying the device path.
        self.limiter
            .check(&[Key::Endpoint(endpoint_id.to_string())])?;

        let device = devices::resolve(&self.pool, endpoint_id, username).await?;
        let Some(user) = auth::find_by_id(&self.pool, &device.user_id).await? else {
            // The FK cascade should make this impossible; if it happens the
            // enrolment is orphaned and must not authenticate anyone.
            return Err(ServerError::DeviceNotEnrolled);
        };
        devices::touch(&self.pool, endpoint_id, &user.id).await?;

        Ok((user, device))
    }

    /// Every device enrolled for an account, revoked ones included.
    pub async fn devices_for_user(&self, user_id: &str) -> Result<Vec<Device>> {
        devices::for_user(&self.pool, user_id).await
    }

    /// Kicks a device off. Its next `Hello` fails with
    /// [`ServerError::DeviceRevoked`]; the key itself is not blacklisted, so
    /// the password can enrol it again.
    pub async fn revoke_device(&self, endpoint_id: &str, user_id: &str) -> Result<()> {
        devices::revoke(&self.pool, endpoint_id, user_id).await
    }

    pub async fn user_by_id(&self, user_id: &str) -> Result<Option<User>> {
        auth::find_by_id(&self.pool, user_id).await
    }

    /// Every channel in the space, in render order.
    pub async fn channels(&self) -> Result<Vec<he_proto::Channel>> {
        channels::list(&self.pool).await
    }

    /// Every account, as `Ready.members`.
    pub async fn members(&self) -> Result<Vec<he_proto::Member>> {
        auth::list_members(&self.pool).await
    }

    /// Stores a message and returns the authoritative row.
    ///
    /// Broadcasting it is the network layer's job: this call has no idea who
    /// is connected, which is what keeps it testable without a socket.
    pub async fn post_message(
        &self,
        author: &User,
        channel_id: &str,
        content: &str,
    ) -> Result<he_proto::Message> {
        limits::validate_message_content(content)?;
        channels::require(&self.pool, channel_id).await?;

        let mut conn = self.pool.acquire().await?;
        let message =
            messages::insert(&mut conn, channel_id, &author.id, &author.username, content).await?;

        // Deliberately no `content` field: PLAN §11. The ids are enough to
        // find the row in a database the host can already read.
        tracing::debug!(
            message_id = %message.id,
            channel_id = %channel_id,
            author_id = %author.id,
            "message stored"
        );
        Ok(message)
    }

    /// A page of history, newest first, ending just before `before`.
    pub async fn backfill(
        &self,
        channel_id: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<he_proto::Message>> {
        limits::validate_backfill_limit(limit)?;
        channels::require(&self.pool, channel_id).await?;
        messages::backfill(&self.pool, channel_id, before, limit).await
    }

    pub async fn user_by_username(&self, username: &str) -> Result<Option<User>> {
        Ok(auth::find_by_username(&self.pool, username)
            .await?
            .map(|(user, _hash)| user))
    }
}

/// Backoff is counted against the device *and* the account it is aiming at —
/// see [`ratelimit`] for why neither alone is enough.
fn rate_limit_keys(endpoint_id: &iroh::EndpointId, username: Option<&str>) -> Vec<Key> {
    let mut keys = vec![Key::Endpoint(endpoint_id.to_string())];
    if let Some(username) = username {
        keys.push(Key::Username(limits::username_ci(username)));
    }
    keys
}

/// Reads the single `server_meta` row, creating it on a first run. The `bool`
/// says whether this run created it, which is when a new space needs its
/// first channel.
async fn load_or_create_identity(
    pool: &SqlitePool,
    name_if_new: &str,
) -> Result<(ServerIdentity, String, bool)> {
    if let Some(row) = sqlx::query!("SELECT name, secret_key FROM server_meta WHERE id = 1")
        .fetch_optional(pool)
        .await?
    {
        let bytes: [u8; 32] = row
            .secret_key
            .try_into()
            .map_err(|_| ServerError::CorruptIdentity)?;
        return Ok((ServerIdentity::from_bytes(bytes), row.name, false));
    }

    let identity = ServerIdentity::generate();
    let secret = identity.to_bytes().to_vec();
    let created_at = db::now_unix();
    sqlx::query!(
        "INSERT INTO server_meta (id, name, secret_key, created_at) VALUES (1, ?1, ?2, ?3)",
        name_if_new,
        secret,
        created_at,
    )
    .execute(pool)
    .await?;

    Ok((identity, name_if_new.to_owned(), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_a_storage_round_trip() {
        let original = ServerIdentity::generate();
        let restored = ServerIdentity::from_bytes(original.to_bytes());
        assert_eq!(
            original.endpoint_id(),
            restored.endpoint_id(),
            "a restored identity must keep the same EndpointId, or every \
             previously issued invite ticket breaks"
        );
    }

    #[test]
    fn generated_identities_are_distinct() {
        assert_ne!(
            ServerIdentity::generate().endpoint_id(),
            ServerIdentity::generate().endpoint_id()
        );
    }

    #[test]
    fn the_secret_key_never_formats_itself() {
        // A `tracing` call that prints the server struct must not be the thing
        // that ends up in a pasted bug report (PLAN §11).
        let identity = ServerIdentity::generate();
        let printed = format!("{identity:?}");
        let hex: String = identity
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(!printed.contains(&hex), "{printed}");
    }
}
