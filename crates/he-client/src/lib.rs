//! The HIT_ENTER client: dialing servers and mirroring their messages locally.
//!
//! M0 status: the crate exists and its dependencies resolve. Connection
//! management arrives in M2, the mirror database in M3.

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
