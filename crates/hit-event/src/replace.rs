//! Which events replace which (NIP-01), in one place.
//!
//! The store and the [`BindingBook`](crate::BindingBook) both decide
//! replacement. If they disagreed, a peer's database could keep a binding
//! its book considers revoked, and sync would carry the wrong one onward —
//! so both call [`replaces`].

use nostr::prelude::Event;

use crate::{
    binding::{DEVICE_BINDING, is_revoked},
    group::{STATE_KINDS, seq_of},
};

/// Where an event lives, by its kind (NIP-01 "Kinds").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// Every event is kept.
    Regular,
    /// Newest per (kind, pubkey): 0, 3, 10000–19999.
    Replaceable,
    /// Newest per (kind, pubkey, `d`): 30000–39999. A missing `d` is `""`.
    Addressable(String),
    /// Never stored: 20000–29999.
    Ephemeral,
}

pub fn slot(event: &Event) -> Slot {
    let k = event.kind.as_u16();
    match k {
        0 | 3 | 10_000..20_000 => Slot::Replaceable,
        20_000..30_000 => Slot::Ephemeral,
        30_000..40_000 => Slot::Addressable(event.tags.identifier().unwrap_or_default()),
        _ => Slot::Regular,
    }
}

/// Does `new` replace `current`? Both must be in the same slot.
///
/// Newer `created_at` wins. On a tie the lower id wins, as NIP-01 says —
/// with two exceptions (`docs/PROTOCOL.md`):
///
/// - **a group's state (`39000`–`39003`) is ordered by `seq` first**, because
///   clocks on home machines lie and a counter signed by one key does not
///   (`DESIGN.md` §6). An event without a `seq` loses to one with.
/// - for device bindings, **a revocation beats a binding on a tie**, so a
///   re-sign and a revocation in the same second cannot leave a revoked
///   device alive.
pub fn replaces(new: &Event, current: &Event) -> bool {
    if STATE_KINDS.contains(&new.kind) && new.kind == current.kind {
        let (n, c) = (seq_of(new), seq_of(current));
        if n != c {
            return n > c;
        }
    }
    if new.created_at != current.created_at {
        return new.created_at > current.created_at;
    }
    if new.kind == DEVICE_BINDING && current.kind == DEVICE_BINDING {
        match (is_revoked(new), is_revoked(current)) {
            (true, false) => return true,
            (false, true) => return false,
            _ => {}
        }
    }
    new.id < current.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag, Timestamp};

    fn ev(keys: &Keys, kind: u16, at: u64, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::from(kind), "")
            .custom_created_at(Timestamp::from(at))
            .tags(tags)
            .finalize(keys)
            .unwrap()
    }

    #[test]
    fn slots_follow_nip01() {
        let k = Keys::generate();
        assert_eq!(slot(&ev(&k, 1, 1, vec![])), Slot::Regular);
        assert_eq!(slot(&ev(&k, 0, 1, vec![])), Slot::Replaceable);
        assert_eq!(slot(&ev(&k, 10_002, 1, vec![])), Slot::Replaceable);
        assert_eq!(slot(&ev(&k, 22_242, 1, vec![])), Slot::Ephemeral);
        assert_eq!(
            slot(&ev(&k, 30_023, 1, vec![Tag::identifier("a")])),
            Slot::Addressable("a".into())
        );
        assert_eq!(
            slot(&ev(&k, 30_023, 1, vec![])),
            Slot::Addressable(String::new())
        );
    }

    #[test]
    fn newer_wins_and_a_tie_goes_to_the_lower_id() {
        let k = Keys::generate();
        let (old, new) = (ev(&k, 0, 1, vec![]), ev(&k, 0, 2, vec![]));
        assert!(replaces(&new, &old) && !replaces(&old, &new));
        let a = ev(&k, 0, 5, vec![Tag::custom("x", ["a"])]);
        let b = ev(&k, 0, 5, vec![Tag::custom("x", ["b"])]);
        let (lo, hi) = if a.id < b.id { (a, b) } else { (b, a) };
        assert!(replaces(&lo, &hi) && !replaces(&hi, &lo));
    }

    #[test]
    fn group_state_is_ordered_by_seq_not_by_clock() {
        let k = Keys::generate();
        let d = Tag::identifier(k.public_key().to_hex());
        let seq = |n: &str| Tag::custom("seq", [n]);
        // Seq 2 signed on a clock that ran slow still replaces seq 1.
        let one = ev(&k, 39_002, 100, vec![d.clone(), seq("1")]);
        let two = ev(&k, 39_002, 50, vec![d.clone(), seq("2")]);
        assert!(replaces(&two, &one) && !replaces(&one, &two));
        // Same seq: back to the clock.
        let later = ev(&k, 39_002, 60, vec![d.clone(), seq("2")]);
        assert!(replaces(&later, &two));
        // No seq at all loses.
        let bare = ev(&k, 39_002, 999, vec![d]);
        assert!(!replaces(&bare, &one));
    }
}
