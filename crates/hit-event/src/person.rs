//! The person: a Nostr keypair, the npub (`docs/DESIGN.md` §3).
//!
//! [`Person`] holds the secret key and is the only type here that does. It
//! has no `Debug` that could print the secret, no `Display`, no `Serialize`,
//! and exactly two ways out for the secret, both named for what they are for:
//! [`Person::to_ncryptsec`] (the backup we recommend) and
//! [`Person::reveal_nsec_for_backup`] (the backup step that shows the raw key
//! once, at onboarding). Everything else that needs the key — storing it,
//! signing with it — goes through a method, so the key never has to leave.

use std::fmt;

use nostr::prelude::{
    EncryptedSecretKey, Event, EventBuilder, FinalizeEvent, FromBech32, KeySecurity, Keys,
    PublicKey, ToBech32,
};
use zeroize::Zeroizing;

use crate::store::{SecretStore, StoreError};

/// scrypt cost for a backup we write: 2^16, about 64 MiB and a few hundred
/// milliseconds. NIP-49's own example. Reading accepts up to `nostr`'s cap
/// (2^18), so a backup made elsewhere with a higher cost still imports.
const NCRYPTSEC_LOG_N: u8 = 16;

/// Errors from making, importing or exporting a person key.
///
/// No variant carries the input: an error message is the kind of thing that
/// ends up in a log, and the input to an import *is* the secret.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("not a valid nsec or hex secret key")]
    InvalidSecret,
    #[error("not a valid ncryptsec")]
    InvalidNcryptsec,
    #[error("wrong password, or a corrupted ncryptsec")]
    WrongPassword,
    #[error("could not encrypt the key")]
    Encrypt,
    #[error("could not sign the event")]
    Sign,
    #[error("the keychain entry for this npub holds something that is not a key")]
    CorruptStoredKey,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A person's Nostr keypair. Holds the secret; see the module docs for the
/// only ways it leaves.
pub struct Person {
    keys: Keys,
}

impl Person {
    /// A brand-new identity, from the OS CSPRNG.
    pub fn generate() -> Self {
        Self {
            keys: Keys::generate(),
        }
    }

    /// Imports an existing identity from an `nsec1…` string or 64 hex chars.
    pub fn from_nsec(secret: &str) -> Result<Self, KeyError> {
        let keys = Keys::parse(secret.trim()).map_err(|_| KeyError::InvalidSecret)?;
        Ok(Self { keys })
    }

    /// Imports an identity from a NIP-49 `ncryptsec1…` backup.
    pub fn from_ncryptsec(ncryptsec: &str, password: &str) -> Result<Self, KeyError> {
        let encrypted = EncryptedSecretKey::from_bech32(ncryptsec.trim())
            .map_err(|_| KeyError::InvalidNcryptsec)?;
        let secret = encrypted
            .decrypt(password)
            .map_err(|_| KeyError::WrongPassword)?;
        Ok(Self {
            keys: Keys::new(secret),
        })
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    /// The `npub1…` form of [`Self::public_key`].
    pub fn npub(&self) -> String {
        // Encoding 32 bytes as bech32 cannot fail; if it ever did, the hex
        // form is still a correct, if less friendly, name for the person.
        self.keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| self.keys.public_key().to_hex())
    }

    /// A NIP-49 backup, encrypted with `password`. The backup we recommend:
    /// it can be written down or saved to a file without being the key.
    pub fn to_ncryptsec(&self, password: &str) -> Result<String, KeyError> {
        EncryptedSecretKey::new(
            self.keys.secret_key(),
            password,
            NCRYPTSEC_LOG_N,
            KeySecurity::Medium,
        )
        .and_then(|e| e.to_bech32())
        .map_err(|_| KeyError::Encrypt)
    }

    /// The raw `nsec1…`, for the onboarding backup step and nothing else.
    ///
    /// CLAUDE.md: the nsec is never displayed after onboarding. That is a rule
    /// for the caller — this crate cannot know when onboarding ends — so the
    /// name says what it is for, and the return value wipes itself on drop.
    pub fn reveal_nsec_for_backup(&self) -> Zeroizing<String> {
        Zeroizing::new(
            self.keys
                .secret_key()
                .to_bech32()
                .unwrap_or_else(|_| self.keys.secret_key().to_secret_hex()),
        )
    }

    /// Saves the secret into `store`, under this person's npub.
    pub fn save(&self, store: &dyn SecretStore) -> Result<(), KeyError> {
        let secret = self.reveal_nsec_for_backup();
        store.put(&self.npub(), &secret)?;
        Ok(())
    }

    /// Loads the person whose npub is `npub` from `store`, if it holds one.
    pub fn load(store: &dyn SecretStore, npub: &str) -> Result<Option<Self>, KeyError> {
        let Some(secret) = store.get(npub)? else {
            return Ok(None);
        };
        let person = Self::from_nsec(&secret).map_err(|_| KeyError::CorruptStoredKey)?;
        // A keychain entry filed under one npub must hold that npub's key.
        if person.npub() != npub {
            return Err(KeyError::CorruptStoredKey);
        }
        Ok(Some(person))
    }

    /// The keys, for this crate's NIP-44 and NIP-59 (`dm.rs`). Never
    /// public: the secret leaves `Person` only by the ways the module docs
    /// list.
    pub(crate) fn keys(&self) -> &Keys {
        &self.keys
    }

    /// Signs `builder` as this person.
    pub fn sign(&self, builder: EventBuilder) -> Result<Event, KeyError> {
        builder.finalize(&self.keys).map_err(|_| KeyError::Sign)
    }
}

/// Prints the npub only. There is deliberately no way to format the secret.
impl fmt::Debug for Person {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Person").field(&self.npub()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    #[test]
    fn nsec_round_trip_keeps_the_identity() {
        let p = Person::generate();
        let q = Person::from_nsec(&p.reveal_nsec_for_backup()).unwrap();
        assert_eq!(p.public_key(), q.public_key());
    }

    #[test]
    fn hex_secret_imports_too() {
        let p = Person::generate();
        let hex = p.keys.secret_key().to_secret_hex();
        assert_eq!(
            Person::from_nsec(&hex).unwrap().public_key(),
            p.public_key()
        );
    }

    #[test]
    fn ncryptsec_round_trip_and_wrong_password() {
        let p = Person::generate();
        let backup = p.to_ncryptsec("correct horse").unwrap();
        assert!(backup.starts_with("ncryptsec1"));
        let q = Person::from_ncryptsec(&backup, "correct horse").unwrap();
        assert_eq!(p.public_key(), q.public_key());
        assert!(matches!(
            Person::from_ncryptsec(&backup, "wrong"),
            Err(KeyError::WrongPassword)
        ));
    }

    #[test]
    fn garbage_is_refused_without_echoing_it() {
        let err = Person::from_nsec("nsec1notakey").unwrap_err();
        assert!(!err.to_string().contains("notakey"));
        assert!(matches!(
            Person::from_ncryptsec("ncryptsec1nope", "x"),
            Err(KeyError::InvalidNcryptsec)
        ));
    }

    #[test]
    fn store_round_trip_and_mismatched_entry() {
        let store = MemoryStore::default();
        let p = Person::generate();
        p.save(&store).unwrap();
        let q = Person::load(&store, &p.npub()).unwrap().unwrap();
        assert_eq!(p.public_key(), q.public_key());

        // Someone else's key filed under p's npub is refused, not adopted.
        let other = Person::generate();
        store
            .put(&p.npub(), &other.reveal_nsec_for_backup())
            .unwrap();
        assert!(matches!(
            Person::load(&store, &p.npub()),
            Err(KeyError::CorruptStoredKey)
        ));
        assert!(Person::load(&store, "npub1nobody").unwrap().is_none());
    }

    #[test]
    fn debug_shows_the_npub_and_not_the_secret() {
        let p = Person::generate();
        let shown = format!("{p:?}");
        assert!(shown.contains(&p.npub()));
        assert!(!shown.contains(p.reveal_nsec_for_backup().as_str()));
        assert!(!shown.contains(&p.keys.secret_key().to_secret_hex()));
    }
}
