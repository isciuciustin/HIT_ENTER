//! The `hit/nostr/1` wire: the ALPN, length-prefixed framing and its limits,
//! and the binding frame that opens every session (M2, `docs/PLAN.md`;
//! `docs/DESIGN.md` §4; `docs/PROTOCOL.md`). **No I/O**, except [`io`],
//! behind the off-by-default `io` feature.
//!
//! Frame bodies are NIP-01 and NIP-77 messages, unchanged. The framing is
//! HIT_ENTER's `he-proto` (MIT OR Apache-2.0, notice in
//! `THIRD_PARTY_NOTICES.md`), ported: same header, same cap, same
//! check-before-allocate rule.

pub mod contact;
pub mod frame;
pub mod hello;
pub mod invite;
#[cfg(feature = "io")]
pub mod io;
pub mod limits;

/// The chat plane's request/response ALPN. A node speaks NIP-01 and NIP-77
/// over it, as a relay and as a client, with no difference in code.
pub const ALPN: &[u8] = b"hit/nostr/1";

/// The prefix of a group's gossip ALPN (M3). **One gossip instance, and
/// ALPN, per group**: a connection carries every topic, so the ALPN is the
/// unit a gate can refuse (spike S2, `docs/DESIGN.md` §4).
pub const GOSSIP_ALPN_PREFIX: &str = "hit/gossip/1/";

/// `hit/gossip/1/<group hex>`.
pub fn gossip_alpn(group: &nostr::prelude::PublicKey) -> Vec<u8> {
    format!("{GOSSIP_ALPN_PREFIX}{}", group.to_hex()).into_bytes()
}

/// The group a gossip ALPN names, if it is one.
pub fn gossip_group(alpn: &[u8]) -> Option<nostr::prelude::PublicKey> {
    let hex = std::str::from_utf8(alpn)
        .ok()?
        .strip_prefix(GOSSIP_ALPN_PREFIX)?;
    nostr::prelude::PublicKey::from_hex(hex).ok()
}
