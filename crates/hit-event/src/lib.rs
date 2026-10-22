//! Our Nostr event kinds, tag builders and validation, on top of the `nostr`
//! crate. **No I/O**, except the OS keychain behind the `keychain` feature
//! (M1, `docs/PLAN.md`).
//!
//! - [`person`]: the person's key — generate, import (`nsec`, NIP-49
//!   `ncryptsec`), back up, sign.
//! - [`store`]: where the nsec lives — the OS keychain, or memory.
//! - [`binding`]: *"npub P speaks from device D"*, and [`verify_binding`].
//! - [`group`]: NIP-29 group events and the group key — their shape, not
//!   their meaning (that is `hit-group`).
//! - [`dm`]: NIP-17 direct messages — wrap, open, the inbox list.
//! - [`profile`]: a person's kind 0 — the name others see.
//! - [`verified`]: an event whose signature has been checked — the only kind
//!   the store takes.
//! - [`replace`]: NIP-01 replaceable and addressable slots, decided once.
//! - [`zap`]: NIP-57 zap requests and receipts, bolt11's fields, and the
//!   carrier that brings a receipt into a group (M10).
//!
//! The rules that bind this crate:
//!
//! - `verify_binding` takes the connection's `EndpointId` as a required
//!   argument. **There is no signature-only entry point.**
//! - Person key, group key, device key: three keys, never conflated.
//! - The nsec lives in the OS keychain. Never logged, never displayed after
//!   onboarding, never sent, never written to a plain file.
//! - Every kind we emit is documented in `docs/PROTOCOL.md` in the same commit.

pub mod binding;
pub mod dm;
pub mod group;
pub mod person;
pub mod profile;
pub mod replace;
pub mod store;
pub mod verified;
pub mod zap;

pub use binding::{
    BindingBook, BindingError, DEVICE_BINDING, Inserted, is_revoked, needs_renewal, sign_binding,
    sign_revocation, verify_binding, verify_binding_at,
};
pub use group::GroupKey;
pub use person::{KeyError, Person};
pub use replace::{Slot, replaces, slot};
#[cfg(feature = "keychain")]
pub use store::Keychain;
pub use store::{MemoryStore, SecretStore, StoreError};
pub use verified::{BadEvent, Verified};

/// Re-exported so callers name the same types this crate checks.
pub use iroh_base::{EndpointId, SecretKey as DeviceKey};
pub use nostr::prelude::{Event, PublicKey, Timestamp};
