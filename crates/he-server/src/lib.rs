//! The HIT_ENTER server.
//!
//! Deliberately free of any Tauri dependency: the same library backs the
//! desktop app's "host a space" switch and the headless `he-serverd` binary.
//! See `docs/PLAN.md` §2.1 — the host's own client dials this over iroh exactly
//! as a remote client does, so there is only ever one network path.
//!
//! M0 status: the crate exists, its dependencies resolve, and the identity
//! primitive below is exercised by a test. Storage and the protocol handler
//! land in M1 and M2.

use iroh::SecretKey;

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
}
