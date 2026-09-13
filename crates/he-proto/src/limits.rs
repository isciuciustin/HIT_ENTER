//! Size and shape limits, validated on **both** sides of the wire.
//!
//! A client validates to give fast feedback; a server validates because it can
//! never trust a client. Both call exactly these functions, so the two can not
//! drift apart.

use thiserror::Error;

/// Largest single protocol frame, enforced before allocating a read buffer.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub const USERNAME_MIN_CHARS: usize = 3;
pub const USERNAME_MAX_CHARS: usize = 32;
pub const PASSWORD_MIN_BYTES: usize = 8;
/// Argon2 itself is unbounded, but accepting megabyte passwords is a free
/// denial-of-service against our own hashing.
pub const PASSWORD_MAX_BYTES: usize = 1024;
pub const MESSAGE_MAX_CHARS: usize = 4000;
pub const CHANNEL_NAME_MAX_CHARS: usize = 64;

/// Longest id accepted from the wire. Ours are UUIDv7 (36 characters); the cap
/// exists so that a hostile id cannot be used to build a huge query string or
/// a huge log line.
pub const ID_MAX_BYTES: usize = 64;

/// An invite code is a bearer credential typed or pasted by a human, and the
/// server is the only thing that can say whether one is *real* — in constant
/// time, so that trying is not an oracle. This cap only refuses what could
/// never be a code, before it is carried any further.
pub const INVITE_CODE_MAX_BYTES: usize = 64;

/// A send nonce is chosen by the client and echoed back untouched. It is never
/// stored, so it only has to be long enough to be unique within one client.
pub const NONCE_MAX_BYTES: usize = 64;

/// Default and maximum page size for `backfill`. The maximum is what stops one
/// request from asking the server to read a whole channel into memory.
pub const BACKFILL_DEFAULT_LIMIT: u32 = 50;
pub const BACKFILL_MAX_LIMIT: u32 = 200;

/// How many channel cursors one `resume` may carry.
///
/// A reconnect names every channel the client has history for, so this is a
/// bound on how much work a single frame can ask the server to do — one query
/// per cursor — before the client is told to backfill the ordinary way.
pub const RESUME_MAX_CHANNELS: usize = 200;

/// Most messages one `resume` will hand back, across every channel.
///
/// A client that has been away for a month must not be answered with a month
/// of history in one frame. Past this the gap is reported as *truncated* and
/// the channel is filled the paged way, which is what `backfill` is for.
pub const RESUME_MAX_MESSAGES: u32 = 500;

/// Most messages one channel contributes to a `resume`.
pub const RESUME_PER_CHANNEL_LIMIT: u32 = BACKFILL_MAX_LIMIT;

/// How long a typing indicator stays on screen without being renewed.
///
/// Shared so that the sender's throttle and the receiver's expiry are derived
/// from one number: a throttle longer than the timeout makes the indicator
/// flicker, and there is no way to notice that from either side alone.
pub const TYPING_TIMEOUT_SECS: u64 = 8;

/// How often a client may say it is typing. Comfortably inside
/// [`TYPING_TIMEOUT_SECS`], so a continuous typist never flickers.
pub const TYPING_THROTTLE_SECS: u64 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("username must be {USERNAME_MIN_CHARS}-{USERNAME_MAX_CHARS} characters")]
    UsernameLength,
    #[error("username may only contain letters, digits, '_', '.' or '-'")]
    UsernameCharset,
    #[error("password must be at least {PASSWORD_MIN_BYTES} bytes")]
    PasswordTooShort,
    #[error("password must be at most {PASSWORD_MAX_BYTES} bytes")]
    PasswordTooLong,
    #[error("message is empty")]
    MessageEmpty,
    #[error("message must be at most {MESSAGE_MAX_CHARS} characters")]
    MessageTooLong,
    #[error("channel name is empty")]
    ChannelNameEmpty,
    #[error("channel name must be at most {CHANNEL_NAME_MAX_CHARS} characters")]
    ChannelNameTooLong,
    #[error("identifier is empty, too long, or not an identifier")]
    IdInvalid,
    #[error("nonce must be 1-{NONCE_MAX_BYTES} bytes")]
    NonceInvalid,
    #[error("backfill limit must be 1-{BACKFILL_MAX_LIMIT}")]
    BackfillLimit,
    #[error("an invite must allow at least one use")]
    InviteUses,
    #[error("invite code is empty, too long, or not an invite code")]
    InviteCodeInvalid,
    #[error("a resume may name at most {RESUME_MAX_CHANNELS} channels")]
    ResumeTooManyChannels,
}

/// Lowercased form used as the uniqueness key for accounts.
///
/// Usernames are compared case-insensitively so that `Justin` and `justin`
/// cannot both be registered and impersonate one another.
pub fn username_ci(username: &str) -> String {
    username.to_lowercase()
}

pub fn validate_username(username: &str) -> Result<(), ValidationError> {
    let len = username.chars().count();
    if !(USERNAME_MIN_CHARS..=USERNAME_MAX_CHARS).contains(&len) {
        return Err(ValidationError::UsernameLength);
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(ValidationError::UsernameCharset);
    }
    Ok(())
}

/// Passwords are measured in **bytes**, not characters: the byte length is what
/// the hasher and the network see.
pub fn validate_password(password: &str) -> Result<(), ValidationError> {
    match password.len() {
        n if n < PASSWORD_MIN_BYTES => Err(ValidationError::PasswordTooShort),
        n if n > PASSWORD_MAX_BYTES => Err(ValidationError::PasswordTooLong),
        _ => Ok(()),
    }
}

pub fn validate_message_content(content: &str) -> Result<(), ValidationError> {
    if content.trim().is_empty() {
        return Err(ValidationError::MessageEmpty);
    }
    if content.chars().count() > MESSAGE_MAX_CHARS {
        return Err(ValidationError::MessageTooLong);
    }
    Ok(())
}

pub fn validate_channel_name(name: &str) -> Result<(), ValidationError> {
    if name.trim().is_empty() {
        return Err(ValidationError::ChannelNameEmpty);
    }
    if name.chars().count() > CHANNEL_NAME_MAX_CHARS {
        return Err(ValidationError::ChannelNameTooLong);
    }
    Ok(())
}

/// Checks an id that arrived from the wire.
///
/// Queries are parameterised, so this is not about injection — it is about
/// refusing to carry a megabyte of attacker-chosen text through the server on
/// the way to a `WHERE` clause that will not match anything anyway.
pub fn validate_id(id: &str) -> Result<(), ValidationError> {
    if id.is_empty()
        || id.len() > ID_MAX_BYTES
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ValidationError::IdInvalid);
    }
    Ok(())
}

pub fn validate_nonce(nonce: &str) -> Result<(), ValidationError> {
    if nonce.is_empty() || nonce.len() > NONCE_MAX_BYTES {
        return Err(ValidationError::NonceInvalid);
    }
    Ok(())
}

/// Checks the *shape* of an invite code, never its validity.
///
/// Formatting is tolerated — a code is read aloud, retyped, and pasted out of
/// chat messages, so `K7QP-2M4X-9WTZ`, `k7qp 2m4x 9wtz` and a stray underscore
/// all have to survive. The server canonicalises and compares in constant
/// time; this is only the bound that keeps a megabyte of attacker-chosen text
/// out of a query and a log line.
pub fn validate_invite_code(code: &str) -> Result<(), ValidationError> {
    if code.is_empty()
        || code.len() > INVITE_CODE_MAX_BYTES
        || !code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
    {
        return Err(ValidationError::InviteCodeInvalid);
    }
    Ok(())
}

pub fn validate_backfill_limit(limit: u32) -> Result<(), ValidationError> {
    if limit == 0 || limit > BACKFILL_MAX_LIMIT {
        return Err(ValidationError::BackfillLimit);
    }
    Ok(())
}

/// Checks the cursor map a `resume` carries.
///
/// Every key is a channel id and every value a message id, both of which end
/// up in a query, and the map as a whole decides how many queries one frame
/// costs.
pub fn validate_resume_cursors<'a>(
    cursors: impl ExactSizeIterator<Item = (&'a str, &'a str)>,
) -> Result<(), ValidationError> {
    if cursors.len() > RESUME_MAX_CHANNELS {
        return Err(ValidationError::ResumeTooManyChannels);
    }
    for (channel_id, last_id) in cursors {
        validate_id(channel_id)?;
        validate_id(last_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reasonable_usernames() {
        for name in ["justin", "HAKR_FLEXER", "a.b-c", "u12"] {
            assert!(validate_username(name).is_ok(), "rejected {name}");
        }
    }

    #[test]
    fn rejects_bad_usernames() {
        assert_eq!(
            validate_username("ab"),
            Err(ValidationError::UsernameLength)
        );
        assert_eq!(
            validate_username(&"x".repeat(33)),
            Err(ValidationError::UsernameLength)
        );
        assert_eq!(
            validate_username("has space"),
            Err(ValidationError::UsernameCharset)
        );
        // Homoglyphs must not sneak past the charset check.
        assert_eq!(
            validate_username("justіn"), // Cyrillic 'і'
            Err(ValidationError::UsernameCharset)
        );
    }

    #[test]
    fn username_uniqueness_is_case_insensitive() {
        assert_eq!(username_ci("Justin"), username_ci("jUsTiN"));
    }

    #[test]
    fn username_length_counts_characters_not_bytes() {
        // Four characters, eight bytes: length is fine, charset is not.
        assert_eq!(
            validate_username("héllo"),
            Err(ValidationError::UsernameCharset)
        );
    }

    #[test]
    fn password_bounds() {
        assert_eq!(
            validate_password("short"),
            Err(ValidationError::PasswordTooShort)
        );
        assert!(validate_password("correct horse").is_ok());
        assert_eq!(
            validate_password(&"x".repeat(PASSWORD_MAX_BYTES + 1)),
            Err(ValidationError::PasswordTooLong)
        );
    }

    #[test]
    fn message_bounds() {
        assert_eq!(
            validate_message_content("   \n "),
            Err(ValidationError::MessageEmpty)
        );
        assert!(validate_message_content("hi").is_ok());
        assert!(validate_message_content(&"x".repeat(MESSAGE_MAX_CHARS)).is_ok());
        assert_eq!(
            validate_message_content(&"x".repeat(MESSAGE_MAX_CHARS + 1)),
            Err(ValidationError::MessageTooLong)
        );
    }

    #[test]
    fn ids_must_look_like_ids() {
        assert!(validate_id("0199c1f8-7c3a-7a1e-9f0b-6d2f4c8a1b2c").is_ok());
        assert_eq!(validate_id(""), Err(ValidationError::IdInvalid));
        assert_eq!(
            validate_id(&"a".repeat(ID_MAX_BYTES + 1)),
            Err(ValidationError::IdInvalid)
        );
        // Not because of injection — queries are parameterised — but because
        // nothing downstream should have to think about it.
        assert_eq!(validate_id("1' OR '1'='1"), Err(ValidationError::IdInvalid));
    }

    #[test]
    fn backfill_pages_are_bounded() {
        assert!(validate_backfill_limit(BACKFILL_DEFAULT_LIMIT).is_ok());
        assert!(validate_backfill_limit(BACKFILL_MAX_LIMIT).is_ok());
        assert_eq!(
            validate_backfill_limit(0),
            Err(ValidationError::BackfillLimit)
        );
        assert_eq!(
            validate_backfill_limit(BACKFILL_MAX_LIMIT + 1),
            Err(ValidationError::BackfillLimit)
        );
    }

    #[test]
    fn nonces_are_bounded() {
        assert!(validate_nonce("n1").is_ok());
        assert_eq!(validate_nonce(""), Err(ValidationError::NonceInvalid));
        assert_eq!(
            validate_nonce(&"n".repeat(NONCE_MAX_BYTES + 1)),
            Err(ValidationError::NonceInvalid)
        );
    }

    #[test]
    fn invite_codes_are_bounded_but_forgiving_about_formatting() {
        for code in ["K7QP-2M4X-9WTZ", "k7qp2m4x9wtz", "K7QP 2M4X 9WTZ"] {
            assert!(validate_invite_code(code).is_ok(), "rejected {code}");
        }
        assert_eq!(
            validate_invite_code(""),
            Err(ValidationError::InviteCodeInvalid)
        );
        assert_eq!(
            validate_invite_code(&"A".repeat(INVITE_CODE_MAX_BYTES + 1)),
            Err(ValidationError::InviteCodeInvalid)
        );
        // A code goes into a URL query string without escaping, so anything
        // that would need escaping is not a code.
        assert_eq!(
            validate_invite_code("K7QP&c=other"),
            Err(ValidationError::InviteCodeInvalid)
        );
    }

    #[test]
    fn a_resume_is_bounded_in_both_directions() {
        // Both bounds matter: the number of cursors is the number of queries,
        // and the number of messages is how much one frame may weigh.
        let many: Vec<(String, String)> = (0..RESUME_MAX_CHANNELS + 1)
            .map(|n| (format!("c{n}"), format!("m{n}")))
            .collect();
        assert_eq!(
            validate_resume_cursors(many.iter().map(|(c, m)| (c.as_str(), m.as_str()))),
            Err(ValidationError::ResumeTooManyChannels)
        );
        assert!(validate_resume_cursors([("c1", "m1")].into_iter()).is_ok());
        assert_eq!(
            validate_resume_cursors([("c1", "not an id!")].into_iter()),
            Err(ValidationError::IdInvalid)
        );
    }

    #[test]
    fn a_typing_throttle_is_shorter_than_its_timeout() {
        // Otherwise the indicator expires between two keystrokes and the
        // person on the other end watches it blink.
        const { assert!(TYPING_THROTTLE_SECS < TYPING_TIMEOUT_SECS) };
    }

    #[test]
    fn multibyte_messages_are_measured_in_characters() {
        // 4000 emoji is 16000 bytes but a legal message.
        assert!(validate_message_content(&"🎉".repeat(MESSAGE_MAX_CHARS)).is_ok());
    }
}
