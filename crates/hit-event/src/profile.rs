//! A person's profile: NIP-01 kind 0, as every Nostr client writes it (M4).
//!
//! The app uses one field, the display name. Everything else a profile
//! carries — `picture`, `about`, `lud16` for zaps — is kept as it was, so a
//! person who brings an existing Nostr identity does not lose their profile
//! to ours.

use nostr::prelude::{Event, EventBuilder, IntoEventBuilder, Kind, Metadata};

/// Longer names are cut here, on the way out and on the way in: a name is a
/// label in a member list, not a message.
pub const MAX_NAME: usize = 64;

/// `previous` (the person's current profile, if any) with its `name` set to
/// `name`, trimmed and cut to [`MAX_NAME`] characters.
pub fn with_name(previous: Option<&Event>, name: &str) -> EventBuilder {
    let mut meta = previous
        .filter(|e| e.kind == Kind::Metadata)
        .and_then(|e| Metadata::try_from(e).ok())
        .unwrap_or_default();
    meta.name = Some(clean(name));
    meta.into_event_builder()
}

/// `previous` with its name set and marked as an **anchor** (M6): NIP-24's
/// `bot` field. An anchor is a member like any other; this is how apps tell
/// it apart — to say so in the member list, and to name it in invite links
/// as a device that is always there.
pub fn as_anchor(previous: Option<&Event>, name: &str) -> EventBuilder {
    let mut meta = previous
        .filter(|e| e.kind == Kind::Metadata)
        .and_then(|e| Metadata::try_from(e).ok())
        .unwrap_or_default();
    meta.name = Some(clean(name));
    meta.custom_field("bot", true).into_event_builder()
}

/// Our profile field for an anchor's public `wss://` relay (M10): where a
/// Lightning address server can deliver a zap receipt for its groups.
pub const RELAY_FIELD: &str = "relay";

/// `previous` with its public relay set, or removed with `None`.
pub fn with_relay(previous: Option<&Event>, url: Option<&str>) -> EventBuilder {
    let mut meta = previous
        .filter(|e| e.kind == Kind::Metadata)
        .and_then(|e| Metadata::try_from(e).ok())
        .unwrap_or_default();
    match url {
        Some(u) => {
            meta.custom.insert(RELAY_FIELD.to_string(), u.trim().into());
        }
        None => {
            meta.custom.remove(RELAY_FIELD);
        }
    }
    meta.into_event_builder()
}

/// An anchor's public relay, if its profile names a `wss://` one (or a
/// loopback `ws://`, for tests).
pub fn relay_of(event: &Event) -> Option<String> {
    if event.kind != Kind::Metadata {
        return None;
    }
    let meta = Metadata::try_from(event).ok()?;
    let url = meta.custom.get(RELAY_FIELD)?.as_str()?;
    // `ws://` only to this machine: a test's anchor.
    let local = ["ws://127.0.0.1:", "ws://localhost:", "ws://[::1]:"]
        .iter()
        .any(|p| url.starts_with(p));
    ((url.starts_with("wss://") || local) && url.len() <= 256 && !url.contains(char::is_whitespace))
        .then(|| url.to_string())
}

/// Does this profile say it is an anchor (`"bot": true`)?
pub fn is_anchor(event: &Event) -> bool {
    event.kind == Kind::Metadata
        && Metadata::try_from(event)
            .ok()
            .and_then(|m| m.custom.get("bot").and_then(|v| v.as_bool()))
            .unwrap_or(false)
}

/// The name a profile shows: `name`, else `display_name`, cleaned. `None`
/// when it has neither, or is not a profile.
pub fn name_of(event: &Event) -> Option<String> {
    if event.kind != Kind::Metadata {
        return None;
    }
    let meta = Metadata::try_from(event).ok()?;
    [meta.name, meta.display_name]
        .into_iter()
        .flatten()
        .map(|n| clean(&n))
        .find(|n| !n.is_empty())
}

/// Trimmed, control characters dropped, at most [`MAX_NAME`] characters.
fn clean(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Person;

    #[test]
    fn a_name_is_set_and_everything_else_survives() {
        let p = Person::generate();
        let theirs = Metadata::new()
            .name("old")
            .about("from another client")
            .lud16("me@example.com");
        let before = p.sign(theirs.into_event_builder()).unwrap();
        let after = p.sign(with_name(Some(&before), "  new\u{7}  ")).unwrap();
        assert_eq!(name_of(&after).as_deref(), Some("new"));
        let meta = Metadata::try_from(&after).unwrap();
        assert_eq!(meta.about.as_deref(), Some("from another client"));
        assert_eq!(meta.lud16.as_deref(), Some("me@example.com"));
    }

    #[test]
    fn an_anchor_says_so_and_a_person_does_not() {
        let p = Person::generate();
        let anchor = p.sign(as_anchor(None, "the always-on one")).unwrap();
        assert!(is_anchor(&anchor));
        assert_eq!(name_of(&anchor).as_deref(), Some("the always-on one"));
        let person = p.sign(with_name(None, "a person")).unwrap();
        assert!(!is_anchor(&person));
        // Renaming an anchor keeps it one.
        let renamed = p.sign(with_name(Some(&anchor), "renamed")).unwrap();
        assert!(is_anchor(&renamed));
    }

    #[test]
    fn a_long_name_is_cut_and_an_empty_one_is_none() {
        let p = Person::generate();
        let long = p.sign(with_name(None, &"x".repeat(500))).unwrap();
        assert_eq!(name_of(&long).map(|n| n.chars().count()), Some(MAX_NAME));
        let empty = p.sign(with_name(None, "   ")).unwrap();
        assert_eq!(name_of(&empty), None);
    }
}
