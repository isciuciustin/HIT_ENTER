//! Where the nsec lives: the OS keychain, behind a trait (`docs/DESIGN.md` §3).
//!
//! CLAUDE.md: the nsec is never logged, never sent, and never written to a
//! plain file. [`Keychain`] is the store the app uses. [`MemoryStore`] is for
//! tests and throwaway CLI identities, and is memory, not a file, so it keeps
//! the rule too. There is deliberately no file-backed store: the one an app
//! would reach for "until the keychain works" is the one that breaks the rule.

use std::{collections::HashMap, sync::Mutex};

use zeroize::Zeroizing;

/// Errors from a secret store. Never carries the secret.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the OS keychain is not available: {0}")]
    Unavailable(String),
    #[error("the OS keychain refused the operation: {0}")]
    Backend(String),
}

/// A place that holds secrets under an account name (we use the npub).
pub trait SecretStore {
    fn put(&self, account: &str, secret: &str) -> Result<(), StoreError>;
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, StoreError>;
    /// Removing an absent entry is not an error.
    fn delete(&self, account: &str) -> Result<(), StoreError>;
}

/// Secrets in process memory. Never touches the disk.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Zeroizing<String>>>,
}

impl MemoryStore {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Zeroizing<String>>> {
        // A poisoned lock means a panic elsewhere while holding it; the map
        // itself is still consistent, so keep going rather than panic again.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SecretStore for MemoryStore {
    fn put(&self, account: &str, secret: &str) -> Result<(), StoreError> {
        self.entries()
            .insert(account.to_owned(), Zeroizing::new(secret.to_owned()));
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, StoreError> {
        Ok(self.entries().get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        self.entries().remove(account);
        Ok(())
    }
}

/// The OS keychain: Keychain on macOS, Credential Manager on Windows, the
/// Secret Service on Linux.
#[cfg(feature = "keychain")]
pub struct Keychain {
    service: String,
}

#[cfg(feature = "keychain")]
impl Keychain {
    /// The service name the app files its keys under.
    pub const APP_SERVICE: &str = "hit-enter";

    /// A keychain view under `service`. The app uses [`Self::APP_SERVICE`];
    /// tests use a unique name so they never touch a real identity.
    pub fn new(service: impl Into<String>) -> Result<Self, StoreError> {
        if let Err(e) = keyring::Entry::store_status() {
            return Err(StoreError::Unavailable(e.to_string()));
        }
        Ok(Self {
            service: service.into(),
        })
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, account).map_err(backend)
    }
}

/// keyring's `Debug` for `BadEncoding` and `BadDataFormat` prints the raw
/// secret bytes it failed to decode; its `Display` does not. So errors cross
/// into ours through `Display`, here and nowhere else — never `{e:?}`.
#[cfg(feature = "keychain")]
fn backend(e: keyring::Error) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[cfg(feature = "keychain")]
impl SecretStore for Keychain {
    fn put(&self, account: &str, secret: &str) -> Result<(), StoreError> {
        self.entry(account)?.set_password(secret).map_err(backend)
    }

    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, StoreError> {
        match self.entry(account)?.get_password() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }
}
