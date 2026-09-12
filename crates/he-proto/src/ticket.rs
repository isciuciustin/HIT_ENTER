//! Invite links — the one string a person has to send another person.
//!
//! ```text
//! hitenter://join?t=endpointb3r…&c=K7QP-2M4X-9WTZ
//! ```
//!
//! `t` is an iroh [`EndpointTicket`]: the server's `EndpointId` plus whatever
//! relay and direct addresses it knew about itself when the link was minted.
//! `c` is the registration code that gates account creation (PLAN §5, §11).
//!
//! **The link is self-verifying.** The `EndpointId` inside it *is* an ed25519
//! public key, so there is nothing to pin and no way to be silently redirected
//! to an impostor — a tampered link does not point at a different server, it
//! points at nothing. The address hints may go stale as the network changes;
//! the key never does, and discovery re-resolves from it.
//!
//! This lives in `he-proto` because both sides need it and neither may own it:
//! a host mints links and a client parses them, and a second implementation of
//! the format is a second opinion about what a valid link is.
//!
//! Nothing here does I/O. An [`EndpointAddr`] is an address, not a connection.

use std::fmt;
use std::str::FromStr;

use iroh_base::EndpointAddr;
use iroh_tickets::endpoint::EndpointTicket;

use crate::limits;

/// The URI scheme HIT_ENTER registers with the desktop.
pub const SCHEME: &str = "hitenter";

/// The only action a link can ask for, today. Named rather than implied so
/// that a future `hitenter://` verb does not have to break this one.
pub const ACTION: &str = "join";

/// Everything needed to find a space and be allowed into it.
///
/// The code is optional because the same string is also how a user points a
/// *second* machine at a space they already have an account on — there the
/// password enrols the device and no invite is involved (PLAN §3). A link
/// minted by a host always carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    addr: EndpointAddr,
    code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("that is not a hitenter:// link, an invite ticket, or a space address")]
    Unrecognised,
    #[error("this link is for {0:?}, which this version does not understand")]
    UnknownAction(String),
    #[error("the address in this link is not readable")]
    BadAddress,
    #[error("the invite code in this link is not readable")]
    BadCode,
}

impl InviteLink {
    pub fn new(addr: EndpointAddr, code: Option<String>) -> Self {
        Self { addr, code }
    }

    /// A link that carries only the address: "this is my space", with no
    /// permission to register attached.
    pub fn address_only(addr: EndpointAddr) -> Self {
        Self::new(addr, None)
    }

    /// Where to dial. Includes the relay and direct-address hints from the
    /// ticket, which is what makes a first connection fast; if they are stale
    /// iroh falls back to resolving the `EndpointId`.
    pub fn addr(&self) -> &EndpointAddr {
        &self.addr
    }

    /// The space's identity, as the string that appears everywhere else.
    pub fn endpoint_id(&self) -> String {
        self.addr.id.to_string()
    }

    /// The registration code, if this link carries one.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The home relay the host last knew about, for the mirror to remember.
    pub fn relay_url(&self) -> Option<String> {
        self.addr.relay_urls().next().map(ToString::to_string)
    }

    /// Parses anything a user might plausibly paste.
    ///
    /// In descending order of how it was meant to be used:
    ///
    /// - a full `hitenter://join?t=…&c=…` link;
    /// - a bare `endpoint…` ticket, optionally followed by a code;
    /// - a bare `EndpointId`, optionally followed by a code.
    ///
    /// Being liberal here is not sloppiness — the alternative is a user who
    /// pasted the two halves of an invite and is told, accurately and
    /// uselessly, that it is not a link.
    pub fn parse_relaxed(input: &str) -> Result<Self, LinkError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(LinkError::Unrecognised);
        }

        if starts_with_scheme(input) {
            return Self::from_str(input);
        }

        // Two words: an address and a code, in that order — which is what the
        // invite dialog used to put on the clipboard.
        let mut parts = input.split_whitespace();
        let address = parts.next().ok_or(LinkError::Unrecognised)?;
        let code = parts.next();
        if parts.next().is_some() {
            return Err(LinkError::Unrecognised);
        }

        let addr = parse_address(address)?;
        let code = code.map(validate_code).transpose()?;
        Ok(Self { addr, code })
    }
}

impl fmt::Display for InviteLink {
    /// The canonical form. Both values are restricted to characters that need
    /// no percent-encoding, which is what lets this survive being pasted
    /// through a chat client that helpfully linkifies things.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{SCHEME}://{ACTION}?t={}",
            EndpointTicket::new(self.addr.clone())
        )?;
        if let Some(code) = &self.code {
            write!(f, "&c={code}")?;
        }
        Ok(())
    }
}

impl FromStr for InviteLink {
    type Err = LinkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let rest = strip_scheme(s).ok_or(LinkError::Unrecognised)?;

        // `hitenter://join?…` and `hitenter:join?…` are the same link; which
        // one a chat client hands back depends on the chat client.
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        let (action, query) = match rest.split_once('?') {
            Some((action, query)) => (action, query),
            None => (rest, ""),
        };
        let action = action.trim_end_matches('/');
        if !action.eq_ignore_ascii_case(ACTION) {
            return Err(LinkError::UnknownAction(action.to_owned()));
        }

        let mut addr = None;
        let mut code = None;
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or(LinkError::Unrecognised)?;
            match key {
                "t" => addr = Some(parse_address(value)?),
                "c" => code = Some(validate_code(value)?),
                // An unknown parameter is a link from a newer version. Ignore
                // it: the two that matter are here, and refusing would turn a
                // forward-compatible addition into a broken invite.
                _ => {}
            }
        }

        Ok(Self {
            addr: addr.ok_or(LinkError::BadAddress)?,
            code,
        })
    }
}

fn starts_with_scheme(input: &str) -> bool {
    strip_scheme(input).is_some()
}

fn strip_scheme(input: &str) -> Option<&str> {
    let (scheme, rest) = input.split_once(':')?;
    scheme.eq_ignore_ascii_case(SCHEME).then_some(rest)
}

/// An `endpoint…` ticket or a bare `EndpointId`.
fn parse_address(value: &str) -> Result<EndpointAddr, LinkError> {
    if let Ok(ticket) = value.parse::<EndpointTicket>() {
        return Ok(ticket.into());
    }
    value
        .parse::<iroh_base::EndpointId>()
        .map(EndpointAddr::new)
        .map_err(|_| LinkError::BadAddress)
}

/// Keeps a code inside [`limits::INVITE_CODE_MAX_BYTES`] and free of anything
/// that would need escaping in a query string.
///
/// Whether the code is *real* is the server's business and is answered in
/// constant time; this only refuses what could never be one.
fn validate_code(code: &str) -> Result<String, LinkError> {
    limits::validate_invite_code(code)
        .map(|()| code.to_owned())
        .map_err(|_| LinkError::BadCode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::{RelayUrl, SecretKey, TransportAddr};

    fn addr() -> EndpointAddr {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        EndpointAddr::from_parts(
            id,
            [
                TransportAddr::Relay(
                    "https://euw-1.relay.n0.iroh.link."
                        .parse::<RelayUrl>()
                        .expect("relay url"),
                ),
                TransportAddr::Ip("192.0.2.10:41234".parse().expect("socket addr")),
            ],
        )
    }

    #[test]
    fn a_link_round_trips() {
        let link = InviteLink::new(addr(), Some("K7QP-2M4X-9WTZ".into()));
        let printed = link.to_string();
        assert!(printed.starts_with("hitenter://join?t="), "{printed}");
        assert_eq!(InviteLink::from_str(&printed).expect("parsable"), link);
    }

    #[test]
    fn the_hints_survive_the_round_trip() {
        // A ticket that lost its relay would still work — discovery would find
        // the key — but it would work *slowly*, which is the failure nobody
        // reports because the app merely feels bad.
        let link = InviteLink::new(addr(), Some("K7QP-2M4X-9WTZ".into()));
        let back = InviteLink::from_str(&link.to_string()).expect("parsable");
        assert_eq!(back.addr(), link.addr());
        assert_eq!(
            back.relay_url().as_deref(),
            Some("https://euw-1.relay.n0.iroh.link./")
        );
    }

    #[test]
    fn a_link_needs_no_percent_encoding() {
        let printed = InviteLink::new(addr(), Some("K7QP-2M4X-9WTZ".into())).to_string();
        assert!(
            printed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "?&=:/.-_".contains(c)),
            "{printed}"
        );
    }

    #[test]
    fn a_code_is_optional() {
        // "Point this second machine at my space" is the same string minus the
        // half that grants registration.
        let link = InviteLink::address_only(addr());
        let printed = link.to_string();
        assert!(!printed.contains("&c="), "{printed}");
        assert_eq!(InviteLink::from_str(&printed).expect("parsable"), link);
    }

    #[test]
    fn the_two_halves_of_an_invite_parse_as_a_link() {
        // What the M3 invite dialog put on the clipboard, and what a person
        // will paste for years after it stopped doing that.
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let pasted = format!("{id} K7QP-2M4X-9WTZ");
        let link = InviteLink::parse_relaxed(&pasted).expect("parsable");
        assert_eq!(link.endpoint_id(), id.to_string());
        assert_eq!(link.code(), Some("K7QP-2M4X-9WTZ"));
    }

    #[test]
    fn a_bare_address_parses_with_no_code() {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let link = InviteLink::parse_relaxed(&id.to_string()).expect("parsable");
        assert_eq!(link.code(), None);
        assert!(link.addr().addrs.is_empty(), "nothing to hint with");
    }

    #[test]
    fn a_bare_ticket_parses() {
        let ticket = EndpointTicket::new(addr()).to_string();
        let link = InviteLink::parse_relaxed(&ticket).expect("parsable");
        assert_eq!(link.addr(), &addr());
    }

    #[test]
    fn scheme_and_action_are_matched_loosely_but_not_wrongly() {
        let printed = InviteLink::new(addr(), None).to_string();
        let single_slash = printed.replace("hitenter://", "hitenter:");
        assert!(InviteLink::from_str(&single_slash).is_ok());
        assert!(InviteLink::from_str(&printed.to_uppercase().replace("T=", "t=")).is_err());

        let other_verb = printed.replace("join?", "leave?");
        assert!(matches!(
            InviteLink::from_str(&other_verb),
            Err(LinkError::UnknownAction(_))
        ));
    }

    #[test]
    fn an_unknown_parameter_does_not_break_an_invite() {
        // Forward compatibility, deliberately: a link from a newer version
        // that adds a hint must still join, not fail with a parse error.
        let printed = format!("{}&x=tomorrow", InviteLink::new(addr(), None));
        assert!(InviteLink::from_str(&printed).is_ok());
    }

    #[test]
    fn nonsense_is_refused() {
        for input in [
            "",
            "   ",
            "https://example.com/join",
            "hitenter://join?t=not-a-ticket",
            "hitenter://join",
            "one two three",
        ] {
            assert!(
                InviteLink::parse_relaxed(input).is_err(),
                "accepted {input:?}"
            );
        }
    }

    #[test]
    fn a_tampered_link_points_at_nothing_rather_than_somewhere_else() {
        // The address is a public key. Editing a character does not redirect
        // the invite to an attacker's server; it produces a key nobody holds,
        // or no readable ticket at all. This is why there is nothing to pin
        // and no fingerprint for a user to compare (PLAN §5).
        let original = InviteLink::new(addr(), None);
        let printed = original.to_string();
        let at = printed.find("t=").expect("a ticket") + "t=endpoint".len() + 4;

        let mut tampered: Vec<char> = printed.chars().collect();
        tampered[at] = if tampered[at] == 'a' { 'b' } else { 'a' };
        let tampered: String = tampered.into_iter().collect();
        assert_ne!(tampered, printed);

        match InviteLink::from_str(&tampered) {
            Err(_) => {}
            Ok(other) => assert_ne!(other.endpoint_id(), original.endpoint_id()),
        }
    }

    #[test]
    fn an_oversized_code_is_refused_before_it_reaches_a_server() {
        let printed = format!("{}&c={}", InviteLink::new(addr(), None), "A".repeat(4096));
        assert_eq!(InviteLink::from_str(&printed), Err(LinkError::BadCode));
    }
}
