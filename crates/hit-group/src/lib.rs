//! Group state and permissions (M3, `docs/PLAN.md`; `docs/DESIGN.md` §6).
//!
//! **Every authorization decision goes through this crate**, as a pure
//! function over state that every peer runs on every event. No I/O and no
//! clock — the caller passes `now` — so the adversarial tests need no
//! network and no database.
//!
//! - [`perm`]: the permission bits (moved from HIT_ENTER_2).
//! - [`state`]: one group, folded from signed events — the snapshot, the
//!   actions on top, and every permission answer.
//! - [`Groups`]: the groups a node carries, and the questions a node asks
//!   about an event or a peer: may it be stored, may this person read it,
//!   which gossip carries it.

pub mod perm;
pub mod state;

use std::collections::{BTreeMap, BTreeSet};

use hit_event::{
    DEVICE_BINDING, Verified,
    group::{
        ACTION_KINDS, CHANNEL_KINDS, CHAT_MESSAGE, CREATE_GROUP, CREATE_INVITE, DELETE_EVENT,
        DELETION, PRESENCE, REACTION, STATE_KINDS, channel_of, group_of, parse_state,
    },
};
use nostr::prelude::{Event, Kind, PublicKey, Timestamp};

pub use perm::Permissions;
pub use state::{FOLD_KINDS, GroupState, Rank, Silence};

/// A person's profile (NIP-01 kind 0): a display name, for now.
pub const PROFILE: Kind = Kind::Metadata;

/// Every kind a node carries. Anything else is refused, not stored: a peer
/// stores only what it has a reason to (`DESIGN.md` §4).
pub const CARRIED_KINDS: [Kind; 23] = [
    DEVICE_BINDING,
    PROFILE,
    hit_event::dm::INBOX_LIST,
    hit_event::dm::GIFT_WRAP,
    hit_event::group::VOICE_PRESENCE,
    CHAT_MESSAGE,
    REACTION,
    hit_event::zap::ZAP_CARRIER,
    DELETION,
    PRESENCE,
    CREATE_GROUP,
    ACTION_KINDS[0],
    ACTION_KINDS[1],
    ACTION_KINDS[2],
    ACTION_KINDS[3],
    ACTION_KINDS[4],
    ACTION_KINDS[5],
    ACTION_KINDS[6],
    DELETE_EVENT,
    STATE_KINDS[0],
    STATE_KINDS[1],
    STATE_KINDS[2],
    STATE_KINDS[3],
];

/// How far ahead of our clock an event may be dated: the same skew a device
/// binding allows.
pub const MAX_CLOCK_SKEW: u64 = 600;

/// A presence older than this is refused: it would say someone is here who
/// may have left, and an old one replayed is the cheapest way to lie.
pub const PRESENCE_MAX_AGE: u64 = 120;

/// The largest gift wrap kept, in bytes of content: a DM of the longest
/// allowed text, encrypted twice, with room to spare. A wrap is from a
/// throwaway key, so size is the one thing a recipient's anchor can refuse
/// it on before knowing anything else.
pub const MAX_WRAP_BYTES: usize = 64 * 1024;

/// What a deletion may withdraw (NIP-09, the author's own only): messages
/// and reactions. Never a group action — those are the fold's, and a
/// deleted join would rewrite who is a member.
pub const DELETABLE_KINDS: [Kind; 2] = [CHAT_MESSAGE, REACTION];

/// Why a peer or an event was refused. The words are safe to send to the
/// peer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("not a member")]
    NotAMember,
    #[error("already a member")]
    AlreadyAMember,
    #[error("this node does not carry kind {0}")]
    KindNotCarried(u16),
    #[error("this node does not carry that group")]
    GroupNotCarried,
    #[error("this node has no state for that group yet")]
    NoState,
    #[error("group state not signed by the group key")]
    NotSignedByGroup,
    #[error("malformed group event")]
    Malformed,
    #[error("no such channel")]
    NoSuchChannel,
    #[error("lacks permission: {0}")]
    Lacks(Permissions),
    #[error("nobody may act on the owner")]
    TheOwner,
    #[error("only the owner the group key names may sign its creation")]
    NotTheOwner,
    #[error("does not outrank the target")]
    Outranked,
    #[error("cannot grant a permission it does not hold")]
    CannotGrant,
    #[error("unknown role")]
    UnknownRole,
    #[error("no valid invite with that code")]
    NoSuchInvite,
    #[error("folded into the group's snapshot")]
    Folded,
    #[error("not valid against the group's state")]
    NotApplied,
    #[error("dated too far in the future")]
    FromTheFuture,
    #[error("too old to mean anything now")]
    Stale,
    #[error("banned from this group")]
    Banned,
    #[error("not banned")]
    NotBanned,
    #[error("muted")]
    Muted,
    #[error("not muted")]
    NotMuted,
    #[error("an edit this group cannot take")]
    BadEdit,
    #[error("slowmode: wait {0} s between messages here")]
    TooSoon(u32),
    #[error("new here: posting opens {0} s after joining")]
    Newcomer(u32),
    #[error("not a DM for anyone this node keeps them for")]
    NotOurs,
    #[error("too large")]
    TooLarge,
}

/// What [`Groups::judge`] decided about an event that may be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    /// Store it.
    Store,
    /// An action of this group: store it only if folding it together with
    /// everything already held applies it ([`GroupState::applied`]). The
    /// caller holds the events; this crate does no I/O.
    FoldWith(PublicKey),
}

/// The groups a node carries, and its own person.
#[derive(Debug, Clone)]
pub struct Groups {
    me: PublicKey,
    /// Each person's inbox list (kind 10050): the anchors that keep their
    /// DMs (M7). Set by the node from what it holds.
    inboxes: BTreeMap<PublicKey, BTreeSet<PublicKey>>,
    /// `None` while carried without state — a join on its way.
    carried: BTreeMap<PublicKey, Option<GroupState>>,
}

impl Groups {
    pub fn new(me: PublicKey) -> Self {
        Self {
            me,
            inboxes: BTreeMap::new(),
            carried: BTreeMap::new(),
        }
    }

    pub fn me(&self) -> PublicKey {
        self.me
    }

    /// Records `person`'s inbox list: the anchors that keep their DMs.
    pub fn set_inboxes(&mut self, person: PublicKey, anchors: BTreeSet<PublicKey>) {
        if anchors.is_empty() {
            self.inboxes.remove(&person);
        } else {
            self.inboxes.insert(person, anchors);
        }
    }

    /// The people whose inbox list names `anchor`.
    pub fn served_by(&self, anchor: &PublicKey) -> impl Iterator<Item = &PublicKey> {
        self.inboxes
            .iter()
            .filter(move |(_, a)| a.contains(anchor))
            .map(|(p, _)| p)
    }

    /// May `reader` hold a DM wrapped to `recipient`? The recipient, and the
    /// anchors their inbox list names — nobody else.
    fn keeps_dms_for(&self, reader: &PublicKey, recipient: &PublicKey) -> bool {
        reader == recipient
            || self
                .inboxes
                .get(recipient)
                .is_some_and(|a| a.contains(reader))
    }

    /// Starts carrying `group` (created here, or joining).
    pub fn carry(&mut self, group: PublicKey) {
        self.carried.entry(group).or_insert(None);
    }

    /// Stops carrying `group` — a public group read and put down (M13).
    pub fn drop_group(&mut self, group: &PublicKey) {
        self.carried.remove(group);
    }

    /// Replaces a carried group's state with a fresh fold.
    pub fn set_state(&mut self, group: PublicKey, state: Option<GroupState>) {
        if let Some(slot) = self.carried.get_mut(&group) {
            *slot = state;
        }
    }

    pub fn is_carried(&self, group: &PublicKey) -> bool {
        self.carried.contains_key(group)
    }

    pub fn carried(&self) -> impl Iterator<Item = &PublicKey> {
        self.carried.keys()
    }

    pub fn state(&self, group: &PublicKey) -> Option<&GroupState> {
        self.carried.get(group)?.as_ref()
    }

    /// The groups whose state says `p` is a member.
    pub fn groups_of(&self, p: &PublicKey) -> impl Iterator<Item = &GroupState> {
        self.carried
            .values()
            .flatten()
            .filter(move |s| s.is_member(p))
    }

    /// A profile (kind 0) of a member of a public group we carry: anyone's
    /// to read (M13). Not their device bindings — those name devices to
    /// dial, and stay among the people they share a group with.
    fn public_profile(&self, event: &Event) -> bool {
        event.kind == PROFILE
            && self
                .carried
                .values()
                .flatten()
                .any(|s| s.is_public() && s.is_member(&event.pubkey))
    }

    /// Do `a` and `b` share a group we carry?
    pub fn share_a_group(&self, a: &PublicKey, b: &PublicKey) -> bool {
        self.groups_of(a).any(|s| s.is_member(b))
    }

    /// Everyone we share a group with — whom a node syncs with.
    pub fn peers(&self) -> BTreeSet<PublicKey> {
        self.groups_of(&self.me)
            .flat_map(|s| s.members().copied())
            .filter(|p| *p != self.me)
            .collect()
    }

    /// May `event` be stored and forwarded, whoever delivered it? Judged by
    /// its author and its group, never by the peer that sent it — a member
    /// may relay another member's events.
    pub fn judge(&self, event: &Verified, now: Timestamp) -> Result<Judgement, Refusal> {
        if !CARRIED_KINDS.contains(&event.kind) {
            return Err(Refusal::KindNotCarried(event.kind.as_u16()));
        }
        if event.created_at > now + MAX_CLOCK_SKEW {
            return Err(Refusal::FromTheFuture);
        }
        if event.kind == hit_event::dm::GIFT_WRAP {
            // A DM (M7): kept for its recipient, by them and by the anchors
            // they named. The author is a throwaway key — nothing to judge
            // there; the sender's own outbox copy is the node's to keep.
            if event.content.len() > MAX_WRAP_BYTES {
                return Err(Refusal::TooLarge);
            }
            let to = hit_event::dm::recipient_of(event).ok_or(Refusal::Malformed)?;
            return if self.keeps_dms_for(&self.me, &to) {
                Ok(Judgement::Store)
            } else {
                Err(Refusal::NotOurs)
            };
        }
        if is_personal(event.kind) {
            // The bindings and profiles of people we share a group with: how
            // members find and name each other (`DESIGN.md` §8). Nobody
            // else's — but a public group's members' profiles, which anyone
            // may read (M13).
            return if event.pubkey == self.me
                || self.share_a_group(&self.me, &event.pubkey)
                || self.public_profile(event)
            {
                Ok(Judgement::Store)
            } else {
                Err(Refusal::NotAMember)
            };
        }
        let group = group_of(event).ok_or(Refusal::Malformed)?;
        let slot = self.carried.get(&group).ok_or(Refusal::GroupNotCarried)?;
        if STATE_KINDS.contains(&event.kind) {
            return parse_state(event)
                .map(|_| Judgement::Store)
                .ok_or(Refusal::NotSignedByGroup);
        }
        let state = slot.as_ref().ok_or(Refusal::NoState)?;
        if event.kind == CREATE_GROUP {
            return if state.named_owner() == Some(event.pubkey) {
                Ok(Judgement::Store)
            } else {
                Err(Refusal::NotTheOwner)
            };
        }
        if event.kind == CREATE_INVITE {
            // A credential, not a state change: worth keeping only while its
            // author may invite, and until the snapshot passes its expiry.
            // A join checks it again when it is applied.
            if state.is_folded(event, now) {
                return Err(Refusal::Folded);
            }
            return if state
                .base(&event.pubkey)
                .contains(Permissions::CREATE_INVITE)
            {
                Ok(Judgement::Store)
            } else {
                Err(Refusal::Lacks(Permissions::CREATE_INVITE))
            };
        }
        if ACTION_KINDS.contains(&event.kind) {
            if state.is_folded(event, now) {
                return Err(Refusal::Folded);
            }
            return Ok(Judgement::FoldWith(group));
        }
        match event.kind {
            k if k == PRESENCE => {
                if event.created_at + PRESENCE_MAX_AGE < now {
                    return Err(Refusal::Stale);
                }
                if !state.is_member(&event.pubkey) {
                    return Err(Refusal::NotAMember);
                }
                Ok(Judgement::Store)
            }
            k if k == hit_event::group::VOICE_PRESENCE => {
                if event.created_at + PRESENCE_MAX_AGE < now {
                    return Err(Refusal::Stale);
                }
                state.judge_voice(event).map(|()| Judgement::Store)
            }
            k if k == REACTION => state.judge_reaction(event).map(|()| Judgement::Store),
            k if k == hit_event::zap::ZAP_CARRIER => {
                state.judge_zap(event).map(|()| Judgement::Store)
            }
            k if k == DELETION => state.judge_deletion(event).map(|()| Judgement::Store),
            k if k == DELETE_EVENT => state.judge_removal(event).map(|()| Judgement::Store),
            _ => state.judge_message(event).map(|()| Judgement::Store),
        }
    }

    /// May `reader` see `event`? The one rule behind a `REQ`, a negentropy
    /// set and gossip fan-out (`DESIGN.md` §4: list, write *and* fan-out).
    pub fn may_read(&self, reader: &PublicKey, event: &Event, now: Timestamp) -> bool {
        if event.kind == hit_event::dm::GIFT_WRAP {
            return hit_event::dm::recipient_of(event)
                .is_some_and(|to| self.keeps_dms_for(reader, &to));
        }
        if is_personal(event.kind) {
            return event.pubkey == *reader
                || self.share_a_group(reader, &event.pubkey)
                || self.public_profile(event);
        }
        group_of(event)
            .and_then(|g| self.state(&g))
            .is_some_and(|s| s.may_read(reader, event, now))
    }

    /// The group whose gossip carries `event`, if any: its state, its
    /// actions, and messages in channels every member can view. Everything
    /// else travels by sync, which checks the reader per event.
    pub fn gossip_group(&self, event: &Event) -> Option<PublicKey> {
        let group = group_of(event)?;
        let state = self.state(&group)?;
        if CHANNEL_KINDS.contains(&event.kind) || event.kind == hit_event::group::VOICE_PRESENCE {
            return channel_of(event)
                .is_some_and(|c| state.everyone_can_view(c))
                .then_some(group);
        }
        (STATE_KINDS.contains(&event.kind)
            || event.kind == PRESENCE
            || ACTION_KINDS.contains(&event.kind)
            || event.kind == CREATE_GROUP)
            .then_some(group)
    }
}

/// A person's own events rather than a group's: their device bindings and
/// their profile. Readable by them and by anyone they share a group with.
pub fn is_personal(kind: Kind) -> bool {
    kind == DEVICE_BINDING || kind == PROFILE || kind == hit_event::dm::INBOX_LIST
}

/// The order a batch of fetched events is applied in, so that what
/// authorises an event arrives before it: the group key's state, the owner's
/// `9007`, actions in time order, bindings, then messages.
pub fn apply_order(event: &Event) -> (u8, Timestamp) {
    let class = if STATE_KINDS.contains(&event.kind) {
        0
    } else if event.kind == CREATE_GROUP {
        1
    } else if ACTION_KINDS.contains(&event.kind) {
        2
    } else if is_personal(event.kind) {
        3
    } else if event.kind == DELETION || event.kind == DELETE_EVENT {
        5
    } else {
        4
    };
    (class, event.created_at)
}
