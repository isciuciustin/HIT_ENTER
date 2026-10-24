//! A group's state, folded from signed events, and the one place a
//! permission question about it is answered (`DESIGN.md` §6, Design C).
//!
//! ## The fold
//!
//! 1. **The snapshot**: the group key's four state events (`39000`–`39003`),
//!    each at its highest `seq`. Without all four there is no state, and
//!    nothing in the group is accepted — a node that has only half a group
//!    does not guess the other half.
//! 2. **The cutoff** is the newest `created_at` among those four. Whoever
//!    signs a snapshot dates it **one second after** every action it folded
//!    (timestamps are whole seconds).
//! 3. **The owner** is the person the snapshot names, *and* who signed a
//!    `9007` for this group — the group key alone cannot make somebody the
//!    owner of something, as a device cannot be bound without its own proof.
//! 4. **Actions dated at or after the cutoff** (`9000`, `9001`, `9021`, `9022`) are
//!    applied in time order, each one only if it is valid against the state
//!    the actions before it produced. An invalid action is skipped, not
//!    fatal: every peer skips the same ones. **Within one second** — the
//!    clock's resolution — actions are applied in id order *until nothing
//!    more applies*, so a join whose inviter joined in the same second is not
//!    refused for losing a coin toss on ids. Across seconds the order is
//!    strict: an old action never jumps past a newer one.
//! 5. **Invites (`9009`) are credentials, not state changes**: a join is
//!    checked against them when the join is applied, whatever their age.
//!
//! Actions dated before the cutoff are *folded*: the snapshot already says
//! what they did, or the group key's holder chose otherwise ("the owner has
//! the last word"). They are not applied again and not synced.
//!
//! **The honest limit, said once**: an action the snapshot's signer had not
//! seen, dated before the snapshot, is lost — a peer whose clock was behind,
//! or whose action reached the signer late. It is visible (the action is not
//! in the state its author expected) and can be redone. A sequencer that is
//! allowed to be late is allowed to miss things.
//!
//! ## How an answer is computed (HIT_ENTER_2 `roles.rs`, as a pure function)
//!
//! 1. **The owner short-circuits** to every permission, and nobody — the
//!    owner included — may act on the owner.
//! 2. **Union the role bits**: the default role plus every role held.
//! 3. **Apply the channel override**: `(base & !deny) | allow`, each unioned
//!    across the same roles, so an explicit allow wins.
//! 4. **Sanitize**: bits this build does not define are dropped.
//!
//! The escalation rule: **you may not touch a role at or above your own
//! rank, and you may not grant a permission you do not hold.**

use std::collections::{BTreeMap, BTreeSet};

use hit_event::group::{
    ACTION_KINDS, Action, CHANNEL_KINDS, CREATE_GROUP, CREATE_INVITE, Channel, DEFAULT_ROLE, Edit,
    GROUP_ADMINS, GROUP_MEMBERS, GROUP_METADATA, GROUP_ROLES, MAX_NEWCOMER_WAIT, MAX_REACTION,
    MAX_SLOWMODE, PUBLIC_NEWCOMER_FLOOR,
    OWNER_MARK, Override, RoleDef, STATE_KINDS, StatePiece, channel_of, group_of, parse_action,
    parse_state,
};
use hit_event::replaces;
use nostr::prelude::{Event, EventId, PublicKey, Timestamp};

use crate::{Refusal, perm::Permissions};

/// Rank, for the escalation rule: the position of a member's highest role,
/// or [`Rank::FLOOR`] with only the default one. The owner has no rank; it
/// is checked before rank is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank(i64);

impl Rank {
    pub const FLOOR: Self = Self(i64::MIN);
}

/// A span in which a person's messages and reactions are not valid: from
/// a kick, ban or leave until they are let back in, or a mute's span. Every
/// honest peer refuses their events dated inside one, and drops any it took
/// before the span reached it (`DESIGN.md` §6, "what enforcement means").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Silence {
    pub who: PublicKey,
    pub from: Timestamp,
    /// `None`: until they are a member again.
    pub until: Option<Timestamp>,
    /// A mute rather than a removal.
    pub mute: bool,
}

impl Silence {
    pub fn covers(&self, t: Timestamp) -> bool {
        self.from <= t && self.until.is_none_or(|u| t < u)
    }
}

/// Names, ids and the like in a `9002`: at most this many characters.
pub const MAX_LABEL: usize = 64;

#[derive(Debug, Clone)]
struct Invite {
    author: PublicKey,
    created_at: Timestamp,
    expires: Option<Timestamp>,
}

/// One group, as every peer holding the same events computes it.
#[derive(Debug, Clone)]
pub struct GroupState {
    id: PublicKey,
    seq: u64,
    cutoff: Timestamp,
    name: String,
    /// Anyone may find, read and join it (M13).
    public: bool,
    /// Seconds a newcomer waits before posting, as the group set it; see
    /// [`Self::newcomer_wait`] for what applies (M13).
    newcomer_wait: u32,
    /// When each member who joined by themselves (a `9021`) since the
    /// cutoff did so — the newcomers. Members the snapshot names are not.
    joined: BTreeMap<PublicKey, Timestamp>,
    owner: Option<PublicKey>,
    named_owner: Option<PublicKey>,
    channels: Vec<Channel>,
    overrides: Vec<Override>,
    roles: BTreeMap<String, RoleDef>,
    members: BTreeMap<PublicKey, BTreeSet<String>>,
    invites: BTreeMap<String, Vec<Invite>>,
    applied: BTreeSet<EventId>,
    bans: BTreeSet<PublicKey>,
    mutes: BTreeMap<PublicKey, Timestamp>,
    silences: Vec<Silence>,
}

impl GroupState {
    /// Folds `events` into the state of `group`. Events of other groups, and
    /// kinds the fold does not read (messages, bindings), are ignored, so a
    /// caller may pass everything it holds for the group.
    ///
    /// `None` until the group key's four state events are all present.
    pub fn fold<'a>(group: PublicKey, events: impl IntoIterator<Item = &'a Event>) -> Option<Self> {
        let mut pieces: BTreeMap<u16, &Event> = BTreeMap::new();
        let mut creators = BTreeSet::new();
        let mut actions = Vec::new();
        for ev in events {
            if group_of(ev) != Some(group) {
                continue;
            }
            if STATE_KINDS.contains(&ev.kind) {
                if parse_state(ev).is_none() {
                    continue;
                }
                let slot = pieces.entry(ev.kind.as_u16()).or_insert(ev);
                if replaces(ev, slot) {
                    *slot = ev;
                }
            } else if ev.kind == CREATE_GROUP {
                creators.insert(ev.pubkey);
            } else if ACTION_KINDS.contains(&ev.kind) {
                actions.push(ev);
            }
        }

        let mut state = Self {
            id: group,
            seq: 0,
            cutoff: Timestamp::from(0),
            name: String::new(),
            public: false,
            newcomer_wait: 0,
            joined: BTreeMap::new(),
            owner: None,
            named_owner: None,
            channels: Vec::new(),
            overrides: Vec::new(),
            roles: BTreeMap::new(),
            members: BTreeMap::new(),
            invites: BTreeMap::new(),
            applied: BTreeSet::new(),
            bans: BTreeSet::new(),
            mutes: BTreeMap::new(),
            silences: Vec::new(),
        };
        let mut assignments = BTreeMap::new();
        let mut named_owner = None;
        for kind in STATE_KINDS {
            let ev = pieces.get(&kind.as_u16())?;
            let (_, seq, piece) = parse_state(ev)?;
            state.seq = state.seq.max(seq);
            state.cutoff = state.cutoff.max(ev.created_at);
            match piece {
                StatePiece::Metadata {
                    name,
                    public,
                    newcomer_wait,
                    channels,
                    overrides,
                } => {
                    state.name = name;
                    state.public = public;
                    state.newcomer_wait = newcomer_wait;
                    state.channels = channels;
                    state.overrides = overrides;
                }
                StatePiece::Admins {
                    owner,
                    assignments: a,
                } => {
                    named_owner = owner;
                    assignments = a;
                }
                StatePiece::Members {
                    members,
                    bans,
                    mutes,
                } => {
                    state.members = members.into_iter().map(|p| (p, BTreeSet::new())).collect();
                    state.bans = bans;
                    state.mutes = mutes;
                }
                StatePiece::Roles(roles) => {
                    for mut r in roles {
                        if r.id == OWNER_MARK {
                            continue;
                        }
                        r.perms = Permissions(r.perms).sanitized().bits();
                        state.roles.insert(r.id.clone(), r);
                    }
                }
            }
        }
        // A group without a default role still needs an answer to "what may
        // a plain member do": nothing.
        state
            .roles
            .entry(DEFAULT_ROLE.to_owned())
            .or_insert_with(|| RoleDef {
                id: DEFAULT_ROLE.to_owned(),
                name: DEFAULT_ROLE.to_owned(),
                perms: 0,
                position: 0,
            });
        for (p, roles) in assignments {
            let roles = state.known_roles(roles);
            if let Some(held) = state.members.get_mut(&p) {
                *held = roles;
            }
        }
        state.named_owner = named_owner;
        state.owner = named_owner.filter(|o| creators.contains(o));
        if let Some(owner) = state.owner {
            state.members.insert(owner, BTreeSet::new());
        }

        // A mute the snapshot carries silences from the cutoff: when it
        // began is folded away.
        let cutoff = state.cutoff;
        state.silences = state
            .mutes
            .iter()
            .filter(|(_, u)| **u > cutoff)
            .map(|(p, u)| Silence {
                who: *p,
                from: cutoff,
                until: Some(*u),
                mute: true,
            })
            .collect();

        actions.sort_by_key(|ev| (ev.created_at, ev.id));
        for ev in &actions {
            if ev.kind == CREATE_INVITE
                && let Some((_, Action::Invite { code, expires })) = parse_action(ev)
            {
                state.invites.entry(code).or_default().push(Invite {
                    author: ev.pubkey,
                    created_at: ev.created_at,
                    expires,
                });
            }
        }
        let live = actions
            .into_iter()
            .filter(|ev| ev.kind != CREATE_INVITE && ev.created_at >= cutoff);
        let mut second: Vec<&Event> = Vec::new();
        for ev in live {
            if second
                .first()
                .is_some_and(|f| f.created_at != ev.created_at)
            {
                state.apply_second(std::mem::take(&mut second));
            }
            second.push(ev);
        }
        state.apply_second(second);
        Some(state)
    }

    /// Applies one second's actions, in id order, until none of the rest
    /// applies.
    fn apply_second(&mut self, mut pending: Vec<&Event>) {
        loop {
            let before = pending.len();
            pending.retain(|ev| {
                if self.apply(ev).is_ok() {
                    self.applied.insert(ev.id);
                    false
                } else {
                    true
                }
            });
            if pending.len() == before || pending.is_empty() {
                return;
            }
        }
    }

    /// Applies one action if it is valid against the current state.
    fn apply(&mut self, ev: &Event) -> Result<(), Refusal> {
        let (_, action) = parse_action(ev).ok_or(Refusal::Malformed)?;
        let actor = ev.pubkey;
        match action {
            Action::Put { target, roles } => {
                self.require_member(&actor)?;
                if Some(target) == self.owner {
                    return Err(Refusal::TheOwner);
                }
                let wanted: BTreeSet<String> =
                    roles.into_iter().filter(|r| r != DEFAULT_ROLE).collect();
                if wanted
                    .iter()
                    .any(|r| r == OWNER_MARK || !self.roles.contains_key(r))
                {
                    return Err(Refusal::UnknownRole);
                }
                let current = self.members.get(&target).cloned();
                if current.is_none() {
                    if self.bans.contains(&target) {
                        return Err(Refusal::Banned);
                    }
                    self.require(&actor, Permissions::CREATE_INVITE)?;
                }
                let held = current.clone().unwrap_or_default();
                if wanted != held {
                    self.require(&actor, Permissions::MANAGE_ROLES)?;
                    if current.is_some() && !self.outranks(&actor, &target) {
                        return Err(Refusal::Outranked);
                    }
                    let mut granting = Permissions::NONE;
                    for r in wanted.symmetric_difference(&held) {
                        let role = self.roles.get(r).ok_or(Refusal::UnknownRole)?;
                        if !self.outranks_role(&actor, role) {
                            return Err(Refusal::Outranked);
                        }
                        if wanted.contains(r) {
                            granting = granting.union(Permissions(role.perms));
                        }
                    }
                    if !self.can_grant(&actor, granting) {
                        return Err(Refusal::CannotGrant);
                    }
                }
                if current.is_none() {
                    self.let_back_in(target, ev.created_at);
                }
                self.members.insert(target, wanted);
            }
            Action::Remove { target, ban } => {
                self.require_member(&actor)?;
                if Some(target) == self.owner {
                    return Err(Refusal::TheOwner);
                }
                let member = self.members.contains_key(&target);
                // Banning someone who already left is how you keep them
                // out; kicking someone who is not here is nothing.
                if !member && !ban {
                    return Err(Refusal::NotAMember);
                }
                self.require(
                    &actor,
                    if ban {
                        Permissions::BAN_MEMBERS
                    } else {
                        Permissions::KICK_MEMBERS
                    },
                )?;
                if !self.outranks(&actor, &target) {
                    return Err(Refusal::Outranked);
                }
                if member {
                    self.members.remove(&target);
                    self.mutes.remove(&target);
                    self.joined.remove(&target);
                }
                // A ban silences its target from its time on, member or not:
                // in the same second as their join, the fold may apply the
                // ban first, and what they said after it must still go
                // (found by M5's done-when test).
                if member || ban {
                    self.silence(target, ev.created_at, None, false);
                }
                if ban {
                    self.bans.insert(target);
                }
            }
            Action::Unban { target } => {
                self.require_member(&actor)?;
                self.require(&actor, Permissions::BAN_MEMBERS)?;
                if !self.bans.remove(&target) {
                    return Err(Refusal::NotBanned);
                }
            }
            Action::Mute { target, until } => {
                self.require_member(&actor)?;
                if Some(target) == self.owner {
                    return Err(Refusal::TheOwner);
                }
                if !self.members.contains_key(&target) {
                    return Err(Refusal::NotAMember);
                }
                self.require(&actor, Permissions::MUTE_MEMBERS)?;
                if !self.outranks(&actor, &target) {
                    return Err(Refusal::Outranked);
                }
                let at = ev.created_at;
                // Lifting (or shortening) a mute ends its open span now.
                for s in self.silences.iter_mut() {
                    if s.who == target && s.mute && s.covers(at) {
                        s.until = Some(at);
                    }
                }
                match until {
                    Some(u) if u > at => {
                        self.mutes.insert(target, u);
                        self.silence(target, at, Some(u), true);
                    }
                    Some(_) => return Err(Refusal::Malformed),
                    None => {
                        if self.mutes.remove(&target).is_none() {
                            return Err(Refusal::NotMuted);
                        }
                    }
                }
            }
            Action::Edit(edits) => {
                self.require_member(&actor)?;
                // Judged against the state *before* the edit, all of it: one
                // edit in a batch cannot hand the actor the power the next
                // one needs.
                let before = self.clone();
                let mut next = self.clone();
                for e in edits {
                    next.edit(&before, &actor, e)?;
                }
                *self = next;
            }
            Action::Join { code } => {
                if self.members.contains_key(&actor) {
                    return Err(Refusal::AlreadyAMember);
                }
                let at = ev.created_at;
                // No code: a public group's open door (M13), as it stood at
                // this point in the fold.
                let honoured = match &code {
                    None => self.public,
                    Some(code) => self.invites.get(code).is_some_and(|invites| {
                        invites.iter().any(|i| {
                            i.created_at <= at
                                && i.expires.is_none_or(|e| at <= e)
                                && self.base(&i.author).contains(Permissions::CREATE_INVITE)
                        })
                    }),
                };
                if !honoured {
                    return Err(Refusal::NoSuchInvite);
                }
                if self.bans.contains(&actor) {
                    return Err(Refusal::Banned);
                }
                self.let_back_in(actor, at);
                self.members.insert(actor, BTreeSet::new());
                // Everyone who let themselves in is a newcomer — by invite
                // too, since links get posted. Someone a moderator put in
                // (`9000`) is not.
                self.joined.insert(actor, at);
            }
            Action::Leave => {
                if Some(actor) == self.owner {
                    return Err(Refusal::TheOwner);
                }
                self.members
                    .remove(&actor)
                    .ok_or(Refusal::NotAMember)
                    .map(|_| ())?;
                self.mutes.remove(&actor);
                self.joined.remove(&actor);
                self.silence(actor, ev.created_at, None, false);
            }
            // Recorded as credentials by the fold, never applied.
            Action::Invite { .. } => return Err(Refusal::Malformed),
        }
        Ok(())
    }

    fn silence(&mut self, who: PublicKey, from: Timestamp, until: Option<Timestamp>, mute: bool) {
        self.silences.push(Silence {
            who,
            from,
            until,
            mute,
        });
    }

    /// `who` is a member again from `at`: their open removal span ends.
    fn let_back_in(&mut self, who: PublicKey, at: Timestamp) {
        for s in self.silences.iter_mut() {
            if s.who == who && !s.mute && s.until.is_none() {
                s.until = Some(at);
            }
        }
    }

    /// One edit of a `9002`, checked against `before` — the state the whole
    /// batch is judged by — and applied to `self`.
    fn edit(&mut self, before: &Self, actor: &PublicKey, e: Edit) -> Result<(), Refusal> {
        let label = |s: &str| {
            let n = s.trim().chars().count();
            if n == 0 || n > MAX_LABEL {
                Err(Refusal::BadEdit)
            } else {
                Ok(())
            }
        };
        match e {
            Edit::Name(n) => {
                before.require(actor, Permissions::MANAGE_GROUP)?;
                label(&n)?;
                self.name = n.trim().to_owned();
            }
            // Who may read and join is the owner's call alone (M13): no
            // role grants it, as none grants acting on the owner.
            Edit::Public(public) => {
                if Some(*actor) != before.owner {
                    return Err(Refusal::NotTheOwner);
                }
                self.public = public;
            }
            // Slowmode's permission: both pace who may post when. Below a
            // public group's floor it is accepted, and the floor applies.
            Edit::NewcomerWait(secs) => {
                before.require(actor, Permissions::MANAGE_CHANNELS)?;
                if secs > MAX_NEWCOMER_WAIT {
                    return Err(Refusal::BadEdit);
                }
                self.newcomer_wait = secs;
            }
            Edit::Channel(c) => {
                before.require(actor, Permissions::MANAGE_CHANNELS)?;
                label(&c.id)?;
                label(&c.name)?;
                if c.slowmode > MAX_SLOWMODE {
                    return Err(Refusal::BadEdit);
                }
                match self.channels.iter_mut().find(|x| x.id == c.id) {
                    Some(x) => *x = c,
                    None => self.channels.push(c),
                }
            }
            Edit::ChannelDelete(id) => {
                before.require(actor, Permissions::MANAGE_CHANNELS)?;
                if !self.channels.iter().any(|c| c.id == id) {
                    return Err(Refusal::NoSuchChannel);
                }
                self.channels.retain(|c| c.id != id);
                self.overrides.retain(|o| o.channel != id);
            }
            Edit::Override(mut o) => {
                before.require(actor, Permissions::MANAGE_ROLES)?;
                if !self.channels.iter().any(|c| c.id == o.channel) {
                    return Err(Refusal::NoSuchChannel);
                }
                let role = before.roles.get(&o.role).ok_or(Refusal::UnknownRole)?;
                if !before.outranks_role(actor, role) {
                    return Err(Refusal::Outranked);
                }
                // Only what a channel can change survives, and an override
                // cannot hand out what its author lacks there.
                let scoped = |bits: u64| {
                    Permissions(bits)
                        .sanitized()
                        .intersection(Permissions::CHANNEL_SCOPED)
                        .bits()
                };
                o.allow = scoped(o.allow);
                o.deny = scoped(o.deny);
                if !Permissions(o.allow)
                    .beyond(before.effective(actor, &o.channel))
                    .is_empty()
                {
                    return Err(Refusal::CannotGrant);
                }
                self.overrides
                    .retain(|x| !(x.channel == o.channel && x.role == o.role));
                if o.allow != 0 || o.deny != 0 {
                    self.overrides.push(o);
                }
            }
            Edit::Role(mut r) => {
                before.require(actor, Permissions::MANAGE_ROLES)?;
                label(&r.id)?;
                label(&r.name)?;
                if r.id == OWNER_MARK {
                    return Err(Refusal::BadEdit);
                }
                r.perms = Permissions(r.perms).sanitized().bits();
                if r.id == DEFAULT_ROLE {
                    // Everyone holds it, so only the owner changes it, and
                    // it stays at the bottom.
                    if Some(*actor) != before.owner {
                        return Err(Refusal::NotTheOwner);
                    }
                    if r.position != 0 {
                        return Err(Refusal::BadEdit);
                    }
                } else {
                    if r.position <= 0 {
                        return Err(Refusal::BadEdit);
                    }
                    if let Some(old) = before.roles.get(&r.id)
                        && !before.outranks_role(actor, old)
                    {
                        return Err(Refusal::Outranked);
                    }
                    if !before.outranks_role(actor, &r) {
                        return Err(Refusal::Outranked);
                    }
                }
                if !before.can_grant(actor, Permissions(r.perms)) {
                    return Err(Refusal::CannotGrant);
                }
                self.roles.insert(r.id.clone(), r);
            }
            Edit::RoleDelete(id) => {
                before.require(actor, Permissions::MANAGE_ROLES)?;
                if id == DEFAULT_ROLE {
                    return Err(Refusal::BadEdit);
                }
                let role = before.roles.get(&id).ok_or(Refusal::UnknownRole)?;
                if !before.outranks_role(actor, role) {
                    return Err(Refusal::Outranked);
                }
                self.roles.remove(&id);
                for held in self.members.values_mut() {
                    held.remove(&id);
                }
                self.overrides.retain(|o| o.role != id);
            }
        }
        Ok(())
    }

    fn known_roles(&self, roles: BTreeSet<String>) -> BTreeSet<String> {
        roles
            .into_iter()
            .filter(|r| r != DEFAULT_ROLE && self.roles.contains_key(r))
            .collect()
    }

    fn require_member(&self, p: &PublicKey) -> Result<(), Refusal> {
        if self.is_member(p) {
            Ok(())
        } else {
            Err(Refusal::NotAMember)
        }
    }

    fn require(&self, p: &PublicKey, needed: Permissions) -> Result<(), Refusal> {
        if self.base(p).contains(needed) {
            Ok(())
        } else {
            Err(Refusal::Lacks(needed))
        }
    }

    // --- the answers ---------------------------------------------------

    pub fn id(&self) -> PublicKey {
        self.id
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The newest `created_at` of the snapshot: actions dated before it are
    /// folded.
    pub fn cutoff(&self) -> Timestamp {
        self.cutoff
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Anyone may find, read and join it (M13).
    pub fn is_public(&self) -> bool {
        self.public
    }

    /// The wait that applies to a newcomer, in seconds: the group's, and
    /// never under [`PUBLIC_NEWCOMER_FLOOR`] in a public group (M13).
    pub fn newcomer_wait(&self) -> u32 {
        if self.public {
            self.newcomer_wait.max(PUBLIC_NEWCOMER_FLOOR)
        } else {
            self.newcomer_wait
        }
    }

    /// The wait the group set, before any floor — what settings shows.
    pub fn newcomer_wait_set(&self) -> u32 {
        self.newcomer_wait
    }

    /// When `p` may first post, if they are a newcomer still waiting at
    /// `now`: their join plus [`Self::newcomer_wait`].
    /// No wait, no rule: a message dated before its author's join stays
    /// as acceptable as it was before M13.
    pub fn posts_from(&self, p: &PublicKey, now: Timestamp) -> Option<Timestamp> {
        let wait = self.newcomer_wait();
        if wait == 0 {
            return None;
        }
        let from = *self.joined.get(p)? + u64::from(wait);
        (from > now).then_some(from)
    }

    /// The owner: named by the group key *and* the signer of a `9007`.
    pub fn owner(&self) -> Option<PublicKey> {
        self.owner
    }

    /// Whom the group key names as owner, whether or not their `9007` has
    /// arrived — the only person whose `9007` is worth storing.
    pub fn named_owner(&self) -> Option<PublicKey> {
        self.named_owner
    }

    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub fn is_member(&self, p: &PublicKey) -> bool {
        self.members.contains_key(p)
    }

    pub fn members(&self) -> impl Iterator<Item = &PublicKey> {
        self.members.keys()
    }

    /// The roles `p` holds beyond the default one.
    pub fn roles_of(&self, p: &PublicKey) -> Option<&BTreeSet<String>> {
        self.members.get(p)
    }

    /// Did the fold apply this action?
    pub fn applied(&self, id: &EventId) -> bool {
        self.applied.contains(id)
    }

    /// Group-wide permissions, before any channel override. A non-member
    /// holds nothing.
    pub fn base(&self, p: &PublicKey) -> Permissions {
        if Some(*p) == self.owner {
            return Permissions::ALL;
        }
        let Some(held) = self.members.get(p) else {
            return Permissions::NONE;
        };
        self.base_of(held)
    }

    /// What holding `held` (and the default role) grants, group-wide.
    fn base_of(&self, held: &BTreeSet<String>) -> Permissions {
        let mut perms = Permissions::NONE;
        for r in std::iter::once(DEFAULT_ROLE).chain(held.iter().map(String::as_str)) {
            if let Some(role) = self.roles.get(r) {
                perms = perms.union(Permissions(role.perms));
            }
        }
        perms.sanitized()
    }

    /// What `p` may do in `channel`: `(base & !deny) | allow`, the allows
    /// and denies each unioned over the default role and every role held,
    /// so a deny from one role never cancels an allow from another.
    pub fn effective(&self, p: &PublicKey, channel: &str) -> Permissions {
        if Some(*p) == self.owner {
            return Permissions::ALL;
        }
        let Some(held) = self.members.get(p) else {
            return Permissions::NONE;
        };
        self.effective_of(held, channel)
    }

    /// What holding `held` (and the default role) grants in `channel`.
    fn effective_of(&self, held: &BTreeSet<String>, channel: &str) -> Permissions {
        let base = self.base_of(held);
        let (mut allow, mut deny) = (Permissions::NONE, Permissions::NONE);
        for o in self.overrides.iter().filter(|o| o.channel == channel) {
            if o.role == DEFAULT_ROLE || held.contains(&o.role) {
                allow = allow.union(Permissions(o.allow));
                deny = deny.union(Permissions(o.deny));
            }
        }
        let allow = allow.intersection(Permissions::CHANNEL_SCOPED);
        let deny = deny.intersection(Permissions::CHANNEL_SCOPED);
        base.without(deny).union(allow).sanitized()
    }

    pub fn rank(&self, p: &PublicKey) -> Rank {
        self.members
            .get(p)
            .and_then(|held| {
                held.iter()
                    .filter_map(|r| self.roles.get(r))
                    .map(|r| r.position)
                    .max()
            })
            .map_or(Rank::FLOOR, Rank)
    }

    /// May `actor` act on `target`? **The owner check comes first and the
    /// order is the rule**: nobody acts on the owner, the owner included.
    /// Equal rank is not enough — two moderators kicking each other is a
    /// standoff, not a hierarchy.
    pub fn outranks(&self, actor: &PublicKey, target: &PublicKey) -> bool {
        if Some(*target) == self.owner {
            return false;
        }
        if Some(*actor) == self.owner {
            return true;
        }
        self.rank(actor) > self.rank(target)
    }

    /// May `actor` hand out (or take back) the role `id`? What a role
    /// picker offers — the fold checks it again on the action.
    pub fn may_assign(&self, actor: &PublicKey, id: &str) -> bool {
        self.roles.get(id).is_some_and(|r| {
            id != DEFAULT_ROLE
                && self.base(actor).contains(Permissions::MANAGE_ROLES)
                && self.outranks_role(actor, r)
                && self.can_grant(actor, Permissions(r.perms))
        })
    }

    fn outranks_role(&self, actor: &PublicKey, role: &RoleDef) -> bool {
        Some(*actor) == self.owner || self.rank(actor) > Rank(role.position)
    }

    /// You cannot grant what you do not hold.
    fn can_grant(&self, actor: &PublicKey, granting: Permissions) -> bool {
        granting.beyond(self.base(actor)).is_empty()
    }

    fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// May `msg` (a chat message in this group) be stored and shown? Its
    /// author must be a member who can view and post in an existing channel.
    pub fn judge_message(&self, msg: &Event) -> Result<(), Refusal> {
        // Its files, if any, within every limit (M8): one bad `imeta` and
        // the message is not valid at all.
        if msg.kind == hit_event::group::CHAT_MESSAGE
            && hit_event::group::attachments_of(msg).is_err()
        {
            return Err(Refusal::Malformed);
        }
        self.judge_in_channel(
            msg,
            Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES),
        )?;
        if let Some(s) = self.silence_at(&msg.pubkey, msg.created_at) {
            return Err(if s.mute {
                Refusal::Muted
            } else {
                Refusal::NotAMember
            });
        }
        // A newcomer's message dated inside their wait (M13). Judged by its
        // own date, as slowmode: a patched app can date its join early.
        if let Some(from) = self.posts_from(&msg.pubkey, msg.created_at) {
            let left = from.as_secs().saturating_sub(msg.created_at.as_secs());
            return Err(Refusal::Newcomer(u32::try_from(left).unwrap_or(u32::MAX)));
        }
        Ok(())
    }

    /// A voice presence (M9): a member who may view the channel, and the
    /// channel a voice channel. Joining is viewing; whether they may be
    /// heard is the call's question (`SEND_MESSAGES`).
    pub fn judge_voice(&self, ev: &Event) -> Result<(), Refusal> {
        let channel = channel_of(ev).ok_or(Refusal::Malformed)?;
        let c = self.channel(channel).ok_or(Refusal::NoSuchChannel)?;
        if c.kind != hit_event::group::ChannelKind::Voice {
            return Err(Refusal::NoSuchChannel);
        }
        self.require_member(&ev.pubkey)?;
        if self
            .effective(&ev.pubkey, channel)
            .contains(Permissions::VIEW_CHANNEL)
        {
            Ok(())
        } else {
            Err(Refusal::Lacks(Permissions::VIEW_CHANNEL))
        }
    }

    /// A moderator's `9005`: `MANAGE_MESSAGES` in its channel — HIT_ENTER_2's
    /// rule, no rank: a moderator of a channel may remove anyone's words
    /// there. Which targets it actually removes is the node's, since they
    /// may not have arrived: only messages and reactions in that channel.
    pub fn judge_removal(&self, ev: &Event) -> Result<(), Refusal> {
        if hit_event::group::targets_of(ev).is_empty() {
            return Err(Refusal::Malformed);
        }
        self.judge_in_channel(
            ev,
            Permissions::VIEW_CHANNEL.union(Permissions::MANAGE_MESSAGES),
        )
    }

    /// How many actions apply on top of the snapshot — what a compaction
    /// would fold. 0: the snapshot already says everything.
    pub fn live_actions(&self) -> usize {
        self.applied.len()
    }

    /// The members holding `role` (by id).
    pub fn holders_of<'a>(&'a self, role: &'a str) -> impl Iterator<Item = &'a PublicKey> {
        self.members
            .iter()
            .filter(move |(_, held)| held.contains(role))
            .map(|(p, _)| p)
    }

    /// The span silencing `who` at `t`, if any.
    pub fn silence_at(&self, who: &PublicKey, t: Timestamp) -> Option<&Silence> {
        self.silences.iter().find(|s| s.who == *who && s.covers(t))
    }

    /// Every silence span the live actions produced — what a node purges by.
    pub fn silences(&self) -> &[Silence] {
        &self.silences
    }

    pub fn is_banned(&self, p: &PublicKey) -> bool {
        self.bans.contains(p)
    }

    pub fn bans(&self) -> impl Iterator<Item = &PublicKey> {
        self.bans.iter()
    }

    /// Until when `p` is muted, as of `now`.
    pub fn muted_until(&self, p: &PublicKey, now: Timestamp) -> Option<Timestamp> {
        self.mutes.get(p).copied().filter(|u| *u > now)
    }

    /// The seconds a member must leave between messages in `channel`.
    pub fn slowmode(&self, channel: &str) -> u32 {
        self.channel(channel).map_or(0, |c| c.slowmode)
    }

    pub fn roles(&self) -> impl Iterator<Item = &RoleDef> {
        self.roles.values()
    }

    pub fn overrides(&self) -> &[Override] {
        &self.overrides
    }

    /// A reaction: as a message — there is no separate bit for it — with
    /// one target and a short, non-empty content.
    pub fn judge_reaction(&self, ev: &Event) -> Result<(), Refusal> {
        let len = ev.content.chars().count();
        if hit_event::group::targets_of(ev).len() != 1 || len == 0 || len > MAX_REACTION {
            return Err(Refusal::Malformed);
        }
        self.judge_message(ev)
    }

    /// A zap receipt carried in (M10): by a member who can view the
    /// channel — bringing in a receipt says nothing, so no permission to
    /// send is needed — and only if the receipt inside passes NIP-57's
    /// checks and agrees with the carrier. Whether its signer is the
    /// recipient's provider is the recipient's profile's to say, and is
    /// checked where totals are counted: the profile may arrive later.
    pub fn judge_zap(&self, ev: &Event) -> Result<(), Refusal> {
        match hit_event::zap::carried(ev) {
            Ok(_) => self.judge_in_channel(ev, Permissions::VIEW_CHANNEL),
            Err(hit_event::zap::ZapError::TooLarge) => Err(Refusal::TooLarge),
            Err(_) => Err(Refusal::Malformed),
        }
    }

    /// A deletion: by a member who can view the channel. *Whose* events it
    /// withdraws is the store's rule — only its author's own (NIP-09) — since
    /// the targets may not have arrived yet.
    pub fn judge_deletion(&self, ev: &Event) -> Result<(), Refusal> {
        if hit_event::group::targets_of(ev).is_empty() {
            return Err(Refusal::Malformed);
        }
        self.judge_in_channel(ev, Permissions::VIEW_CHANNEL)
    }

    fn judge_in_channel(&self, ev: &Event, needed: Permissions) -> Result<(), Refusal> {
        let channel = channel_of(ev).ok_or(Refusal::Malformed)?;
        self.channel(channel).ok_or(Refusal::NoSuchChannel)?;
        self.require_member(&ev.pubkey)?;
        if self.effective(&ev.pubkey, channel).contains(needed) {
            Ok(())
        } else {
            Err(Refusal::Lacks(needed))
        }
    }

    /// An action the snapshot has folded, or an invite that expired before
    /// it: kept by nobody, synced to nobody. Expired is not enough — a join
    /// is honoured by an invite valid *at the join's time*, so until the
    /// snapshot passes the expiry, a node that arrives later needs the
    /// invite to apply the join (found on the first VPS anchor: four
    /// members, and everything they wrote, refused as "not a member").
    pub fn is_folded(&self, ev: &Event, now: Timestamp) -> bool {
        if ev.kind == CREATE_INVITE {
            return matches!(
                parse_action(ev),
                Some((_, Action::Invite { expires: Some(e), .. })) if e < now && e < self.cutoff
            );
        }
        ACTION_KINDS.contains(&ev.kind) && ev.created_at < self.cutoff
    }

    /// May `reader` see `ev`, an event of this group? Members see the
    /// group's state and actions; a message only if they can view its
    /// channel — the same rule for a `REQ`, for negentropy and for fan-out.
    pub fn may_read(&self, reader: &PublicKey, ev: &Event, now: Timestamp) -> bool {
        // The one thing a removed person may read: the action that removed
        // them — so their own app learns it (by sync: their gossip is cut
        // the moment the removal lands). Even folded, since a snapshot they
        // cannot read would otherwise be the only record (M5, found
        // driving two windows).
        if !self.is_member(reader) {
            if matches!(
                parse_action(ev),
                Some((_, Action::Remove { target, .. })) if target == *reader
            ) {
                return true;
            }
            // M13: a public group reads to anyone as to a plain member who
            // sees only its open channels — its actions and invites too,
            // since a reader's own node must fold them to know who is a
            // member, or it would refuse every message it is shown.
            if !self.public || self.is_folded(ev, now) {
                return false;
            }
            if CHANNEL_KINDS.contains(&ev.kind) || ev.kind == hit_event::group::VOICE_PRESENCE {
                return channel_of(ev).is_some_and(|c| self.open_to_readers(c));
            }
            return true;
        }
        if self.is_folded(ev, now) {
            return false;
        }
        if CHANNEL_KINDS.contains(&ev.kind) || ev.kind == hit_event::group::VOICE_PRESENCE {
            return channel_of(ev).is_some_and(|c| {
                self.effective(reader, c)
                    .contains(Permissions::VIEW_CHANNEL)
            });
        }
        true
    }

    /// May anyone read `channel` (M13)? A public group's channel that a
    /// plain member — no roles — can view. "Mods only" channels are not.
    pub fn open_to_readers(&self, channel: &str) -> bool {
        self.public
            && self.channel(channel).is_some()
            && self
                .effective_of(&BTreeSet::new(), channel)
                .contains(Permissions::VIEW_CHANNEL)
    }

    /// Can every member view `channel`? Only then may its messages go over
    /// the group's gossip, whose allow-list is every member. A private
    /// channel's messages travel by sync until it has its own instance.
    pub fn everyone_can_view(&self, channel: &str) -> bool {
        self.members.keys().all(|p| {
            self.effective(p, channel)
                .contains(Permissions::VIEW_CHANNEL)
        })
    }

    /// The state as a snapshot at `seq + 1` — what a group-key holder signs
    /// to compact the actions folded so far (`DESIGN.md` §6 step 4).
    pub fn to_snapshot(&self) -> hit_event::group::Snapshot {
        hit_event::group::Snapshot {
            seq: self.seq + 1,
            name: self.name.clone(),
            public: self.public,
            newcomer_wait: self.newcomer_wait,
            channels: self.channels.clone(),
            overrides: self.overrides.clone(),
            roles: self.roles.values().cloned().collect(),
            owner: self.owner,
            members: self.members.clone(),
            bans: self.bans.clone(),
            mutes: self.mutes.clone(),
        }
    }
}

/// The kinds the fold reads — what a node loads from its store to fold.
pub const FOLD_KINDS: [nostr::prelude::Kind; 12] = [
    GROUP_METADATA,
    GROUP_ADMINS,
    GROUP_MEMBERS,
    GROUP_ROLES,
    CREATE_GROUP,
    ACTION_KINDS[0],
    ACTION_KINDS[1],
    ACTION_KINDS[2],
    ACTION_KINDS[3],
    ACTION_KINDS[4],
    ACTION_KINDS[5],
    ACTION_KINDS[6],
];
