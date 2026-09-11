//! Passwords and accounts.
//!
//! The whole of PLAN §10 in one sentence: a password goes in, an Argon2id PHC
//! string comes out, and **no code path anywhere turns that back into a
//! password** — not for us, not for the server owner, not for someone holding
//! the database file.

use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};
use he_proto::Password;
use he_proto::limits;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{new_id, now_unix};
use crate::error::{Result, ServerError};

/// An account on this server.
///
/// The password hash is deliberately absent: it is read inside this module and
/// nowhere else, so it cannot escape into a log line or across the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: String,
    /// As the user typed it. `username_ci` is what uniqueness is enforced on.
    pub username: String,
    pub display_name: Option<String>,
    pub is_owner: bool,
    pub created_at: i64,
}

/// Argon2id, with the parameters this server hashes *new* passwords at.
///
/// Verification always uses the parameters embedded in the stored PHC string,
/// so raising these later keeps every existing account working; only newly set
/// passwords get the stronger settings.
#[derive(Debug, Clone)]
pub struct Hasher {
    params: Params,
}

/// OWASP's first recommended Argon2id configuration: 19 MiB, 2 passes, 1 lane.
/// Lands around 50–100 ms on a laptop of the era, which is the target in
/// PLAN §11 — slow enough to make guessing expensive, fast enough that a login
/// still feels instant.
const M_COST_KIB: u32 = 19 * 1024;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher {
    pub fn new() -> Self {
        // `Params::new` only rejects values below its documented minimums, and
        // the constants above are well clear of them. Falling back to
        // `Params::DEFAULT` (which is the same configuration) keeps the
        // no-`unwrap` rule without pretending this can fail.
        let params = Params::new(M_COST_KIB, T_COST, P_COST, None).unwrap_or(Params::DEFAULT);
        Self { params }
    }

    fn argon2(&self) -> Argon2<'static> {
        Argon2::new(Algorithm::Argon2id, Version::V0x13, self.params.clone())
    }

    /// Hashes a password on the blocking pool.
    ///
    /// Argon2id is *designed* to burn a CPU core for ~100 ms. Doing that on an
    /// async worker thread would stall every other connection the runtime is
    /// serving, which is a denial of service anyone can trigger by logging in.
    pub async fn hash(&self, password: &Password) -> Result<String> {
        let argon2 = self.argon2();
        let password = password.expose().to_owned();
        blocking(move || {
            argon2
                .hash_password(password.as_bytes())
                .map(|hash| hash.to_string())
                .map_err(|_| ServerError::PasswordHash)
        })
        .await
    }

    /// Checks a password against a stored PHC string.
    ///
    /// Returns `Ok(false)` for a wrong password and `Err` only when something
    /// is actually broken — a corrupt hash, say. The comparison itself is
    /// constant-time inside `argon2`.
    pub async fn verify(&self, password: &Password, phc: &str) -> Result<bool> {
        let argon2 = self.argon2();
        let password = password.expose().to_owned();
        let phc = phc.to_owned();
        blocking(move || {
            match argon2.verify_password(password.as_bytes(), phc.as_str()) {
                Ok(()) => Ok(true),
                Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
                // A hash that will not parse is a corrupt row, not a failed
                // login. Say so rather than telling the user their password is
                // wrong forever.
                Err(_) => Err(ServerError::PasswordHash),
            }
        })
        .await
    }

    /// Spends the same work as [`Hasher::verify`] against a throwaway hash.
    ///
    /// Called when the username does not exist. Without it, "no such user"
    /// returns in microseconds while "wrong password" takes 100 ms, and anyone
    /// with a stopwatch can enumerate the accounts on the server — which
    /// [`ServerError::BadCredentials`] exists to prevent.
    pub async fn verify_dummy(&self, password: &Password) {
        let _ = self.verify(password, dummy_hash()).await;
    }
}

/// A PHC string for a password nobody knows, hashed once per process.
///
/// Generated at runtime rather than baked in as a constant so that it always
/// carries this build's parameters, and therefore costs exactly what a real
/// verification costs.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        let mut secret = [0u8; 32];
        // If the OS RNG fails, a fixed value still produces a valid hash of a
        // password no attacker can guess anything from; the point is the work,
        // not the secrecy.
        let _ = getrandom::fill(&mut secret);
        let argon2 = Hasher::new().argon2();
        argon2
            .hash_password(&secret)
            .map(|hash| hash.to_string())
            .unwrap_or_default()
    })
}

/// Runs a CPU-bound closure off the async runtime's worker threads.
async fn blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        // The only way a `spawn_blocking` task fails is a panic inside it, and
        // the only thing in there is Argon2.
        Err(_) => Err(ServerError::PasswordHash),
    }
}

/// Inserts a new account. The caller is responsible for the *policy* — invite
/// gating, ownership — and this function for the *storage*.
///
/// Takes a connection rather than the pool so that registration can run the
/// invite redemption, the insert and the device enrolment in one transaction.
pub(crate) async fn create_user(
    conn: &mut SqliteConnection,
    username: &str,
    password_hash: &str,
    is_owner: bool,
) -> Result<User> {
    limits::validate_username(username)?;

    let id = new_id();
    let username_ci = limits::username_ci(username);
    let created_at = now_unix();
    let owner_flag = i64::from(is_owner);

    let result = sqlx::query!(
        "INSERT INTO users (id, username, username_ci, password_hash, is_owner, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        id,
        username,
        username_ci,
        password_hash,
        owner_flag,
        created_at,
    )
    .execute(&mut *conn)
    .await;

    if let Err(sqlx::Error::Database(err)) = &result
        && err.is_unique_violation()
    {
        // The UNIQUE index on username_ci is the authority, not a prior SELECT:
        // two registrations racing must not both win.
        return Err(ServerError::UsernameTaken);
    }
    result?;

    Ok(User {
        id,
        username: username.to_owned(),
        display_name: None,
        is_owner,
        created_at,
    })
}

/// Looks an account up the way logins do: case-insensitively.
///
/// Returns the PHC string alongside the user, because the only caller is the
/// one that immediately verifies a password against it.
pub(crate) async fn find_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<(User, String)>> {
    let username_ci = limits::username_ci(username);
    let row = sqlx::query!(
        r#"SELECT id, username, display_name, is_owner as "is_owner: bool", created_at,
                  password_hash
           FROM users WHERE username_ci = ?1"#,
        username_ci,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| {
        (
            User {
                id: row.id,
                username: row.username,
                display_name: row.display_name,
                is_owner: row.is_owner,
                created_at: row.created_at,
            },
            row.password_hash,
        )
    }))
}

pub(crate) async fn find_by_id(pool: &SqlitePool, user_id: &str) -> Result<Option<User>> {
    let row = sqlx::query!(
        r#"SELECT id, username, display_name, is_owner as "is_owner: bool", created_at
           FROM users WHERE id = ?1"#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| User {
        id: row.id,
        username: row.username,
        display_name: row.display_name,
        is_owner: row.is_owner,
        created_at: row.created_at,
    }))
}

/// Every account on the server, in the shape other members are allowed to see.
///
/// Note what the query does not select: `password_hash`. The wire type has
/// nowhere to put it, which is the cheapest access control available.
pub(crate) async fn list_members(pool: &SqlitePool) -> Result<Vec<he_proto::Member>> {
    let rows = sqlx::query_as!(
        he_proto::Member,
        r#"SELECT id            AS "id!",
                  username      AS "username!",
                  display_name,
                  is_owner      AS "is_owner!: bool"
           FROM users ORDER BY username_ci"#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

impl User {
    /// The same account, as the wire describes it.
    pub fn as_member(&self) -> he_proto::Member {
        he_proto::Member {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            is_owner: self.is_owner,
        }
    }
}

pub(crate) async fn owner_exists(pool: &SqlitePool) -> Result<bool> {
    let row = sqlx::query_scalar!("SELECT EXISTS(SELECT 1 FROM users WHERE is_owner = 1)")
        .fetch_one(pool)
        .await?;
    Ok(row != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_hash_verifies_and_a_wrong_password_does_not() -> Result<()> {
        let hasher = Hasher::new();
        let password = Password::new("correct horse battery staple");
        let phc = hasher.hash(&password).await?;

        assert!(phc.starts_with("$argon2id$"), "must be Argon2id, got {phc}");
        assert!(hasher.verify(&password, &phc).await?);
        assert!(!hasher.verify(&Password::new("wrong horse"), &phc).await?);
        Ok(())
    }

    #[tokio::test]
    async fn the_same_password_hashes_differently_every_time() -> Result<()> {
        let hasher = Hasher::new();
        let password = Password::new("correct horse battery staple");
        let a = hasher.hash(&password).await?;
        let b = hasher.hash(&password).await?;
        assert_ne!(
            a, b,
            "a per-password salt is what makes a hash table useless"
        );
        // Both still verify: the salt travels inside the PHC string.
        assert!(hasher.verify(&password, &a).await?);
        assert!(hasher.verify(&password, &b).await?);
        Ok(())
    }

    #[tokio::test]
    async fn the_hash_leaks_nothing_of_the_password() -> Result<()> {
        let phc = Hasher::new()
            .hash(&Password::new("hunter2-hunter2"))
            .await?;
        assert!(!phc.contains("hunter2"));
        Ok(())
    }

    #[tokio::test]
    async fn a_corrupt_hash_is_an_error_not_a_failed_login() {
        let result = Hasher::new()
            .verify(&Password::new("whatever it is"), "not a PHC string")
            .await;
        assert!(matches!(result, Err(ServerError::PasswordHash)));
    }
}
