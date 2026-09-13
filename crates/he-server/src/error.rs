//! One error type for the whole server library.
//!
//! Two rules shape it. First, `thiserror` in a library, `anyhow` only at the
//! binary edge (PLAN §14). Second, and less obvious: **no variant may carry a
//! secret**, because errors get logged. A wrong password produces
//! [`ServerError::BadCredentials`] and nothing else — not the password, not
//! the username, not whether the account exists.

use std::time::Duration;

use thiserror::Error;

pub type Result<T, E = ServerError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error(transparent)]
    Validation(#[from] he_proto::limits::ValidationError),

    /// Wrong username *or* wrong password — deliberately indistinguishable, so
    /// that a stranger cannot enumerate the accounts on a server.
    #[error("invalid credentials")]
    BadCredentials,

    /// Unknown, expired, or exhausted invite code. Also deliberately one
    /// variant: which of the three it was is none of the caller's business.
    #[error("invite code is not valid")]
    InviteInvalid,

    #[error("username is already taken")]
    UsernameTaken,

    /// The device was enrolled once and the owner has since revoked it. The
    /// client is told, because it must stop reconnecting and forget the space.
    #[error("this device has been revoked")]
    DeviceRevoked,

    /// This `EndpointId` has no enrolment on this server: log in with a
    /// password to enrol it.
    #[error("this device is not enrolled")]
    DeviceNotEnrolled,

    /// One device, several accounts on this server. The client has to say
    /// which one it means.
    #[error("this device is enrolled for more than one account")]
    DeviceAmbiguous,

    /// Too many failed attempts. The wait is exponential in the number of
    /// failures — see [`crate::ratelimit`].
    #[error("rate limited: retry in {}s", .retry_after.as_secs())]
    RateLimited { retry_after: Duration },

    /// A server has exactly one owner, created once, locally (PLAN §11 —
    /// registration over the network is always invite-gated).
    #[error("this server already has an owner")]
    OwnerExists,

    #[error("no such user")]
    UnknownUser,

    #[error("no such channel")]
    UnknownChannel,

    #[error("no such message")]
    UnknownMessage,

    /// Authenticated, and still not allowed: editing somebody else's message,
    /// for instance. Distinct from a "not found" because the client can
    /// already see who wrote every message it is looking at, so answering
    /// honestly gives away nothing it did not have.
    #[error("not allowed")]
    Forbidden,

    /// Argon2 failed, or a stored PHC string would not parse. Never contains
    /// the password or the hash.
    #[error("password hashing failed")]
    PasswordHash,

    /// The stored `server_meta.secret_key` is not a 32-byte iroh secret key.
    #[error("server identity is corrupt")]
    CorruptIdentity,

    /// The iroh endpoint could not be bound. A `String` rather than iroh's
    /// error type so that this enum does not have to re-export it; the text is
    /// a transport message and carries nothing sensitive.
    #[error("could not bind the iroh endpoint: {0}")]
    Endpoint(String),
}
