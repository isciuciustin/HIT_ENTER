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
    fn multibyte_messages_are_measured_in_characters() {
        // 4000 emoji is 16000 bytes but a legal message.
        assert!(validate_message_content(&"🎉".repeat(MESSAGE_MAX_CHARS)).is_ok());
    }
}
