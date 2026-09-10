//! Invite codes: the only way an account is created over the network.
//!
//! An invite is a **bearer credential** — whoever holds it can make an account
//! — so it is generated from the OS CSPRNG and compared in constant time
//! (PLAN §11). The code travels inside a `hitenter://` ticket alongside the
//! server's `EndpointId` (PLAN §5).

use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

use crate::db::now_unix;
use crate::error::{Result, ServerError};

/// Crockford-style alphabet: no `I`, `L`, `O`, `U`, `0` or `1`, because these
/// codes get read aloud, retyped, and pasted out of chat messages.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Three groups of four. 12 characters of a 30-symbol alphabet is a little
/// under 59 bits — far past guessable, still short enough to type.
const GROUPS: usize = 3;
const GROUP_LEN: usize = 4;
const CODE_LEN: usize = GROUPS * GROUP_LEN;

/// An invite as stored. The `code` is the secret; everything else is policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// Display form, `K7QP-2M4X-9WTZ`.
    pub code: String,
    pub created_by: String,
    pub created_at: i64,
    /// `None` = never expires.
    pub expires_at: Option<i64>,
    /// `None` = unlimited uses.
    pub max_uses: Option<i64>,
    pub uses: i64,
}

/// Strips formatting so that `k7qp2m4x9wtz`, `K7QP-2M4X-9WTZ` and a code with a
/// stray space all compare equal.
///
/// Returns `None` for anything that is not `CODE_LEN` symbols of [`ALPHABET`],
/// which is public information about the format and so safe to reject early.
fn canonical(code: &str) -> Option<[u8; CODE_LEN]> {
    let mut out = [0u8; CODE_LEN];
    let mut len = 0;
    for ch in code.chars() {
        if ch == '-' || ch == ' ' || ch == '_' {
            continue;
        }
        let upper = ch.to_ascii_uppercase() as u8;
        if !ALPHABET.contains(&upper) {
            return None;
        }
        *out.get_mut(len)? = upper;
        len += 1;
    }
    (len == CODE_LEN).then_some(out)
}

/// A fresh code in display form.
fn generate_code() -> Result<String> {
    let mut bytes = [0u8; CODE_LEN];
    getrandom::fill(&mut bytes).map_err(|_| ServerError::PasswordHash)?;

    // ALPHABET has 30 symbols and 256 is not a multiple of 30, so folding a
    // byte with `%` biases the first 16 symbols by about 1 part in 8. Over 12
    // characters that costs a fraction of a bit out of ~59 — irrelevant to
    // guessing a code, and worth the simplicity of not rejection-sampling.
    let mut code = String::with_capacity(CODE_LEN + GROUPS - 1);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && i % GROUP_LEN == 0 {
            code.push('-');
        }
        let symbol = ALPHABET
            .get(usize::from(*byte) % ALPHABET.len())
            .copied()
            .unwrap_or(b'2');
        code.push(char::from(symbol));
    }
    Ok(code)
}

/// Issues an invite. Only an owner should be calling this; the caller enforces
/// that, because this module does not know about roles.
pub async fn create(
    pool: &SqlitePool,
    created_by: &str,
    expires_at: Option<i64>,
    max_uses: Option<i64>,
) -> Result<Invite> {
    let code = generate_code()?;
    let created_at = now_unix();

    sqlx::query!(
        "INSERT INTO invites (code, created_by, created_at, expires_at, max_uses, uses)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        code,
        created_by,
        created_at,
        expires_at,
        max_uses,
    )
    .execute(pool)
    .await?;

    Ok(Invite {
        code,
        created_by: created_by.to_owned(),
        created_at,
        expires_at,
        max_uses,
        uses: 0,
    })
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Invite>> {
    let rows = sqlx::query_as!(
        Invite,
        "SELECT code, created_by, created_at, expires_at, max_uses, uses
         FROM invites ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Deletes an invite. Unused ones vanish; the accounts made with it stay.
pub async fn revoke(pool: &SqlitePool, code: &str) -> Result<()> {
    sqlx::query!("DELETE FROM invites WHERE code = ?1", code)
        .execute(pool)
        .await?;
    Ok(())
}

/// Finds the invite matching `presented`, comparing every stored code in
/// constant time.
///
/// A `WHERE code = ?` lookup would be shorter, and would answer in a time that
/// depends on how far into the B-tree the match sat. The number of invites on a
/// space is tiny, so scanning them all and folding the comparisons together
/// costs nothing and leaks nothing.
async fn find_constant_time(conn: &mut SqliteConnection, presented: &str) -> Result<String> {
    let Some(presented) = canonical(presented) else {
        return Err(ServerError::InviteInvalid);
    };

    let rows = sqlx::query!("SELECT code FROM invites")
        .fetch_all(&mut *conn)
        .await?;

    let mut matched: Option<String> = None;
    for row in rows {
        let Some(stored) = canonical(&row.code) else {
            // A row that is not in our format cannot be what was presented.
            continue;
        };
        // No `break`: the loop always runs to the end so its duration does not
        // reveal *which* invite matched, or how many were checked.
        if bool::from(stored.ct_eq(&presented)) {
            matched = Some(row.code);
        }
    }

    matched.ok_or(ServerError::InviteInvalid)
}

/// Answers "is this code one of ours?" without consuming anything.
///
/// Registration calls this *before* hashing the new password. Argon2id is
/// deliberately expensive, so hashing first would let anyone spend 100 ms of
/// this machine's CPU per guess at an invite code. The authoritative check is
/// still [`redeem`], inside the transaction; this one only refuses to pay for
/// obvious nonsense.
pub(crate) async fn exists(pool: &SqlitePool, presented: &str) -> Result<()> {
    let mut conn = pool.acquire().await?;
    find_constant_time(&mut conn, presented).await.map(|_| ())
}

/// Consumes one use of an invite, or fails.
///
/// The expiry and use-count checks live in the `UPDATE`'s `WHERE` clause on
/// purpose: two registrations racing for the last use of a `max_uses = 1`
/// invite both pass any check done beforehand, and only one can win this.
pub(crate) async fn redeem(conn: &mut SqliteConnection, presented: &str) -> Result<()> {
    let code = find_constant_time(&mut *conn, presented).await?;
    let now = now_unix();

    let result = sqlx::query!(
        "UPDATE invites SET uses = uses + 1
         WHERE code = ?1
           AND (expires_at IS NULL OR expires_at > ?2)
           AND (max_uses   IS NULL OR uses < max_uses)",
        code,
        now,
    )
    .execute(&mut *conn)
    .await?;

    if result.rows_affected() == 0 {
        // Expired or exhausted. The caller is told only "not valid" — which of
        // the two it was is not something a stranger gets to learn.
        return Err(ServerError::InviteInvalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_look_like_the_documented_format() -> Result<()> {
        let code = generate_code()?;
        assert_eq!(code.len(), CODE_LEN + GROUPS - 1, "{code}");
        assert_eq!(code.matches('-').count(), GROUPS - 1, "{code}");
        assert!(canonical(&code).is_some(), "{code} must round-trip");
        Ok(())
    }

    #[test]
    fn codes_do_not_repeat() -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            assert!(seen.insert(generate_code()?), "CSPRNG produced a duplicate");
        }
        Ok(())
    }

    #[test]
    fn canonicalisation_forgives_typing_and_nothing_else() {
        let canonical_form = canonical("K7QP-2M4X-9WTZ");
        assert!(canonical_form.is_some());
        assert_eq!(canonical(" k7qp 2m4x-9wtz "), canonical_form);
        assert_eq!(canonical("K7QP2M4X9WTZ"), canonical_form);

        // Ambiguous glyphs are not in the alphabet, so they are rejected rather
        // than silently mapped onto their look-alikes.
        assert_eq!(canonical("K7QP-2M4X-9WTO"), None);
        assert_eq!(canonical("K7QP-2M4X-9WT"), None, "too short");
        assert_eq!(canonical("K7QP-2M4X-9WTZZ"), None, "too long");
        assert_eq!(canonical(""), None);
    }
}
