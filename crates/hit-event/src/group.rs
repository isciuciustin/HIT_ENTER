//! Group events: NIP-29's vocabulary, our authority (M3, `docs/DESIGN.md`
//! §5–6; wire format in `docs/PROTOCOL.md`).
//!
//! This module knows what the events **look like** — kinds, tags, the group
//! key, how to build and parse each one. What they **mean** — who may do
//! what, the state fold — is `hit-group`'s, and nothing here answers a
//! permission question.
//!
//! - A group's address is its own public key, `G`. Every group event carries
//!   `["h", G]`, except the group key's own state (`39000`–`39003`), which is
//!   addressable with `["d", G]` and signed by `G`.
//! - The group's state is four events signed by the group key with the same
//!   `["seq", N]`: metadata and channels, role assignments, members, roles.
//! - Admin and member actions (`9000`, `9001`, `9009`, `9021`, `9022`) are
//!   signed by people and take effect on every peer that can check them.
//! - **Person key, group key, device key: three keys, never conflated.**
//!   [`GroupKey`] is its own type for that reason, even though it is the
//!   same kind of key as a [`Person`](crate::Person)'s.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use nostr::prelude::{
    Event, EventBuilder, EventId, FinalizeEvent, Keys, Kind, PublicKey, Tag, Timestamp, ToBech32,
};
use zeroize::Zeroizing;

use crate::{KeyError, SecretStore};

/// A chat message: `["h", G]`, `["channel", C]` (NIP-29 kind 9).
pub const CHAT_MESSAGE: Kind = Kind::from_u16(9);
/// A reaction to a message (NIP-25): `["h", G]`, `["channel", C]`,
/// `["e", message]`, `["p", its author]`, `["k", "9"]`; the content is the
/// emoji (M4).
pub const REACTION: Kind = Kind::from_u16(7);
/// Its author withdrawing their own messages or reactions (NIP-09):
/// `["h", G]`, `["channel", C]`, one `["e", id]` per target (M4).
pub const DELETION: Kind = Kind::from_u16(5);
/// "I am here", on a group's gossip only — ephemeral, never stored:
/// `["h", G]` (M4, ours, provisional; the ephemeral twin of 30436).
pub const PRESENCE: Kind = Kind::from_u16(20436);
/// "I am in this voice channel" — or, with `["state", "out"]`, "I left" —
/// on a group's gossip only, ephemeral (M9, ours, provisional): who is in a
/// call, and so whom to dial. Sent every few seconds while in one.
pub const VOICE_PRESENCE: Kind = Kind::from_u16(20437);
/// The most people in one voice channel: a mesh, by `DESIGN.md` §10's
/// arithmetic (8 → 7 × 32 kbit/s up each).
pub const MAX_VOICE: usize = 8;

/// Kinds posted *in a channel*: they carry `channel`, and whoever cannot
/// view that channel neither reads nor stores them.
pub const CHANNEL_KINDS: [Kind; 5] = [
    CHAT_MESSAGE,
    REACTION,
    DELETION,
    DELETE_EVENT,
    crate::zap::ZAP_CARRIER,
];
/// The longest slowmode, in seconds: six hours, as HIT_ENTER_2.
pub const MAX_SLOWMODE: u32 = 6 * 60 * 60;
/// The longest a group may make a newcomer wait before posting, in
/// seconds: a day (M13).
pub const MAX_NEWCOMER_WAIT: u32 = 24 * 60 * 60;
/// The shortest wait a **public** group's newcomer has, whatever the
/// group says: five minutes (M13, the owner's floor). In the rule, so no
/// edit and no client gets under it.
pub const PUBLIC_NEWCOMER_FLOOR: u32 = 5 * 60;
/// Longest reaction, in characters: an emoji, or a short word.
pub const MAX_REACTION: usize = 32;
/// Put a person in the group and set their roles: `["p", target, role…]`.
pub const PUT_USER: Kind = Kind::from_u16(9000);
/// Remove a person from the group: `["p", target]`. With `["ban"]` they
/// may not come back until an unban; with `["unban"]` instead, that unban —
/// which removes nobody (M5).
pub const REMOVE_USER: Kind = Kind::from_u16(9001);
/// Change the group: its name, channels (and their slowmode), channel
/// overrides, and role definitions — one or more edit tags, all applied or
/// none (NIP-29's kind; our tags, M5).
pub const EDIT_METADATA: Kind = Kind::from_u16(9002);
/// A moderator removing messages or reactions in a channel:
/// `["channel", C]`, one `["e", id]` per target (NIP-29's kind, M5).
pub const DELETE_EVENT: Kind = Kind::from_u16(9005);
/// Mute a member until a time, or lift a mute: `["p", target, until]`,
/// `until` 0 to lift (ours, provisional, M5).
pub const MUTE_USER: Kind = Kind::from_u16(9030);
/// The owner's half of a group's creation: "I am the owner of `G`".
pub const CREATE_GROUP: Kind = Kind::from_u16(9007);
/// An invite: `["code", X]`, optional NIP-40 `expiration`.
pub const CREATE_INVITE: Kind = Kind::from_u16(9009);
/// A join presenting an invite code, `["code", X]` — or none, to a
/// public group (M13).
pub const JOIN: Kind = Kind::from_u16(9021);
/// Leaving.
pub const LEAVE: Kind = Kind::from_u16(9022);
/// The group key's state: metadata and channels.
pub const GROUP_METADATA: Kind = Kind::from_u16(39000);
/// The group key's state: the owner and role assignments.
pub const GROUP_ADMINS: Kind = Kind::from_u16(39001);
/// The group key's state: members.
pub const GROUP_MEMBERS: Kind = Kind::from_u16(39002);
/// The group key's state: role definitions.
pub const GROUP_ROLES: Kind = Kind::from_u16(39003);

/// The four state kinds, in the order a snapshot is signed.
pub const STATE_KINDS: [Kind; 4] = [GROUP_METADATA, GROUP_ADMINS, GROUP_MEMBERS, GROUP_ROLES];
/// The actions the fold applies.
pub const ACTION_KINDS: [Kind; 7] = [
    PUT_USER,
    REMOVE_USER,
    EDIT_METADATA,
    MUTE_USER,
    CREATE_INVITE,
    JOIN,
    LEAVE,
];

/// The role every member holds. Defined in `39003` like any other, never
/// assigned.
pub const DEFAULT_ROLE: &str = "member";
/// The marker on the owner's `39001` entry. Not a role: no role may use it.
pub const OWNER_MARK: &str = "owner";

/// Does `kind` belong to a group (and so carry `h` or, for state, `d`)?
pub fn is_group_kind(kind: Kind) -> bool {
    CHANNEL_KINDS.contains(&kind)
        || kind == PRESENCE
        || kind == VOICE_PRESENCE
        || kind == CREATE_GROUP
        || STATE_KINDS.contains(&kind)
        || ACTION_KINDS.contains(&kind)
}

/// The group an event belongs to: `d` for the group key's state, `h` for
/// everything else. `None` for a non-group kind or a malformed tag.
pub fn group_of(event: &Event) -> Option<PublicKey> {
    if STATE_KINDS.contains(&event.kind) {
        return PublicKey::from_hex(&event.tags.identifier()?).ok();
    }
    if !is_group_kind(event.kind) {
        return None;
    }
    PublicKey::from_hex(first_value(event, "h")?).ok()
}

/// The channel a message, reaction, deletion or voice presence is in.
pub fn channel_of(event: &Event) -> Option<&str> {
    if !CHANNEL_KINDS.contains(&event.kind) && event.kind != VOICE_PRESENCE {
        return None;
    }
    first_value(event, "channel")
}

/// The events a reaction or deletion points at: its `e` tags.
pub fn targets_of(event: &Event) -> Vec<EventId> {
    event
        .tags
        .iter()
        .filter_map(|t| match t.as_slice() {
            [n, v, ..] if n == "e" => EventId::from_hex(v).ok(),
            _ => None,
        })
        .collect()
}

/// The `seq` of a state event; `None` if absent or not a number.
pub fn seq_of(event: &Event) -> Option<u64> {
    first_value(event, "seq")?.parse().ok()
}

fn first_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event.tags.iter().find_map(|t| match t.as_slice() {
        [n, v, ..] if n == name => Some(v.as_str()),
        _ => None,
    })
}

fn tag<I, S>(name: &str, values: I) -> Tag
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Tag::custom(name, values)
}

fn h(group: &PublicKey) -> Tag {
    tag("h", [group.to_hex()])
}

// ---------------------------------------------------------------------------
// The group key

/// A group's keypair. Its public key **is** the group; its secret signs the
/// group's state and lives in the OS keychain of the owner (and of any
/// anchor the owner hands it to) — "treat it like an SSH host key".
pub struct GroupKey {
    keys: Keys,
}

impl GroupKey {
    pub fn generate() -> Self {
        Self {
            keys: Keys::generate(),
        }
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    pub fn npub(&self) -> String {
        self.keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| self.keys.public_key().to_hex())
    }

    /// Saves the secret into `store`, under the group's npub.
    pub fn save(&self, store: &dyn SecretStore) -> Result<(), KeyError> {
        let secret = Zeroizing::new(self.keys.secret_key().to_secret_hex());
        store.put(&self.npub(), &secret)?;
        Ok(())
    }

    /// Loads the group key for `npub` from `store`, if it holds one.
    pub fn load(store: &dyn SecretStore, npub: &str) -> Result<Option<Self>, KeyError> {
        let Some(secret) = store.get(npub)? else {
            return Ok(None);
        };
        let keys = Keys::parse(secret.as_str()).map_err(|_| KeyError::CorruptStoredKey)?;
        let key = Self { keys };
        if key.npub() != npub {
            return Err(KeyError::CorruptStoredKey);
        }
        Ok(Some(key))
    }

    fn sign(&self, builder: EventBuilder) -> Result<Event, KeyError> {
        builder.finalize(&self.keys).map_err(|_| KeyError::Sign)
    }
}

/// The public key only.
impl fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GroupKey").field(&self.npub()).finish()
    }
}

// ---------------------------------------------------------------------------
// State: what the group key signs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Text,
    Voice,
}

impl ChannelKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Voice => "voice",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "voice" => Some(Self::Voice),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    /// Seconds a member must wait between messages here; 0 for none.
    pub slowmode: u32,
}

/// One role's adjustment in one channel, as permission bits (`hit-group`
/// gives them meaning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub channel: String,
    pub role: String,
    pub allow: u64,
    pub deny: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleDef {
    pub id: String,
    pub name: String,
    pub perms: u64,
    /// Higher is higher rank. The default role sits at 0.
    pub position: i64,
}

/// Everything the group key states about a group at one `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub seq: u64,
    pub name: String,
    /// Anyone may find, read and join it (M13). Private by default.
    pub public: bool,
    /// Seconds a member who joined by themselves waits before posting
    /// (M13); 0 is none. A public group never waits less than
    /// [`PUBLIC_NEWCOMER_FLOOR`], whatever this says.
    pub newcomer_wait: u32,
    pub channels: Vec<Channel>,
    pub overrides: Vec<Override>,
    pub roles: Vec<RoleDef>,
    /// The person the group key names as owner. Counts only together with
    /// that person's own `9007` (see `hit-group`).
    pub owner: Option<PublicKey>,
    /// Every member, with the roles they hold beyond the default one.
    pub members: BTreeMap<PublicKey, BTreeSet<String>>,
    /// People who may not join until unbanned (M5).
    pub bans: BTreeSet<PublicKey>,
    /// Members muted until a time (M5).
    pub mutes: BTreeMap<PublicKey, Timestamp>,
}

impl Snapshot {
    /// The four state events, signed by the group key, all at `at` and
    /// `self.seq`. Publish all four: a peer folds each kind at its newest.
    pub fn sign(&self, key: &GroupKey, at: Timestamp) -> Result<[Event; 4], KeyError> {
        let g = key.public_key().to_hex();
        let base = |kind: Kind| {
            EventBuilder::new(kind, "")
                .custom_created_at(at)
                .tag(Tag::identifier(g.clone()))
                .tag(tag("seq", [self.seq.to_string()]))
        };

        let mut meta = base(GROUP_METADATA).tag(tag("name", [self.name.clone()]));
        for t in visibility_tags(self.public) {
            meta = meta.tag(t);
        }
        if self.newcomer_wait > 0 {
            meta = meta.tag(tag("newcomer_wait", [self.newcomer_wait.to_string()]));
        }
        for c in &self.channels {
            meta = meta.tag(channel_tag(c));
        }
        for o in &self.overrides {
            meta = meta.tag(tag(
                "override",
                [
                    o.channel.clone(),
                    o.role.clone(),
                    o.allow.to_string(),
                    o.deny.to_string(),
                ],
            ));
        }

        let mut admins = base(GROUP_ADMINS);
        if let Some(owner) = &self.owner {
            admins = admins.tag(tag("p", [owner.to_hex(), OWNER_MARK.to_owned()]));
        }
        for (p, roles) in &self.members {
            if roles.is_empty() {
                continue;
            }
            let mut values = vec![p.to_hex()];
            values.extend(roles.iter().cloned());
            admins = admins.tag(tag("p", values));
        }

        let mut members = base(GROUP_MEMBERS);
        for p in self.members.keys() {
            members = members.tag(tag("p", [p.to_hex()]));
        }
        for p in &self.bans {
            members = members.tag(tag("ban", [p.to_hex()]));
        }
        for (p, until) in &self.mutes {
            members = members.tag(tag("mute", [p.to_hex(), until.as_secs().to_string()]));
        }

        let mut roles = base(GROUP_ROLES);
        for r in &self.roles {
            roles = roles.tag(tag(
                "role",
                [
                    r.id.clone(),
                    r.name.clone(),
                    r.perms.to_string(),
                    r.position.to_string(),
                ],
            ));
        }

        Ok([
            key.sign(meta)?,
            key.sign(admins)?,
            key.sign(members)?,
            key.sign(roles)?,
        ])
    }
}

/// One of the four state events, parsed. `None` from [`parse_state`] means
/// "not a valid state event": wrong kind, not signed by the group it names,
/// no `seq`, or a known tag malformed. Unknown tags are ignored, so a newer
/// build can add some.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatePiece {
    Metadata {
        name: String,
        public: bool,
        newcomer_wait: u32,
        channels: Vec<Channel>,
        overrides: Vec<Override>,
    },
    Admins {
        owner: Option<PublicKey>,
        assignments: BTreeMap<PublicKey, BTreeSet<String>>,
    },
    Members {
        members: BTreeSet<PublicKey>,
        bans: BTreeSet<PublicKey>,
        mutes: BTreeMap<PublicKey, Timestamp>,
    },
    Roles(Vec<RoleDef>),
}

/// Parses a state event: `(group, seq, piece)`.
pub fn parse_state(event: &Event) -> Option<(PublicKey, u64, StatePiece)> {
    let group = group_of(event)?;
    if !STATE_KINDS.contains(&event.kind) || event.pubkey != group {
        return None;
    }
    let seq = seq_of(event)?;
    let rows = || event.tags.iter().map(|t| t.as_slice());
    let piece = if event.kind == GROUP_METADATA {
        let mut name = String::new();
        let (mut public, mut private) = (false, false);
        let mut newcomer_wait = 0;
        let (mut channels, mut overrides) = (Vec::new(), Vec::new());
        for row in rows() {
            match row {
                [n, v, ..] if n == "name" => name = v.clone(),
                [n, ..] if n == "public" => public = true,
                [n, ..] if n == "private" => private = true,
                [n, v, ..] if n == "newcomer_wait" => newcomer_wait = v.parse().ok()?,
                [n, rest @ ..] if n == "channel" => channels.push(parse_channel(rest)?),
                [n, channel, role, allow, deny, ..] if n == "override" => {
                    overrides.push(Override {
                        channel: channel.clone(),
                        role: role.clone(),
                        allow: allow.parse().ok()?,
                        deny: deny.parse().ok()?,
                    })
                }
                [n, ..] if n == "override" => return None,
                _ => {}
            }
        }
        StatePiece::Metadata {
            name,
            // Both, or neither (every group before M13): private.
            public: public && !private,
            newcomer_wait,
            channels,
            overrides,
        }
    } else if event.kind == GROUP_ADMINS {
        let mut owner = None;
        let mut assignments = BTreeMap::new();
        for row in rows() {
            if let [n, p, rest @ ..] = row
                && n == "p"
            {
                let p = PublicKey::from_hex(p).ok()?;
                if rest.iter().any(|r| r == OWNER_MARK) {
                    owner = Some(p);
                    continue;
                }
                assignments.insert(p, rest.iter().cloned().collect::<BTreeSet<_>>());
            }
        }
        StatePiece::Admins { owner, assignments }
    } else if event.kind == GROUP_MEMBERS {
        let (mut members, mut bans, mut mutes) =
            (BTreeSet::new(), BTreeSet::new(), BTreeMap::new());
        for row in rows() {
            match row {
                [n, p, ..] if n == "p" => {
                    members.insert(PublicKey::from_hex(p).ok()?);
                }
                [n, p, ..] if n == "ban" => {
                    bans.insert(PublicKey::from_hex(p).ok()?);
                }
                [n, p, until, ..] if n == "mute" => {
                    mutes.insert(
                        PublicKey::from_hex(p).ok()?,
                        Timestamp::from(until.parse::<u64>().ok()?),
                    );
                }
                _ => {}
            }
        }
        StatePiece::Members {
            members,
            bans,
            mutes,
        }
    } else {
        let mut roles = Vec::new();
        for row in rows() {
            match row {
                [n, id, rname, perms, position, ..] if n == "role" => roles.push(RoleDef {
                    id: id.clone(),
                    name: rname.clone(),
                    perms: perms.parse().ok()?,
                    position: position.parse().ok()?,
                }),
                [n, ..] if n == "role" => return None,
                _ => {}
            }
        }
        StatePiece::Roles(roles)
    };
    Some((group, seq, piece))
}

// ---------------------------------------------------------------------------
// Actions and messages: what people sign

/// An action, parsed. Its author, time and id are the event's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Put `target` in the group with exactly `roles` beyond the default.
    Put {
        target: PublicKey,
        roles: BTreeSet<String>,
    },
    /// Remove `target`; with `ban`, also keep them out.
    Remove {
        target: PublicKey,
        ban: bool,
    },
    /// Lift a ban. Adds nobody.
    Unban {
        target: PublicKey,
    },
    /// Mute `target` until `until`, or lift a mute (`None`).
    Mute {
        target: PublicKey,
        until: Option<Timestamp>,
    },
    /// Change the group: every edit, or none.
    Edit(Vec<Edit>),
    Invite {
        code: String,
        expires: Option<Timestamp>,
    },
    /// `None`: no invite, honoured only by a public group (M13).
    Join {
        code: Option<String>,
    },
    Leave,
}

/// One change a `9002` makes. Tags, in order: `["name", N]`;
/// `["public"]` or `["private"]` (M13, the owner only);
/// `["newcomer_wait", seconds]` (M13);
/// `["channel", id, name, kind, slowmode]` (create or replace);
/// `["channel-delete", id]`; `["override", channel, role, allow, deny]`
/// (both 0 removes it); `["role", id, name, perms, position]` (create or
/// replace); `["role-delete", id]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Name(String),
    /// Public (`true`) or private.
    Public(bool),
    /// Seconds a newcomer waits before posting.
    NewcomerWait(u32),
    Channel(Channel),
    ChannelDelete(String),
    Override(Override),
    Role(RoleDef),
    RoleDelete(String),
}

impl Edit {
    fn to_tag(&self) -> Tag {
        match self {
            Self::Name(n) => tag("name", [n.clone()]),
            Self::Public(true) => tag("public", Vec::<String>::new()),
            Self::Public(false) => tag("private", Vec::<String>::new()),
            Self::NewcomerWait(s) => tag("newcomer_wait", [s.to_string()]),
            Self::Channel(c) => channel_tag(c),
            Self::ChannelDelete(id) => tag("channel-delete", [id.clone()]),
            Self::Override(o) => tag(
                "override",
                [
                    o.channel.clone(),
                    o.role.clone(),
                    o.allow.to_string(),
                    o.deny.to_string(),
                ],
            ),
            Self::Role(r) => tag(
                "role",
                [
                    r.id.clone(),
                    r.name.clone(),
                    r.perms.to_string(),
                    r.position.to_string(),
                ],
            ),
            Self::RoleDelete(id) => tag("role-delete", [id.clone()]),
        }
    }

    fn parse(row: &[String]) -> Option<Option<Self>> {
        Some(Some(match row {
            [n, v, ..] if n == "name" => Self::Name(v.clone()),
            [n, ..] if n == "public" => Self::Public(true),
            [n, ..] if n == "private" => Self::Public(false),
            [n, v, ..] if n == "newcomer_wait" => Self::NewcomerWait(v.parse().ok()?),
            [n, rest @ ..] if n == "channel" => Self::Channel(parse_channel(rest)?),
            [n, id, ..] if n == "channel-delete" => Self::ChannelDelete(id.clone()),
            [n, channel, role, allow, deny, ..] if n == "override" => Self::Override(Override {
                channel: channel.clone(),
                role: role.clone(),
                allow: allow.parse().ok()?,
                deny: deny.parse().ok()?,
            }),
            [n, id, name, perms, position, ..] if n == "role" => Self::Role(RoleDef {
                id: id.clone(),
                name: name.clone(),
                perms: perms.parse().ok()?,
                position: position.parse().ok()?,
            }),
            [n, id, ..] if n == "role-delete" => Self::RoleDelete(id.clone()),
            [n, ..]
                if [
                    "name",
                    "newcomer_wait",
                    "channel",
                    "channel-delete",
                    "override",
                    "role",
                    "role-delete",
                ]
                .contains(&n.as_str()) =>
            {
                return None;
            }
            _ => return Some(None),
        }))
    }
}

/// NIP-29's words on a `39000`: `public` and `open`, or `private` and
/// `closed`. Always written, both ways — a NIP-29 client reads a group
/// with neither as public, and every group before M13 has neither, so
/// "private" is read only from the absence of `public` (see `parse_state`).
pub fn visibility_tags(public: bool) -> [Tag; 2] {
    let none = Vec::<String>::new;
    if public {
        [tag("public", none()), tag("open", none())]
    } else {
        [tag("private", none()), tag("closed", none())]
    }
}

fn channel_tag(c: &Channel) -> Tag {
    tag(
        "channel",
        [
            c.id.clone(),
            c.name.clone(),
            c.kind.as_str().to_owned(),
            c.slowmode.to_string(),
        ],
    )
}

/// `[id, name, kind, slowmode?]` — slowmode absent (an M3 event) is 0.
fn parse_channel(rest: &[String]) -> Option<Channel> {
    match rest {
        [id, name, kind, more @ ..] => Some(Channel {
            id: id.clone(),
            name: name.clone(),
            kind: ChannelKind::parse(kind)?,
            slowmode: match more.first() {
                Some(s) => s.parse().ok()?,
                None => 0,
            },
        }),
        _ => None,
    }
}

/// Parses an action: `(group, action)`. `None` if malformed.
pub fn parse_action(event: &Event) -> Option<(PublicKey, Action)> {
    let group = group_of(event)?;
    let target = || PublicKey::from_hex(first_value(event, "p")?).ok();
    let action = if event.kind == PUT_USER {
        let roles = event
            .tags
            .iter()
            .find_map(|t| match t.as_slice() {
                [n, _, rest @ ..] if n == "p" => Some(rest.iter().cloned().collect()),
                _ => None,
            })
            .unwrap_or_default();
        Action::Put {
            target: target()?,
            roles,
        }
    } else if event.kind == REMOVE_USER {
        let flag = |f: &str| {
            event
                .tags
                .iter()
                .any(|t| t.as_slice().first().is_some_and(|n| n == f))
        };
        if flag("unban") {
            Action::Unban { target: target()? }
        } else {
            Action::Remove {
                target: target()?,
                ban: flag("ban"),
            }
        }
    } else if event.kind == MUTE_USER {
        let until = event.tags.iter().find_map(|t| match t.as_slice() {
            [n, _, until, ..] if n == "p" => until.parse::<u64>().ok(),
            _ => None,
        })?;
        Action::Mute {
            target: target()?,
            until: (until > 0).then(|| Timestamp::from(until)),
        }
    } else if event.kind == EDIT_METADATA {
        let mut edits = Vec::new();
        for t in event.tags.iter() {
            if let Some(e) = Edit::parse(t.as_slice())? {
                edits.push(e);
            }
        }
        if edits.is_empty() {
            return None;
        }
        Action::Edit(edits)
    } else if event.kind == CREATE_INVITE {
        Action::Invite {
            code: first_value(event, "code")?.to_owned(),
            expires: match first_value(event, "expiration") {
                Some(t) => Some(Timestamp::from(t.parse::<u64>().ok()?)),
                None => None,
            },
        }
    } else if event.kind == JOIN {
        Action::Join {
            code: first_value(event, "code").map(str::to_owned),
        }
    } else if event.kind == LEAVE {
        Action::Leave
    } else {
        return None;
    };
    Some((group, action))
}

/// A file attached to a message (M8): NIP-92's `imeta`, one per file.
///
/// `["imeta", "url hit-blob:<blake3>", "m <type>", "x <sha256>",
/// "size <bytes>", "name <filename>"]` — the BLAKE3 hash is what iroh
/// fetches and verifies by; the SHA-256 is NIP-94's `x`, so an anchor could
/// mirror to a Blossom server later. No provider list: whoever can read the
/// message may be asked for the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// BLAKE3, 64 lowercase hex.
    pub hash: String,
    /// SHA-256, 64 lowercase hex.
    pub sha256: String,
    pub size: u64,
    pub mime: String,
    pub name: String,
}

/// The largest file, in bytes: 50 MB, HIT_ENTER_2's limit.
pub const MAX_ATTACHMENT_BYTES: u64 = 50 * 1000 * 1000;
/// Most files one message may carry.
pub const MAX_ATTACHMENTS: usize = 10;
/// Longest file name and media type, in characters.
pub const MAX_ATTACHMENT_NAME: usize = 255;
pub const MAX_ATTACHMENT_MIME: usize = 128;
/// The URL scheme of an `imeta` url: the content, by its BLAKE3 hash.
pub const BLOB_URL: &str = "hit-blob:";

impl Attachment {
    fn to_tag(&self) -> Tag {
        tag(
            "imeta",
            [
                format!("url {BLOB_URL}{}", self.hash),
                format!("m {}", self.mime),
                format!("x {}", self.sha256),
                format!("size {}", self.size),
                format!("name {}", self.name),
            ],
        )
    }

    /// Within every limit, with both hashes well-formed.
    pub fn is_valid(&self) -> bool {
        let hex64 = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        hex64(&self.hash)
            && hex64(&self.sha256)
            && self.size > 0
            && self.size <= MAX_ATTACHMENT_BYTES
            && !self.name.is_empty()
            && self.name.chars().count() <= MAX_ATTACHMENT_NAME
            && !self.name.contains(['/', '\\', '\0'])
            && self.mime.chars().count() <= MAX_ATTACHMENT_MIME
    }
}

/// The files a message carries. `Err(())` if any `imeta` is malformed or out
/// of bounds, or there are too many — the message is then not valid at all.
#[allow(clippy::result_unit_err)]
pub fn attachments_of(event: &Event) -> Result<Vec<Attachment>, ()> {
    let mut out = Vec::new();
    for t in event.tags.iter() {
        let [n, fields @ ..] = t.as_slice() else {
            continue;
        };
        if n != "imeta" {
            continue;
        }
        let field = |k: &str| {
            fields
                .iter()
                .find_map(|f| f.strip_prefix(k).and_then(|v| v.strip_prefix(' ')))
        };
        let a = Attachment {
            hash: field("url")
                .and_then(|u| u.strip_prefix(BLOB_URL))
                .ok_or(())?
                .to_owned(),
            sha256: field("x").ok_or(())?.to_owned(),
            size: field("size").and_then(|v| v.parse().ok()).ok_or(())?,
            mime: field("m").unwrap_or("application/octet-stream").to_owned(),
            name: field("name").unwrap_or("file").to_owned(),
        };
        if !a.is_valid() {
            return Err(());
        }
        out.push(a);
    }
    if out.len() > MAX_ATTACHMENTS {
        return Err(());
    }
    Ok(out)
}

/// A chat message in `channel` of `group`, with files (M8).
pub fn message_with(
    group: &PublicKey,
    channel: &str,
    text: &str,
    attachments: &[Attachment],
) -> EventBuilder {
    let mut b = message(group, channel, text);
    for a in attachments {
        b = b.tag(a.to_tag());
    }
    b
}

/// A chat message in `channel` of `group`.
pub fn message(group: &PublicKey, channel: &str, text: &str) -> EventBuilder {
    EventBuilder::new(CHAT_MESSAGE, text)
        .tag(h(group))
        .tag(tag("channel", [channel]))
}

/// A reaction to `target` (by `author`), in `channel` of `group`.
pub fn reaction(
    group: &PublicKey,
    channel: &str,
    target: &EventId,
    author: &PublicKey,
    emoji: &str,
) -> EventBuilder {
    EventBuilder::new(REACTION, emoji)
        .tag(h(group))
        .tag(tag("channel", [channel]))
        .tag(tag("e", [target.to_hex()]))
        .tag(tag("p", [author.to_hex()]))
        .tag(tag("k", [CHAT_MESSAGE.as_u16().to_string()]))
}

/// Withdraws `targets` — the signer's own messages or reactions in
/// `channel` (NIP-09).
pub fn deletion(group: &PublicKey, channel: &str, targets: &[EventId]) -> EventBuilder {
    let mut b = EventBuilder::new(DELETION, "")
        .tag(h(group))
        .tag(tag("channel", [channel]));
    for t in targets {
        b = b.tag(tag("e", [t.to_hex()]));
    }
    b
}

/// "I am in voice channel `channel` of `group`", or "I left" (`inside`
/// false).
pub fn voice_presence(group: &PublicKey, channel: &str, inside: bool) -> EventBuilder {
    EventBuilder::new(VOICE_PRESENCE, "")
        .tag(h(group))
        .tag(tag("channel", [channel]))
        .tag(tag("state", [if inside { "in" } else { "out" }]))
}

/// Whether a voice presence says "in" (anything else is "out").
pub fn voice_inside(event: &Event) -> bool {
    first_value(event, "state") == Some("in")
}

/// "I am here" for `group`'s gossip.
pub fn presence(group: &PublicKey) -> EventBuilder {
    EventBuilder::new(PRESENCE, "").tag(h(group))
}

/// The owner's `9007`.
pub fn create_group(group: &PublicKey) -> EventBuilder {
    EventBuilder::new(CREATE_GROUP, "").tag(h(group))
}

pub fn put_user(group: &PublicKey, target: &PublicKey, roles: &[&str]) -> EventBuilder {
    let mut values = vec![target.to_hex()];
    values.extend(roles.iter().map(|r| (*r).to_owned()));
    EventBuilder::new(PUT_USER, "")
        .tag(h(group))
        .tag(tag("p", values))
}

pub fn remove_user(group: &PublicKey, target: &PublicKey) -> EventBuilder {
    EventBuilder::new(REMOVE_USER, "")
        .tag(h(group))
        .tag(tag("p", [target.to_hex()]))
}

/// Remove `target` and keep them out.
pub fn ban_user(group: &PublicKey, target: &PublicKey) -> EventBuilder {
    remove_user(group, target).tag(tag("ban", Vec::<String>::new()))
}

/// Lift `target`'s ban.
pub fn unban_user(group: &PublicKey, target: &PublicKey) -> EventBuilder {
    remove_user(group, target).tag(tag("unban", Vec::<String>::new()))
}

/// Mute `target` until `until`; `None` lifts a mute.
pub fn mute_user(group: &PublicKey, target: &PublicKey, until: Option<Timestamp>) -> EventBuilder {
    let until = until.map_or(0, |t| t.as_secs());
    EventBuilder::new(MUTE_USER, "")
        .tag(h(group))
        .tag(tag("p", [target.to_hex(), until.to_string()]))
}

/// Change the group: every edit in `edits`, or none.
pub fn edit_metadata(group: &PublicKey, edits: &[Edit]) -> EventBuilder {
    let mut b = EventBuilder::new(EDIT_METADATA, "").tag(h(group));
    for e in edits {
        b = b.tag(e.to_tag());
    }
    b
}

/// A moderator removing `targets` — anyone's messages or reactions — in
/// `channel`.
pub fn delete_event(group: &PublicKey, channel: &str, targets: &[EventId]) -> EventBuilder {
    let mut b = EventBuilder::new(DELETE_EVENT, "")
        .tag(h(group))
        .tag(tag("channel", [channel]));
    for t in targets {
        b = b.tag(tag("e", [t.to_hex()]));
    }
    b
}

pub fn create_invite(group: &PublicKey, code: &str, expires: Option<Timestamp>) -> EventBuilder {
    let mut b = EventBuilder::new(CREATE_INVITE, "")
        .tag(h(group))
        .tag(tag("code", [code]));
    if let Some(t) = expires {
        b = b.tag(tag("expiration", [t.as_secs().to_string()]));
    }
    b
}

pub fn join(group: &PublicKey, code: &str) -> EventBuilder {
    EventBuilder::new(JOIN, "")
        .tag(h(group))
        .tag(tag("code", [code]))
}

/// A join without an invite: to a public group (M13).
pub fn join_public(group: &PublicKey) -> EventBuilder {
    EventBuilder::new(JOIN, "").tag(h(group))
}

pub fn leave(group: &PublicKey) -> EventBuilder {
    EventBuilder::new(LEAVE, "").tag(h(group))
}

/// A fresh invite code: 128 bits from the OS, hex. Long enough that guessing
/// one is not an attack; it is not a secret from members, who see the `9009`.
pub fn new_invite_code() -> Result<String, KeyError> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| KeyError::Sign)?;
    Ok(hex::encode(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryStore, Person};

    fn snapshot(owner: &Person) -> Snapshot {
        Snapshot {
            seq: 3,
            name: "Friends".into(),
            public: true,
            newcomer_wait: 600,
            channels: vec![
                Channel {
                    id: "general".into(),
                    name: "general".into(),
                    kind: ChannelKind::Text,
                    slowmode: 30,
                },
                Channel {
                    id: "mods".into(),
                    name: "mods only".into(),
                    kind: ChannelKind::Text,
                    slowmode: 0,
                },
            ],
            overrides: vec![Override {
                channel: "mods".into(),
                role: DEFAULT_ROLE.into(),
                allow: 0,
                deny: 3,
            }],
            roles: vec![
                RoleDef {
                    id: DEFAULT_ROLE.into(),
                    name: "member".into(),
                    perms: 19,
                    position: 0,
                },
                RoleDef {
                    id: "mod".into(),
                    name: "Moderator".into(),
                    perms: 511,
                    position: 10,
                },
            ],
            owner: Some(owner.public_key()),
            members: BTreeMap::from([
                (owner.public_key(), BTreeSet::new()),
                (
                    Person::generate().public_key(),
                    BTreeSet::from(["mod".to_owned()]),
                ),
            ]),
            bans: BTreeSet::from([Person::generate().public_key()]),
            mutes: BTreeMap::from([(owner.public_key(), Timestamp::from(5_000))]),
        }
    }

    #[test]
    fn a_snapshot_round_trips_through_its_four_events() {
        let key = GroupKey::generate();
        let owner = Person::generate();
        let snap = snapshot(&owner);
        let events = snap.sign(&key, Timestamp::from(1_000)).unwrap();
        let mut back = Snapshot {
            seq: 3,
            ..Default::default()
        };
        for ev in &events {
            assert!(ev.verify().is_ok());
            assert_eq!(group_of(ev), Some(key.public_key()));
            let (g, seq, piece) = parse_state(ev).unwrap();
            assert_eq!((g, seq), (key.public_key(), 3));
            match piece {
                StatePiece::Metadata {
                    name,
                    public,
                    newcomer_wait,
                    channels,
                    overrides,
                } => {
                    back.name = name;
                    back.public = public;
                    back.newcomer_wait = newcomer_wait;
                    back.channels = channels;
                    back.overrides = overrides;
                }
                StatePiece::Admins { owner, assignments } => {
                    back.owner = owner;
                    for (p, r) in assignments {
                        back.members.insert(p, r);
                    }
                }
                StatePiece::Members {
                    members,
                    bans,
                    mutes,
                } => {
                    for p in members {
                        back.members.entry(p).or_default();
                    }
                    back.bans = bans;
                    back.mutes = mutes;
                }
                StatePiece::Roles(r) => back.roles = r,
            }
        }
        assert_eq!(back, snap);
    }

    #[test]
    fn the_moderation_actions_round_trip() {
        let p = Person::generate();
        let g = GroupKey::generate().public_key();
        let t = Person::generate().public_key();
        let parse = |b: EventBuilder| parse_action(&p.sign(b).unwrap()).unwrap().1;
        assert_eq!(
            parse(remove_user(&g, &t)),
            Action::Remove {
                target: t,
                ban: false
            }
        );
        assert_eq!(
            parse(ban_user(&g, &t)),
            Action::Remove {
                target: t,
                ban: true
            }
        );
        assert_eq!(parse(unban_user(&g, &t)), Action::Unban { target: t });
        let until = Timestamp::from(9_000);
        assert_eq!(
            parse(mute_user(&g, &t, Some(until))),
            Action::Mute {
                target: t,
                until: Some(until)
            }
        );
        assert_eq!(
            parse(mute_user(&g, &t, None)),
            Action::Mute {
                target: t,
                until: None
            }
        );
        let edits = vec![
            Edit::Name("New".into()),
            Edit::Public(true),
            Edit::Public(false),
            Edit::NewcomerWait(900),
            Edit::Channel(Channel {
                id: "slow".into(),
                name: "slow".into(),
                kind: ChannelKind::Text,
                slowmode: 60,
            }),
            Edit::ChannelDelete("old".into()),
            Edit::Override(Override {
                channel: "slow".into(),
                role: DEFAULT_ROLE.into(),
                allow: 0,
                deny: 2,
            }),
            Edit::Role(RoleDef {
                id: "helper".into(),
                name: "Helper".into(),
                perms: 4,
                position: 5,
            }),
            Edit::RoleDelete("gone".into()),
        ];
        assert_eq!(parse(edit_metadata(&g, &edits)), Action::Edit(edits));
        // No edits, or a known edit tag malformed: not an action at all.
        assert!(parse_action(&p.sign(edit_metadata(&g, &[])).unwrap()).is_none());
        let bad = edit_metadata(&g, &[]).tag(tag("role", ["x", "X", "not a number", "1"]));
        assert!(parse_action(&p.sign(bad).unwrap()).is_none());
    }

    #[test]
    fn attachments_round_trip_and_a_bad_one_spoils_the_message() {
        let p = Person::generate();
        let g = GroupKey::generate().public_key();
        let a = Attachment {
            hash: "a".repeat(64),
            sha256: "b".repeat(64),
            size: 1234,
            mime: "image/png".into(),
            name: "cat picture.png".into(),
        };
        let ev = p
            .sign(message_with(
                &g,
                "general",
                "look",
                std::slice::from_ref(&a),
            ))
            .unwrap();
        assert_eq!(attachments_of(&ev), Ok(vec![a.clone()]));
        assert_eq!(
            attachments_of(&p.sign(message(&g, "general", "x")).unwrap()),
            Ok(vec![])
        );
        for bad in [
            Attachment {
                hash: "A".repeat(64),
                ..a.clone()
            },
            Attachment {
                size: 0,
                ..a.clone()
            },
            Attachment {
                size: MAX_ATTACHMENT_BYTES + 1,
                ..a.clone()
            },
            Attachment {
                name: "../etc/passwd".into(),
                ..a.clone()
            },
        ] {
            let ev = p.sign(message_with(&g, "general", "", &[bad])).unwrap();
            assert!(attachments_of(&ev).is_err());
        }
        let many = vec![a; MAX_ATTACHMENTS + 1];
        let ev = p.sign(message_with(&g, "general", "", &many)).unwrap();
        assert!(attachments_of(&ev).is_err());
    }

    /// Every group before M13 wrote neither `public` nor `private`: it
    /// stays private. NIP-29 reads such a group as public, which is why
    /// `sign` now always writes one pair or the other.
    #[test]
    fn a_group_is_public_only_when_its_metadata_says_so_alone() {
        let key = GroupKey::generate();
        let public_of = |rows: &[&str]| {
            let mut b = EventBuilder::new(GROUP_METADATA, "")
                .tag(Tag::identifier(key.public_key().to_hex()))
                .tag(tag("seq", ["0"]))
                .tag(tag("name", ["G"]));
            for r in rows {
                b = b.tag(tag(r, Vec::<String>::new()));
            }
            match parse_state(&key.sign(b).unwrap()) {
                Some((_, _, StatePiece::Metadata { public, .. })) => public,
                other => panic!("not metadata: {other:?}"),
            }
        };
        assert!(!public_of(&[]), "an M3 group");
        assert!(public_of(&["public", "open"]));
        assert!(!public_of(&["private", "closed"]));
        assert!(!public_of(&["public", "private"]), "contradiction: private");

        let rows = |public| {
            let snap = Snapshot {
                public,
                ..Default::default()
            };
            let meta = &snap.sign(&key, Timestamp::from(1)).unwrap()[0];
            meta.tags
                .iter()
                .filter_map(|t| match t.as_slice() {
                    [n] => Some(n.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(true), ["public", "open"]);
        assert_eq!(rows(false), ["private", "closed"]);
    }

    #[test]
    fn an_m3_channel_tag_without_slowmode_still_parses() {
        let c = parse_channel(&["general".into(), "general".into(), "text".into()]).unwrap();
        assert_eq!(c.slowmode, 0);
    }

    #[test]
    fn state_signed_by_anyone_but_the_group_is_not_state() {
        let key = GroupKey::generate();
        let forger = GroupKey::generate();
        // The forger's events name the victim group in `d`.
        let ev = EventBuilder::new(GROUP_MEMBERS, "")
            .tag(Tag::identifier(key.public_key().to_hex()))
            .tag(tag("seq", ["99"]))
            .finalize(&forger.keys)
            .unwrap();
        assert_eq!(group_of(&ev), Some(key.public_key()));
        assert_eq!(parse_state(&ev), None);
    }

    #[test]
    fn actions_and_messages_parse() {
        let g = GroupKey::generate().public_key();
        let p = Person::generate();
        let target = Person::generate().public_key();
        let ev = p.sign(put_user(&g, &target, &["mod"])).unwrap();
        assert_eq!(
            parse_action(&ev),
            Some((
                g,
                Action::Put {
                    target,
                    roles: BTreeSet::from(["mod".to_owned()])
                }
            ))
        );
        let ev = p
            .sign(create_invite(&g, "abc", Some(Timestamp::from(5))))
            .unwrap();
        assert_eq!(
            parse_action(&ev),
            Some((
                g,
                Action::Invite {
                    code: "abc".into(),
                    expires: Some(Timestamp::from(5))
                }
            ))
        );
        let ev = p.sign(join(&g, "abc")).unwrap();
        assert_eq!(
            parse_action(&ev),
            Some((g, Action::Join { code: Some("abc".into()) }))
        );
        // M13: a join without an invite, for a public group.
        let ev = p.sign(join_public(&g)).unwrap();
        assert_eq!(parse_action(&ev), Some((g, Action::Join { code: None })));
        let ev = p.sign(message(&g, "general", "hi")).unwrap();
        assert_eq!(group_of(&ev), Some(g));
        assert_eq!(channel_of(&ev), Some("general"));
        assert_eq!(parse_action(&ev), None);
    }

    #[test]
    fn the_group_key_survives_the_store_and_debug_shows_only_the_npub() {
        let store = MemoryStore::default();
        let key = GroupKey::generate();
        key.save(&store).unwrap();
        let back = GroupKey::load(&store, &key.npub()).unwrap().unwrap();
        assert_eq!(back.public_key(), key.public_key());
        let dbg = format!("{key:?}");
        assert!(dbg.contains(&key.npub()));
        assert!(!dbg.contains(&key.keys.secret_key().to_secret_hex()));
    }

    #[test]
    fn invite_codes_are_fresh() {
        let (a, b) = (new_invite_code().unwrap(), new_invite_code().unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
