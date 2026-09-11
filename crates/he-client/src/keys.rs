//! This device's iroh identity.
//!
//! The `EndpointId` derived from this key is what a server *enrols* (PLAN §3),
//! so it has to survive a restart: generate a fresh one and the server sees an
//! unknown device, and the user is asked for a password they were promised
//! they would never need again.
//!
//! It is not as catastrophic to lose as the server's key — no invite ticket
//! points at it, and a password re-enrols — but it is still a private key, so
//! it is written 0600 and never logged.

use std::path::Path;

use iroh::{EndpointId, SecretKey};

use crate::error::{ClientError, Result};

/// The device key, loaded or generated.
///
/// Deliberately has no `Debug` that prints the key. The public half is
/// [`Self::endpoint_id`], which is safe to show and is what a server owner
/// sees in their device list.
pub struct DeviceIdentity {
    secret: SecretKey,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The public half only: a struct printed by a stray `tracing` call
        // must not be how a secret key reaches a bug report (PLAN §11).
        f.debug_struct("DeviceIdentity")
            .field("endpoint_id", &self.endpoint_id())
            .finish_non_exhaustive()
    }
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::generate(),
        }
    }

    /// Loads the key at `path`, creating it on a first run.
    ///
    /// Stored as 32 raw bytes rather than anything encoded, so there is no
    /// format to get wrong and nothing that looks copy-pasteable.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let bytes: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| ClientError::CorruptIdentity(path.display().to_string()))?;
                Ok(Self {
                    secret: SecretKey::from_bytes(&bytes),
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let identity = Self::generate();
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, identity.secret.to_bytes())?;
                restrict_permissions(path);
                Ok(identity)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// The public identity a server enrols. Safe to print and to share.
    pub fn endpoint_id(&self) -> EndpointId {
        self.secret.public()
    }

    pub fn secret_key(&self) -> &SecretKey {
        &self.secret
    }
}

/// Makes the key readable only by its owner. Best effort: a filesystem that
/// cannot express this is a reason to warn, not a reason to refuse to start.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(path = %path.display(), %err, "could not restrict device key permissions");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_survives_a_restart() {
        // If it did not, every restart would look like a new device and ask
        // for a password that enrolment exists to stop asking for.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device.key");

        let first = DeviceIdentity::load_or_create(&path).expect("create");
        let second = DeviceIdentity::load_or_create(&path).expect("load");
        assert_eq!(first.endpoint_id(), second.endpoint_id());
    }

    #[test]
    fn a_truncated_key_file_is_an_error_not_a_new_identity() {
        // Silently generating a new key would turn a corrupt file into a
        // mysterious password prompt.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device.key");
        std::fs::write(&path, b"too short").expect("write");
        assert!(matches!(
            DeviceIdentity::load_or_create(&path),
            Err(ClientError::CorruptIdentity(_))
        ));
    }

    #[test]
    #[cfg(unix)]
    fn the_key_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device.key");
        DeviceIdentity::load_or_create(&path).expect("create");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "mode was {:o}", mode & 0o777);
    }

    #[test]
    fn the_secret_key_never_formats_itself() {
        let identity = DeviceIdentity::generate();
        let printed = format!("{identity:?}");
        let hex: String = identity
            .secret_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(!printed.contains(&hex), "{printed}");
    }
}
