//! An event whose id and signature have been checked.
//!
//! CLAUDE.md: verify every event before storing, forwarding or showing it —
//! signature first, then `hit-group`'s permissions. [`Verified`] is the first
//! half as a type: the store takes nothing else, so an unverified event
//! cannot reach the disk by forgetting a call. The second half, permissions,
//! is `hit-group`'s, and the node asks it before storing.

use std::ops::Deref;

use nostr::prelude::Event;

/// Why an event failed verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("bad event id or signature")]
pub struct BadEvent;

/// An event with a valid NIP-01 id and Schnorr signature. Constructed only by
/// [`Verified::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified(Event);

impl Verified {
    pub fn check(event: Event) -> Result<Self, BadEvent> {
        event.verify().map_err(|_| BadEvent)?;
        Ok(Self(event))
    }

    pub fn into_event(self) -> Event {
        self.0
    }
}

impl Deref for Verified {
    type Target = Event;
    fn deref(&self) -> &Event {
        &self.0
    }
}
