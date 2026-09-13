//! The HIT_ENTER client: dialing servers and mirroring their messages locally.
//!
//! [`Client`] binds an iroh endpoint, [`Session`] holds one logged-in
//! connection to one server, [`DeviceIdentity`] is the key a server enrols,
//! and [`Mirror`] is the local copy of everything this client has ever seen —
//! which is what makes the app work with the network off.
//!
//! **The host's own client is not special.** It dials its own server's
//! `EndpointId` through exactly this code, and iroh resolves that to a
//! loopback path by itself (PLAN §2.1).

pub mod conn;
pub mod error;
pub mod keys;
pub mod mirror;

pub use conn::{Client, Resumed, Session};
pub use error::{ClientError, Result};
pub use keys::DeviceIdentity;
pub use mirror::{Mirror, MirroredServer, Queued};

/// How a connection to a server is currently carrying traffic.
///
/// This is surfaced in the UI rather than hidden (`docs/PLAN.md` §6): a relayed
/// connection works but is slower, and a user who can see *why* understands
/// their network instead of concluding the app is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPath {
    /// Peer-to-peer. Hole punching succeeded.
    Direct,
    /// Routed via a relay because no direct path could be established.
    /// Still end-to-end encrypted: the relay forwards bytes it cannot read.
    Relayed,
    /// Dialing, or re-establishing after a network change.
    Connecting,
    Offline,
}

impl ConnectionPath {
    /// Whether messages can be sent right now.
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Direct | Self::Relayed)
    }
}

/// Reads the live path status off an open connection.
///
/// A connection typically opens through a relay and then *upgrades* to a
/// direct path once hole punching succeeds, without dropping — so this is a
/// snapshot of a value that changes, and the UI polls or watches it rather
/// than recording it once at connect time (PLAN §4, §6).
pub fn path_of(conn: &iroh::endpoint::Connection) -> ConnectionPath {
    let paths = conn.paths();
    // The selected path is the one carrying application data. A connection
    // with both open is direct, and saying "relayed" because a relay path
    // still exists would tell the user their network is worse than it is.
    for path in paths.iter() {
        if path.is_selected() {
            return if path.is_relay() {
                ConnectionPath::Relayed
            } else {
                ConnectionPath::Direct
            };
        }
    }
    if paths.is_empty() {
        ConnectionPath::Connecting
    } else {
        // Paths exist but none is selected yet: still settling.
        ConnectionPath::Connecting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relayed_connections_are_usable() {
        // Relayed is degraded, never broken — the UI must not present it as an
        // error state.
        assert!(ConnectionPath::Relayed.is_usable());
        assert!(ConnectionPath::Direct.is_usable());
        assert!(!ConnectionPath::Connecting.is_usable());
        assert!(!ConnectionPath::Offline.is_usable());
    }
}
