//! Where a headless anchor keeps its keys (M6).
//!
//! A desktop has an OS keychain; a Raspberry Pi or a VPS usually has no
//! Secret Service running. The rule stays: **no secret in a plain file**. On
//! a server the answer is **systemd credentials**: `systemd-creds encrypt`
//! seals the key at rest (with the host key, or a TPM), and the unit's
//! `LoadCredentialEncrypted=` hands it, decrypted, only to the service, in
//! `$CREDENTIALS_DIRECTORY` — a ramfs nobody else can read, gone when the
//! service stops. [`Credentials`] reads that directory and writes nothing:
//! a key is provisioned by piping it into `systemd-creds`, never by saving a
//! file (`docs/SELF_HOSTING.md`).
//!
//! The anchor's person key and any group key it holds for compaction are
//! filed under their npubs, as in the keychain.

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

use hit_event::{SecretStore, StoreError};
use zeroize::Zeroizing;

/// The read-only secret store systemd hands a service.
pub struct Credentials {
    dir: PathBuf,
}

impl Credentials {
    /// `$CREDENTIALS_DIRECTORY`, as systemd sets it for a unit that loads
    /// credentials.
    pub fn from_env() -> Result<Self, StoreError> {
        let dir = std::env::var_os("CREDENTIALS_DIRECTORY").ok_or_else(|| {
            StoreError::Unavailable(
                "no $CREDENTIALS_DIRECTORY: run under systemd with LoadCredentialEncrypted=".into(),
            )
        })?;
        Ok(Self::at(Path::new(&dir)))
    }

    pub fn at(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn path(&self, account: &str) -> Result<PathBuf, StoreError> {
        // An npub is all a credential name here ever is.
        if account.is_empty() || !account.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(StoreError::Backend("not a credential name".into()));
        }
        Ok(self.dir.join(account))
    }
}

impl SecretStore for Credentials {
    fn put(&self, _account: &str, _secret: &str) -> Result<(), StoreError> {
        Err(StoreError::Backend(
            "systemd credentials are provisioned with systemd-creds, not written by the service"
                .into(),
        ))
    }

    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, StoreError> {
        match std::fs::read_to_string(self.path(account)?) {
            Ok(s) => Ok(Some(Zeroizing::new(s.trim().to_owned()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Backend(e.to_string())),
        }
    }

    fn delete(&self, _account: &str) -> Result<(), StoreError> {
        Err(StoreError::Backend(
            "systemd credentials are removed with systemd, not by the service".into(),
        ))
    }
}

/// Writes `secret` to standard output **only if it is not a terminal** — so
/// it can be piped into `systemd-creds encrypt` and is never shown on a
/// screen or left in scrollback.
pub fn emit_to_pipe(secret: &str) -> anyhow::Result<()> {
    use std::io::Write;
    if std::io::stdout().is_terminal() {
        anyhow::bail!(
            "refusing to print a secret key to a terminal; pipe it: \
             … | systemd-creds encrypt --name=<npub> - /etc/credstore.encrypted/<npub>"
        );
    }
    let mut out = std::io::stdout().lock();
    out.write_all(secret.as_bytes())?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hit_event::Person;

    #[test]
    fn a_credential_is_read_by_npub_and_nothing_is_ever_written() {
        let dir = tempfile::tempdir().unwrap();
        let p = Person::generate();
        std::fs::write(
            dir.path().join(p.npub()),
            format!("{}\n", p.reveal_nsec_for_backup().as_str()),
        )
        .unwrap();
        let creds = Credentials::at(dir.path());
        let back = Person::load(&creds, &p.npub()).unwrap().unwrap();
        assert_eq!(back.npub(), p.npub());
        assert!(creds.get("npub1nobody").unwrap().is_none());
        assert!(creds.get("../escape").is_err());
        assert!(creds.put(&p.npub(), "x").is_err());
        assert!(creds.delete(&p.npub()).is_err());
    }
}
