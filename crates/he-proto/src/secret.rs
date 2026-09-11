//! Types that carry secrets, and refuse to print them.
//!
//! PLAN §11: *"Never log passwords, secret keys, or message content. Give
//! secret-bearing types a `Debug` impl that prints `***`."* A password arriving
//! from the wire will pass through structs that some future `tracing` call
//! formats with `{:?}`; the only reliable defence is for the type itself to
//! have nothing to leak.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits::{self, ValidationError};

/// A plaintext password, in transit between the wire and Argon2id.
///
/// Redacted by both `Debug` and `Display`, so there is no formatting mistake
/// available that puts it in a log. Read it back with [`Password::expose`],
/// which is deliberately awkward to type and easy to grep for.
///
/// This type never reaches storage: the server hashes it and drops it. Nothing
/// in the system can recover a password from what is on disk (PLAN §10).
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    /// Wraps a password without checking it. Use for a *login* attempt, where
    /// the rules that applied at registration may since have changed and the
    /// answer is "wrong password" either way.
    pub fn new(password: impl Into<String>) -> Self {
        Self(password.into())
    }

    /// Wraps a password destined for a *new* account, enforcing [`limits`].
    pub fn new_validated(password: impl Into<String>) -> Result<Self, ValidationError> {
        let password = password.into();
        limits::validate_password(&password)?;
        Ok(Self(password))
    }

    /// The plaintext. Every call site is a place a password could leak, so
    /// there should be exactly one: the hasher.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// A password **must** serialise as its plaintext: crossing the wire is the
/// one thing it is for, and QUIC + TLS 1.3 is what protects it on the way
/// (PLAN §10). The redaction that matters is [`fmt::Debug`], because that is
/// what a stray `tracing` call reaches for.
impl Serialize for Password {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Password {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Unvalidated on purpose: this is an inbound *attempt*, and the rules
        // may have changed since the account was created. The answer is
        // "wrong password" either way, and a validation error here would tell
        // a stranger which rule the password broke.
        String::deserialize(deserializer).map(Self)
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_never_formats_itself() {
        let password = Password::new("hunter2-and-then-some");
        assert_eq!(format!("{password:?}"), "Password(***)");
        assert_eq!(format!("{password}"), "***");
        // The failure this guards against is a struct printed with {:?} by a
        // log line nobody looked at twice.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Hello {
            username: String,
            password: Password,
        }
        let hello = Hello {
            username: "justin".into(),
            password: Password::new("hunter2-and-then-some"),
        };
        assert!(!format!("{hello:?}").contains("hunter2"));
    }

    #[test]
    fn a_password_crosses_the_wire_but_nothing_else() {
        let password = Password::new("correct horse");
        // It has to serialise, or nobody can ever log in.
        assert_eq!(
            serde_json::to_string(&password).expect("serialisable"),
            "\"correct horse\""
        );
        assert_eq!(
            serde_json::from_str::<Password>("\"correct horse\"").expect("parsable"),
            password
        );
    }

    #[test]
    fn validation_applies_only_where_it_should() {
        assert!(Password::new_validated("short").is_err());
        assert!(Password::new_validated("long enough to count").is_ok());
        // A login attempt with a too-short password is a failed login, not a
        // validation error — the server must not confirm which.
        assert_eq!(Password::new("short").expose(), "short");
    }
}
