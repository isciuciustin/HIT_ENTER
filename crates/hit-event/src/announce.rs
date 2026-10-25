//! A public group announcing itself on public Nostr relays (M13), so a
//! stranger can find it in *browse communities*.
//!
//! Kind **39436**, ours: addressable, `d` empty, **signed by the group
//! key** — the author *is* the group, so nobody can announce a group they
//! do not hold the key of. Not NIP-29's `39000`: ours is the whole state
//! (members, roles), large and for members' nodes, and a list reading
//! `39000`s would catch every NIP-29 relay's groups, which a HIT_ENTER app
//! cannot join.
//!
//! A group that goes private replaces its announcement with a withdrawal:
//! `["private"]`, no name, no link. Everything else here is the group's own
//! word — the member count included; a list shows it, it does not trust it.

use nostr::prelude::{Event, EventBuilder, Kind, PublicKey, Tag, Timestamp};

use crate::{GroupKey, KeyError};

pub const ANNOUNCEMENT: Kind = Kind::from_u16(39436);
/// The longest link an announcement may carry.
pub const MAX_LINK: usize = 2048;
const MAX_NAME: usize = 64;

/// What a list shows: the group, its name and size, and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub group: PublicKey,
    pub name: String,
    /// What it is about, in its own words; empty is none.
    pub about: String,
    pub members: u64,
    /// The group's public link (`hit-enter:join?g=…&p=…`, no code).
    pub link: String,
    pub at: Timestamp,
}

/// An announcement, or the withdrawal of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    Public(Announcement),
    Withdrawn { group: PublicKey, at: Timestamp },
}

impl Heard {
    pub fn group(&self) -> PublicKey {
        match self {
            Self::Public(a) => a.group,
            Self::Withdrawn { group, .. } => *group,
        }
    }

    pub fn at(&self) -> Timestamp {
        match self {
            Self::Public(a) => a.at,
            Self::Withdrawn { at, .. } => *at,
        }
    }
}

/// `key`'s group, announced: `name`, `about` (empty for none), `members`,
/// `link`.
pub fn announce(
    key: &GroupKey,
    name: &str,
    about: &str,
    members: u64,
    link: &str,
    at: Timestamp,
) -> Result<Event, KeyError> {
    let mut tags = vec![
        Tag::identifier(""),
        Tag::custom("name", [name]),
        Tag::custom("members", [members.to_string()]),
        Tag::custom("link", [link]),
    ];
    if !about.is_empty() {
        tags.push(Tag::custom("about", [about]));
    }
    key.sign(
        EventBuilder::new(ANNOUNCEMENT, "")
            .tags(tags)
            .custom_created_at(at),
    )
}

/// `key`'s group, no longer announced.
pub fn withdraw(key: &GroupKey, at: Timestamp) -> Result<Event, KeyError> {
    key.sign(
        EventBuilder::new(ANNOUNCEMENT, "")
            .tags([
                Tag::identifier(""),
                Tag::custom("private", Vec::<String>::new()),
            ])
            .custom_created_at(at),
    )
}

/// Reads an announcement from a public relay: `None` for anything not one
/// — a bad signature first.
pub fn read(event: &Event) -> Option<Heard> {
    if event.kind != ANNOUNCEMENT || event.verify().is_err() {
        return None;
    }
    let value = |name: &str| {
        event
            .tags
            .iter()
            .map(|t| t.as_slice())
            .find(|t| t.first().is_some_and(|n| n == name))
            .map(|t| t.get(1).cloned())
    };
    if value("d") != Some(Some(String::new())) {
        return None;
    }
    if value("private").is_some() {
        return Some(Heard::Withdrawn {
            group: event.pubkey,
            at: event.created_at,
        });
    }
    let clean = |s: String| -> String {
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .trim()
            .to_owned()
    };
    let name = clean(value("name")??);
    let about = value("about").flatten().map(clean).unwrap_or_default();
    let link = value("link")??;
    if name.is_empty()
        || name.chars().count() > MAX_NAME
        || about.chars().count() > crate::group::MAX_ABOUT
        || link.len() > MAX_LINK
    {
        return None;
    }
    Some(Heard::Public(Announcement {
        group: event.pubkey,
        name,
        about,
        members: value("members")??.parse().ok()?,
        link,
        at: event.created_at,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use nostr::prelude::{FinalizeEvent, Keys};

    use super::*;

    #[test]
    fn an_announcement_reads_back_and_a_withdrawal_says_so() {
        let key = GroupKey::generate();
        let at = Timestamp::from(1_700_000_000);
        let ev = announce(&key, "Town", "a town square", 12, "hit-enter:join?g=x", at).unwrap();
        assert_eq!(ev.pubkey, key.public_key());
        let Some(Heard::Public(a)) = read(&ev) else {
            panic!("not read")
        };
        assert_eq!((a.name.as_str(), a.members), ("Town", 12));
        assert_eq!(a.about, "a town square");
        assert_eq!(a.link, "hit-enter:join?g=x");
        let gone = withdraw(&key, at + 1).unwrap();
        assert_eq!(
            read(&gone),
            Some(Heard::Withdrawn {
                group: key.public_key(),
                at: at + 1
            })
        );
    }

    #[test]
    fn a_forged_or_empty_announcement_is_not_read() {
        let key = GroupKey::generate();
        let at = Timestamp::from(1_700_000_000);
        let mut ev = announce(&key, "Town", "", 1, "hit-enter:join?g=x", at).unwrap();
        ev.tags = nostr::prelude::Tags::from_list(vec![
            Tag::identifier(""),
            Tag::custom("name", ["Evil"]),
            Tag::custom("members", ["1"]),
            Tag::custom("link", ["hit-enter:join?g=x"]),
        ]);
        assert_eq!(read(&ev), None, "a changed announcement kept its signature");
        let nameless = announce(&key, "  ", "", 1, "x", at).unwrap();
        assert_eq!(read(&nameless), None);
        let other = EventBuilder::new(Kind::from_u16(39000), "")
            .tag(Tag::identifier(""))
            .finalize(&Keys::generate())
            .unwrap();
        assert_eq!(read(&other), None);
    }
}
