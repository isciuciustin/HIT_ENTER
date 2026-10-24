//! Invite links (M3; HIT_ENTER's ticket idea, `docs/DESIGN.md` §5).
//!
//! ```text
//! hit-enter:join?g=<group npub>&c=<code>&p=<device hex>[@<ip:port>]&p=…
//! ```
//!
//! A link carries the group, an invite code, and a few devices to bootstrap
//! from — whoever made the link, an anchor. **A public group's link has no
//! code** (M13): its door is open, and the link only says where it is. An address after `@` is for a
//! LAN or a test with relays off; with relays on, the device id is enough.
//!
//! **A link is a capability.** Anyone who holds it can join until the
//! invite expires, so it travels like a password: not in a public channel.
//! It holds nothing secret beyond that — no key, no member list.

use std::{fmt, net::SocketAddr};

use iroh_base::EndpointId;
use nostr::prelude::{FromBech32, PublicKey, ToBech32};

/// The URL scheme an invite link uses — what the desktop hands the app.
pub const URL_SCHEME: &str = "hit-enter";
const SCHEME: &str = "hit-enter:join?";

/// At most this many bootstrap devices in a link: enough to survive a few
/// being offline, few enough to paste.
pub const MAX_PEERS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    pub group: PublicKey,
    /// `None`: a public group's link — no invite, nothing secret.
    pub code: Option<String>,
    pub peers: Vec<(EndpointId, Option<SocketAddr>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    #[error("not a HIT_ENTER invite link")]
    NotALink,
    #[error("the invite link has no {0}")]
    Missing(&'static str),
    #[error("the invite link's {0} is malformed")]
    Malformed(&'static str),
}

impl fmt::Display for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = self
            .group
            .to_bech32()
            .unwrap_or_else(|_| self.group.to_hex());
        write!(f, "{SCHEME}g={g}")?;
        if let Some(c) = &self.code {
            write!(f, "&c={c}")?;
        }
        for (id, addr) in self.peers.iter().take(MAX_PEERS) {
            write!(f, "&p={id}")?;
            if let Some(a) = addr {
                write!(f, "@{a}")?;
            }
        }
        Ok(())
    }
}

impl std::str::FromStr for InviteLink {
    type Err = InviteError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let query = s.trim().strip_prefix(SCHEME).ok_or(InviteError::NotALink)?;
        let (mut group, mut code, mut peers) = (None, None, Vec::new());
        for pair in query.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            match k {
                "g" => {
                    group = Some(
                        PublicKey::from_bech32(v)
                            .or_else(|_| PublicKey::from_hex(v))
                            .map_err(|_| InviteError::Malformed("group"))?,
                    );
                }
                "c" => {
                    if v.is_empty() || v.len() > 64 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(InviteError::Malformed("code"));
                    }
                    code = Some(v.to_owned());
                }
                "p" if peers.len() < MAX_PEERS => {
                    let (id, addr) = match v.split_once('@') {
                        Some((id, a)) => (
                            id,
                            Some(a.parse().map_err(|_| InviteError::Malformed("peer"))?),
                        ),
                        None => (v, None),
                    };
                    peers.push((
                        id.parse().map_err(|_| InviteError::Malformed("peer"))?,
                        addr,
                    ));
                }
                // Unknown keys are for a newer build; ignore them.
                _ => {}
            }
        }
        Ok(Self {
            group: group.ok_or(InviteError::Missing("group"))?,
            code,
            peers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::Keys;

    fn device() -> EndpointId {
        iroh_base::SecretKey::from_bytes(&[7u8; 32]).public()
    }

    #[test]
    fn a_link_round_trips() {
        let link = InviteLink {
            group: Keys::generate().public_key(),
            code: Some("00ff".into()),
            peers: vec![
                (device(), None),
                (device(), Some("127.0.0.1:4242".parse().unwrap())),
            ],
        };
        let text = link.to_string();
        assert!(text.starts_with("hit-enter:join?g=npub1"));
        assert_eq!(text.parse::<InviteLink>().unwrap(), link);

        // M13: a public group's link, no code.
        let open = InviteLink { code: None, ..link };
        let text = open.to_string();
        assert!(!text.contains("&c="));
        assert_eq!(text.parse::<InviteLink>().unwrap(), open);
    }

    #[test]
    fn junk_is_refused() {
        assert_eq!(
            "https://example.com".parse::<InviteLink>(),
            Err(InviteError::NotALink)
        );
        assert_eq!(
            "hit-enter:join?c=00".parse::<InviteLink>(),
            Err(InviteError::Missing("group"))
        );
        let g = Keys::generate().public_key().to_hex();
        assert_eq!(
            format!("hit-enter:join?g={g}&c=zz").parse::<InviteLink>(),
            Err(InviteError::Malformed("code"))
        );
    }
}
