//! What a NIP-29 client expects to read about a group, signed by the relay
//! — a **translation**, not a pass-through (`PROTOCOL.md`, "Open").
//!
//! NIP-29 says the relay signs `39000`–`39003` with its own key, and states
//! the group's metadata, admins, members and roles in plain tags. Ours are
//! signed by the group key, carry `seq`, `channel`, `override` and `role`
//! rows a NIP-29 client does not know, and the authority behind them is the
//! fold, not a relay. So the anchor states, signed by its own key, what its
//! fold says — **served over `ws://` only, never stored, never synced**: a
//! peer of the mesh would rightly refuse them, as state not signed by the
//! group.
//!
//! The group id a NIP-29 client sees is the group key in hex — the value of
//! every message's `h` tag already.

use hit_event::{Person, group::DEFAULT_ROLE};
use hit_group::GroupState;
use nostr::prelude::{Event, EventBuilder, Kind, Tag, Timestamp};

pub const NIP29_METADATA: Kind = Kind::from_u16(39000);
pub const NIP29_ADMINS: Kind = Kind::from_u16(39001);
pub const NIP29_MEMBERS: Kind = Kind::from_u16(39002);
pub const NIP29_ROLES: Kind = Kind::from_u16(39003);

/// The four, for one group, signed by `relay` at `at`.
pub fn translate(
    relay: &Person,
    state: &GroupState,
    names: &dyn Fn(&nostr::prelude::PublicKey) -> Option<String>,
    at: Timestamp,
) -> anyhow::Result<Vec<Event>> {
    let d = Tag::identifier(state.id().to_hex());
    let row = |name: &str, values: Vec<String>| Tag::custom(name, values);

    // NIP-29's "public" and "open" for a public group (M13), else
    // "private" and "closed" — the same words the group key signs.
    let mut meta = EventBuilder::new(NIP29_METADATA, "")
        .tag(d.clone())
        .tag(row("name", vec![state.name().to_owned()]))
        .tag(row(
            "about",
            // The group's own words (M13), else what it is.
            vec![if state.about().is_empty() {
                "A HIT_ENTER group, read through one of its anchors.".into()
            } else {
                state.about().to_owned()
            }],
        ));
    for t in hit_event::group::visibility_tags(state.is_public()) {
        meta = meta.tag(t);
    }

    let mut admins = EventBuilder::new(NIP29_ADMINS, "").tag(d.clone());
    if let Some(owner) = state.owner() {
        admins = admins.tag(row("p", vec![owner.to_hex(), "owner".into()]));
    }
    for r in state.roles() {
        if r.id == DEFAULT_ROLE {
            continue;
        }
        for p in state.holders_of(&r.id) {
            admins = admins.tag(row("p", vec![p.to_hex(), r.name.clone()]));
        }
    }

    let mut members = EventBuilder::new(NIP29_MEMBERS, "").tag(d.clone());
    for p in state.members() {
        let mut values = vec![p.to_hex()];
        if let Some(n) = names(p) {
            values.push(n);
        }
        members = members.tag(row("p", values));
    }

    let mut roles = EventBuilder::new(NIP29_ROLES, "").tag(d);
    for r in state.roles() {
        roles = roles.tag(row("role", vec![r.name.clone()]));
    }

    [meta, admins, members, roles]
        .into_iter()
        .map(|b| Ok(relay.sign(b.custom_created_at(at))?))
        .collect()
}
