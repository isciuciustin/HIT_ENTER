//! Contact links: how two people with no group in common first reach each
//! other (M9, for 1:1 calls and DMs).
//!
//! ```text
//! hit-enter:contact?p=<person npub>&d=<device hex>[@<ip:port>]&d=…
//! ```
//!
//! A person and the devices they can be reached at. Opening one dials a
//! device once: the `hit/nostr/1` hello proves the device speaks for that
//! person (its binding, checked against the connection), and the binding is
//! kept. It carries no binding itself — bindings expire within days, and a
//! contact link is meant to last as long as the device.
//!
//! **Sharing it lets someone write to you, not call you**: an app rings only
//! for someone its person has written to (`hit-voice`). It does reveal your
//! device ids, so whoever holds it can dial your devices and learn your IP
//! address, as a group member can.

use std::{fmt, net::SocketAddr};

use iroh_base::EndpointId;
use nostr::prelude::{FromBech32, PublicKey, ToBech32};

const SCHEME: &str = "hit-enter:contact?";

/// At most this many devices in a link.
pub const MAX_DEVICES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactLink {
    pub person: PublicKey,
    pub devices: Vec<(EndpointId, Option<SocketAddr>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContactError {
    #[error("not a HIT_ENTER contact link")]
    NotALink,
    #[error("the contact link has no {0}")]
    Missing(&'static str),
    #[error("the contact link's {0} is malformed")]
    Malformed(&'static str),
}

impl fmt::Display for ContactLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self
            .person
            .to_bech32()
            .unwrap_or_else(|_| self.person.to_hex());
        write!(f, "{SCHEME}p={p}")?;
        for (id, addr) in self.devices.iter().take(MAX_DEVICES) {
            write!(f, "&d={id}")?;
            if let Some(a) = addr {
                write!(f, "@{a}")?;
            }
        }
        Ok(())
    }
}

impl std::str::FromStr for ContactLink {
    type Err = ContactError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let query = s
            .trim()
            .strip_prefix(SCHEME)
            .ok_or(ContactError::NotALink)?;
        let (mut person, mut devices) = (None, Vec::new());
        for pair in query.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            match k {
                "p" => {
                    person = Some(
                        PublicKey::from_bech32(v)
                            .or_else(|_| PublicKey::from_hex(v))
                            .map_err(|_| ContactError::Malformed("person"))?,
                    );
                }
                "d" if devices.len() < MAX_DEVICES => {
                    let (id, addr) = match v.split_once('@') {
                        Some((id, a)) => (
                            id,
                            Some(a.parse().map_err(|_| ContactError::Malformed("device"))?),
                        ),
                        None => (v, None),
                    };
                    devices.push((
                        id.parse().map_err(|_| ContactError::Malformed("device"))?,
                        addr,
                    ));
                }
                // Unknown keys are for a newer build; ignore them.
                _ => {}
            }
        }
        if devices.is_empty() {
            return Err(ContactError::Missing("device"));
        }
        Ok(Self {
            person: person.ok_or(ContactError::Missing("person"))?,
            devices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::Keys;

    fn device(n: u8) -> EndpointId {
        iroh_base::SecretKey::from_bytes(&[n; 32]).public()
    }

    #[test]
    fn a_contact_link_round_trips() {
        let link = ContactLink {
            person: Keys::generate().public_key(),
            devices: vec![
                (device(1), None),
                (device(2), Some("127.0.0.1:4242".parse().unwrap())),
            ],
        };
        let text = link.to_string();
        assert!(text.starts_with("hit-enter:contact?p=npub1"));
        assert_eq!(text.parse::<ContactLink>().unwrap(), link);
    }

    #[test]
    fn junk_and_deviceless_links_are_refused() {
        assert_eq!(
            "hit-enter:join?g=x".parse::<ContactLink>(),
            Err(ContactError::NotALink)
        );
        let p = Keys::generate().public_key().to_bech32().unwrap();
        assert_eq!(
            format!("hit-enter:contact?p={p}").parse::<ContactLink>(),
            Err(ContactError::Missing("device"))
        );
        assert!(
            format!("hit-enter:contact?p={p}&d=zz")
                .parse::<ContactLink>()
                .is_err()
        );
    }
}
