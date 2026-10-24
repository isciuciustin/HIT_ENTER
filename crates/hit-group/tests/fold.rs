//! The fold and the permission answers, adversarially. No network, no
//! database: a group is a `Vec<Event>`.
//!
//! The refusal tests are HIT_ENTER_2's (`he2-space/tests/permissions.rs`)
//! that apply to what M3 has — kick, roles, invites, channel view and post,
//! the owner. Mute, ban, slowmode, moderator deletes and role editing come
//! with M5, and their tests with them. The rest are new: things only a
//! replicated state has.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use hit_event::{
    GroupKey, Person, Verified,
    group::{
        self, Channel, ChannelKind, DEFAULT_ROLE, Override, RoleDef, Snapshot, create_group,
        create_invite, join, leave, message, put_user, remove_user,
    },
};
use hit_group::{GroupState, Groups, Judgement, Permissions, Refusal};
use nostr::prelude::{Event, EventBuilder, PublicKey, Timestamp};

const T0: u64 = 1_800_000_000;

/// A group under test: its key, its owner, a few people, and every event
/// so far.
struct G {
    key: GroupKey,
    owner: Person,
    events: Vec<Event>,
    clock: u64,
}

fn role(id: &str, perms: Permissions, position: i64) -> RoleDef {
    RoleDef {
        id: id.into(),
        name: id.into(),
        perms: perms.bits(),
        position,
    }
}

impl G {
    /// The ladder HIT_ENTER_2 seeds: member, moderator (10), admin (20);
    /// channels `general` and a `mods` channel denied to plain members.
    fn new() -> Self {
        Self::with_roles(vec![
            role(DEFAULT_ROLE, Permissions::DEFAULT_MEMBER, 0),
            role("mod", Permissions::DEFAULT_MODERATOR, 10),
            role("admin", Permissions::DEFAULT_ADMIN, 20),
        ])
    }

    fn with_roles(roles: Vec<RoleDef>) -> Self {
        let key = GroupKey::generate();
        let owner = Person::generate();
        let snap = Snapshot {
            seq: 0,
            name: "Test".into(),
            public: false,
            channels: vec![
                Channel {
                    id: "general".into(),
                    name: "general".into(),
                    kind: ChannelKind::Text,
                    slowmode: 0,
                },
                Channel {
                    id: "mods".into(),
                    name: "mods".into(),
                    kind: ChannelKind::Text,
                    slowmode: 0,
                },
            ],
            overrides: vec![
                Override {
                    channel: "mods".into(),
                    role: DEFAULT_ROLE.into(),
                    allow: 0,
                    deny: Permissions::VIEW_CHANNEL.bits(),
                },
                Override {
                    channel: "mods".into(),
                    role: "mod".into(),
                    allow: Permissions::VIEW_CHANNEL.bits(),
                    deny: 0,
                },
            ],
            roles,
            owner: Some(owner.public_key()),
            members: BTreeMap::from([(owner.public_key(), BTreeSet::new())]),
            bans: BTreeSet::new(),
            mutes: BTreeMap::new(),
        };
        let mut events: Vec<Event> = snap
            .sign(&key, Timestamp::from(T0))
            .unwrap()
            .into_iter()
            .collect();
        events.push(
            owner
                .sign(create_group(&key.public_key()).custom_created_at(Timestamp::from(T0)))
                .unwrap(),
        );
        Self {
            key,
            owner,
            events,
            clock: T0,
        }
    }

    fn id(&self) -> PublicKey {
        self.key.public_key()
    }

    /// Signs `b` as `who`, one second after the last event, and keeps it.
    fn act(&mut self, who: &Person, b: EventBuilder) -> Event {
        self.clock += 1;
        let ev = who
            .sign(b.custom_created_at(Timestamp::from(self.clock)))
            .unwrap();
        self.events.push(ev.clone());
        ev
    }

    fn state(&self) -> GroupState {
        GroupState::fold(self.id(), &self.events).unwrap()
    }

    /// Did the fold apply `ev`?
    fn applied(&self, ev: &Event) -> bool {
        self.state().applied(&ev.id)
    }

    /// A new person, made a member by the owner, with `roles`.
    fn member(&mut self, roles: &[&str]) -> Person {
        let p = Person::generate();
        let owner = Person::from_nsec(&self.owner.reveal_nsec_for_backup()).unwrap();
        let ev = self.act(&owner, put_user(&self.id(), &p.public_key(), roles));
        assert!(self.applied(&ev), "the owner could not add a member");
        p
    }
}

// --- who holds what ----------------------------------------------------

#[test]
fn the_owner_holds_everything_without_holding_a_role() {
    let g = G::new();
    let s = g.state();
    assert_eq!(s.owner(), Some(g.owner.public_key()));
    assert_eq!(s.base(&g.owner.public_key()), Permissions::ALL);
    assert!(s.roles_of(&g.owner.public_key()).unwrap().is_empty());
}

#[test]
fn a_plain_member_holds_the_default_role_without_being_assigned_it() {
    let mut g = G::new();
    let m = g.member(&[]);
    assert_eq!(g.state().base(&m.public_key()), Permissions::DEFAULT_MEMBER);
}

#[test]
fn a_stranger_holds_nothing_anywhere() {
    let g = G::new();
    let s = g.state();
    let x = Person::generate().public_key();
    assert_eq!(s.base(&x), Permissions::NONE);
    assert_eq!(s.effective(&x, "general"), Permissions::NONE);
}

// --- kick (9001) and the rank rule ------------------------------------

#[test]
fn a_plain_member_cannot_kick() {
    let mut g = G::new();
    let (a, b) = (g.member(&[]), g.member(&[]));
    let ev = g.act(&a, remove_user(&g.id(), &b.public_key()));
    assert!(!g.applied(&ev));
    assert!(g.state().is_member(&b.public_key()));
}

#[test]
fn a_moderator_can_kick_a_plain_member() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let ev = g.act(&m, remove_user(&g.id(), &p.public_key()));
    assert!(g.applied(&ev));
    assert!(!g.state().is_member(&p.public_key()));
}

#[test]
fn a_moderator_cannot_kick_another_moderator() {
    let mut g = G::new();
    let (m1, m2) = (g.member(&["mod"]), g.member(&["mod"]));
    let ev = g.act(&m1, remove_user(&g.id(), &m2.public_key()));
    assert!(!g.applied(&ev));
}

#[test]
fn a_moderator_cannot_kick_an_admin_but_an_admin_can_kick_a_moderator() {
    let mut g = G::new();
    let (m, a) = (g.member(&["mod"]), g.member(&["admin"]));
    let up = g.act(&m, remove_user(&g.id(), &a.public_key()));
    assert!(!g.applied(&up));
    let down = g.act(&a, remove_user(&g.id(), &m.public_key()));
    assert!(g.applied(&down));
}

#[test]
fn nobody_can_kick_the_owner_including_the_owner() {
    let mut g = G::new();
    let a = g.member(&["admin"]);
    let owner_pk = g.owner.public_key();
    let by_admin = g.act(&a, remove_user(&g.id(), &owner_pk));
    let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
    let by_self = g.act(&owner, remove_user(&g.id(), &owner_pk));
    let leaving = g.act(&owner, leave(&g.id()));
    for ev in [&by_admin, &by_self, &leaving] {
        assert!(!g.applied(ev));
    }
    assert!(g.state().is_member(&owner_pk));
}

/// The race the rank rule exists for: two equal admins remove each other
/// "at once". Neither half is valid, in either order, on every peer.
#[test]
fn two_equal_rank_admins_cannot_remove_each_other() {
    let mut g = G::new();
    let (a, b) = (g.member(&["admin"]), g.member(&["admin"]));
    let ab = g.act(&a, remove_user(&g.id(), &b.public_key()));
    let ba = g.act(&b, remove_user(&g.id(), &a.public_key()));
    let s = g.state();
    assert!(!s.applied(&ab.id) && !s.applied(&ba.id));
    assert!(s.is_member(&a.public_key()) && s.is_member(&b.public_key()));
}

// --- roles (9000) and the escalation rule -----------------------------

#[test]
fn a_plain_member_cannot_touch_roles() {
    let mut g = G::new();
    let (a, b) = (g.member(&[]), g.member(&[]));
    let ev = g.act(&a, put_user(&g.id(), &b.public_key(), &["mod"]));
    assert!(!g.applied(&ev));
    let selfie = g.act(&a, put_user(&g.id(), &a.public_key(), &["mod"]));
    assert!(!g.applied(&selfie));
}

#[test]
fn a_moderator_cannot_touch_roles() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let ev = g.act(&m, put_user(&g.id(), &p.public_key(), &["mod"]));
    assert!(!g.applied(&ev));
}

#[test]
fn an_admin_can_promote_a_plain_member_to_moderator() {
    let mut g = G::new();
    let (a, p) = (g.member(&["admin"]), g.member(&[]));
    let ev = g.act(&a, put_user(&g.id(), &p.public_key(), &["mod"]));
    assert!(g.applied(&ev));
    assert_eq!(
        g.state().roles_of(&p.public_key()),
        Some(&BTreeSet::from(["mod".to_owned()]))
    );
}

#[test]
fn an_admin_cannot_grant_a_permission_they_do_not_hold() {
    // A low role that carries MANAGE_GROUP, which admins do not hold.
    let mut g = G::with_roles(vec![
        role(DEFAULT_ROLE, Permissions::DEFAULT_MEMBER, 0),
        role("admin", Permissions::DEFAULT_ADMIN, 20),
        role("renamer", Permissions::MANAGE_GROUP, 5),
    ]);
    let (a, p) = (g.member(&["admin"]), g.member(&[]));
    let ev = g.act(&a, put_user(&g.id(), &p.public_key(), &["renamer"]));
    assert!(!g.applied(&ev));
    assert!(g.state().roles_of(&p.public_key()).unwrap().is_empty());
}

#[test]
fn an_admin_cannot_assign_a_role_they_do_not_outrank() {
    let mut g = G::new();
    let (a, p) = (g.member(&["admin"]), g.member(&[]));
    let ev = g.act(&a, put_user(&g.id(), &p.public_key(), &["admin"]));
    assert!(!g.applied(&ev), "an admin made another admin");
}

#[test]
fn a_moderator_cannot_be_demoted_by_someone_who_does_not_outrank_them() {
    // An "assistant" role with MANAGE_ROLES but ranked below moderators.
    let mut g = G::with_roles(vec![
        role(DEFAULT_ROLE, Permissions::DEFAULT_MEMBER, 0),
        role("assistant", Permissions::DEFAULT_ADMIN, 5),
        role("mod", Permissions::DEFAULT_MODERATOR, 10),
    ]);
    let (asst, m) = (g.member(&["assistant"]), g.member(&["mod"]));
    let ev = g.act(&asst, put_user(&g.id(), &m.public_key(), &[]));
    assert!(!g.applied(&ev));
    assert!(g.state().roles_of(&m.public_key()).unwrap().contains("mod"));
}

#[test]
fn assigning_an_unknown_or_reserved_role_is_refused() {
    let mut g = G::new();
    let p = g.member(&[]);
    let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
    let unknown = g.act(&owner, put_user(&g.id(), &p.public_key(), &["wizard"]));
    let reserved = g.act(&owner, put_user(&g.id(), &p.public_key(), &["owner"]));
    assert!(!g.applied(&unknown) && !g.applied(&reserved));
}

// --- channels: view and post ------------------------------------------

#[test]
fn a_member_cannot_post_to_a_channel_they_cannot_view() {
    let mut g = G::new();
    let p = g.member(&[]);
    let s = g.state();
    let ev = p.sign(message(&g.id(), "mods", "hello?")).unwrap();
    assert!(matches!(s.judge_message(&ev), Err(Refusal::Lacks(_))));
    assert!(!s.may_read(&p.public_key(), &ev, Timestamp::from(T0)));
    let ok = p.sign(message(&g.id(), "general", "hi")).unwrap();
    assert_eq!(s.judge_message(&ok), Ok(()));
}

#[test]
fn an_allow_override_beats_a_deny_so_a_private_channel_can_be_opened_to_one_role() {
    let mut g = G::new();
    let m = g.member(&["mod"]);
    let plain = g.member(&[]);
    let s = g.state();
    assert!(
        !s.effective(&plain.public_key(), "mods")
            .contains(Permissions::VIEW_CHANNEL)
    );
    // The default role's deny and the mod role's allow both apply; allow wins.
    assert!(
        s.effective(&m.public_key(), "mods")
            .contains(Permissions::VIEW_CHANNEL)
    );
    assert!(!s.everyone_can_view("mods"));
    assert!(s.everyone_can_view("general"));
}

#[test]
fn a_channel_override_cannot_shut_the_owner_out() {
    let g = G::new();
    let s = g.state();
    assert_eq!(s.effective(&g.owner.public_key(), "mods"), Permissions::ALL);
}

#[test]
fn a_message_to_a_channel_that_does_not_exist_is_refused() {
    let mut g = G::new();
    let p = g.member(&[]);
    let ev = p.sign(message(&g.id(), "nowhere", "x")).unwrap();
    assert_eq!(g.state().judge_message(&ev), Err(Refusal::NoSuchChannel));
}

// --- invites (9009) and joins (9021) ----------------------------------

#[test]
fn a_join_with_a_valid_invite_admits_with_nobody_else_online() {
    let mut g = G::new();
    let inviter = g.member(&[]);
    g.act(&inviter, create_invite(&g.id(), "c0de", None));
    let newcomer = Person::generate();
    let ev = g.act(&newcomer, join(&g.id(), "c0de"));
    assert!(g.applied(&ev));
    assert!(g.state().is_member(&newcomer.public_key()));
}

#[test]
fn a_join_is_refused_with_a_wrong_code_an_expired_invite_or_a_departed_inviter() {
    let mut g = G::new();
    let (inviter, mod_) = (g.member(&[]), g.member(&["mod"]));
    g.act(&inviter, create_invite(&g.id(), "gone", None));
    let expires = Timestamp::from(g.clock + 2);
    g.act(&inviter, create_invite(&g.id(), "soon", Some(expires)));

    let wrong = g.act(&Person::generate(), join(&g.id(), "nope"));
    g.clock += 5; // past `soon`'s expiry
    let late = g.act(&Person::generate(), join(&g.id(), "soon"));
    g.act(&mod_, remove_user(&g.id(), &inviter.public_key()));
    let orphan = g.act(&Person::generate(), join(&g.id(), "gone"));
    for ev in [&wrong, &late, &orphan] {
        assert!(!g.applied(ev));
    }
}

#[test]
fn a_member_who_may_not_invite_cannot_mint_a_working_invite() {
    let mut g = G::with_roles(vec![role(
        DEFAULT_ROLE,
        Permissions::DEFAULT_MEMBER.without(Permissions::CREATE_INVITE),
        0,
    )]);
    let p = g.member(&[]);
    g.act(&p, create_invite(&g.id(), "mine", None));
    let ev = g.act(&Person::generate(), join(&g.id(), "mine"));
    assert!(!g.applied(&ev));
}

#[test]
fn joining_twice_is_not_an_action() {
    let mut g = G::new();
    let p = g.member(&[]);
    let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
    g.act(&owner, create_invite(&g.id(), "x", None));
    let again = g.act(&p, join(&g.id(), "x"));
    assert!(!g.applied(&again));
}

// --- the fold itself ---------------------------------------------------

/// Timestamps are whole seconds. A join and the join of its inviter in the
/// same second must not depend on which event id sorts first.
#[test]
fn a_chain_of_joins_in_one_second_does_not_depend_on_ids() {
    for _ in 0..20 {
        let mut g = G::new();
        let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
        g.act(&owner, create_invite(&g.id(), "first", None));
        let (b, c) = (Person::generate(), Person::generate());
        let at = Timestamp::from(g.clock + 1);
        let same = |p: &Person, eb: EventBuilder| p.sign(eb.custom_created_at(at)).unwrap();
        let jb = same(&b, join(&g.id(), "first"));
        let inv = same(&b, create_invite(&g.id(), "second", None));
        let jc = same(&c, join(&g.id(), "second"));
        g.events.extend([jc.clone(), inv, jb.clone()]);
        let s = g.state();
        assert!(s.applied(&jb.id) && s.applied(&jc.id));
    }
}

/// …but across seconds the order is strict: a join refused because its
/// author was already a member does not come back to life after a later
/// kick.
#[test]
fn an_old_refused_action_does_not_apply_after_a_later_change() {
    let mut g = G::new();
    let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
    g.act(&owner, create_invite(&g.id(), "k", None));
    let x = Person::generate();
    g.act(&x, join(&g.id(), "k"));
    let again = g.act(&x, join(&g.id(), "k")); // refused: already a member
    let kick = g.act(&owner, remove_user(&g.id(), &x.public_key()));
    let s = g.state();
    assert!(s.applied(&kick.id));
    assert!(!s.applied(&again.id));
    assert!(!s.is_member(&x.public_key()));
}

#[test]
fn the_fold_does_not_depend_on_the_order_events_arrive_in() {
    let mut g = G::new();
    let (a, m) = (g.member(&["admin"]), g.member(&[]));
    g.act(&a, put_user(&g.id(), &m.public_key(), &["mod"]));
    g.act(&a, create_invite(&g.id(), "k", None));
    let n = Person::generate();
    g.act(&n, join(&g.id(), "k"));
    g.act(&m, remove_user(&g.id(), &n.public_key()));
    let forward = g.state();
    let mut reversed = g.events.clone();
    reversed.reverse();
    let back = GroupState::fold(g.id(), &reversed).unwrap();
    let members = |s: &GroupState| s.members().copied().collect::<Vec<_>>();
    assert_eq!(members(&forward), members(&back));
    assert!(!forward.is_member(&n.public_key()));
    for p in members(&forward) {
        assert_eq!(forward.roles_of(&p), back.roles_of(&p));
    }
}

#[test]
fn without_all_four_state_events_there_is_no_state() {
    let g = G::new();
    let partial: Vec<Event> = g
        .events
        .iter()
        .filter(|e| e.kind != group::GROUP_ROLES)
        .cloned()
        .collect();
    assert!(GroupState::fold(g.id(), &partial).is_none());
}

#[test]
fn state_signed_by_another_key_is_ignored() {
    let mut g = G::new();
    let forger = GroupKey::generate();
    let me = Person::generate();
    // The forger's snapshot claims `me` is a member, at a far higher seq.
    let forged = Snapshot {
        seq: 99,
        members: BTreeMap::from([(me.public_key(), BTreeSet::new())]),
        ..Default::default()
    }
    .sign(&forger, Timestamp::from(T0 + 50))
    .unwrap();
    // Re-point each at the victim group: the signature then cannot match.
    for ev in forged {
        let mut json: serde_json::Value = serde_json::to_value(&ev).unwrap();
        json["tags"][0][1] = serde_json::Value::String(g.id().to_hex());
        let ev: Event = serde_json::from_value(json).unwrap();
        g.events.push(ev);
    }
    assert!(!g.state().is_member(&me.public_key()));
}

#[test]
fn the_group_key_cannot_make_someone_owner_without_their_9007() {
    let mut g = G::new();
    g.events.retain(|e| e.kind != group::CREATE_GROUP);
    let s = g.state();
    assert_eq!(s.owner(), None);
    assert_eq!(s.named_owner(), Some(g.owner.public_key()));
    // Still a member — the snapshot lists them — but only a plain one.
    assert_eq!(s.base(&g.owner.public_key()), Permissions::DEFAULT_MEMBER);
}

#[test]
fn compaction_keeps_the_state_and_folds_the_actions() {
    let mut g = G::new();
    let (a, p) = (g.member(&["admin"]), g.member(&[]));
    let promote = g.act(&a, put_user(&g.id(), &p.public_key(), &["mod"]));
    let before = g.state();

    // The owner compacts: a new snapshot at seq + 1, dated after everything.
    g.clock += 1;
    let snap = before.to_snapshot();
    assert_eq!(snap.seq, 1);
    g.events
        .extend(snap.sign(&g.key, Timestamp::from(g.clock)).unwrap());
    let after = g.state();
    assert_eq!(after.seq(), 1);
    let members = |s: &GroupState| s.members().copied().collect::<Vec<_>>();
    assert_eq!(members(&before), members(&after));
    assert_eq!(
        after.roles_of(&p.public_key()),
        before.roles_of(&p.public_key())
    );
    // The promotion is folded: not applied again, not synced.
    assert!(!after.applied(&promote.id));
    assert!(after.is_folded(&promote, Timestamp::from(g.clock)));
    assert!(!after.may_read(&p.public_key(), &promote, Timestamp::from(g.clock)));
}

/// The first VPS anchor (2026-10-24): it pinned a group after the invites
/// four members had joined with expired, so it could never apply their
/// joins — and refused everything they wrote as "not a member". An expired
/// invite is needed until a snapshot folds the joins it honoured.
#[test]
fn an_expired_invite_is_kept_and_synced_until_a_snapshot_passes_its_expiry() {
    let mut g = G::new();
    let expires = Timestamp::from(g.clock + 2);
    let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
    let invite = g.act(&owner, create_invite(&g.id(), "soon", Some(expires)));
    let newcomer = Person::generate();
    let joined = g.act(&newcomer, join(&g.id(), "soon"));
    assert!(g.applied(&joined));

    // Long after the expiry, a node that arrives late still takes the
    // invite, and members still offer it.
    let later = Timestamp::from(g.clock + 3600);
    let mut late = Groups::new(Person::generate().public_key());
    late.carry(g.id());
    late.set_state(g.id(), Some(g.state()));
    let v = |ev: &Event| Verified::check(ev.clone()).unwrap();
    assert_eq!(late.judge(&v(&invite), later), Ok(Judgement::Store));
    assert!(g.state().may_read(&owner.public_key(), &invite, later));

    // Once a snapshot is dated after the expiry, the invite is folded with
    // the join it honoured: the newcomer is in the snapshot's member list.
    g.clock += 3600;
    g.events.extend(
        g.state()
            .to_snapshot()
            .sign(&g.key, Timestamp::from(g.clock))
            .unwrap(),
    );
    let after = g.state();
    assert!(after.is_folded(&invite, later));
    assert!(after.is_member(&newcomer.public_key()));
    late.set_state(g.id(), Some(after));
    assert_eq!(late.judge(&v(&invite), later), Err(Refusal::Folded));
}

/// M13: who may read and join is the owner's call alone — not even a role
/// holding every permission makes a group public, though it may rename it
/// — and it survives compaction.
#[test]
fn only_the_owner_makes_a_group_public_and_a_snapshot_keeps_it() {
    let mut g = G::with_roles(vec![
        role(DEFAULT_ROLE, Permissions::DEFAULT_MEMBER, 0),
        role("all", Permissions::ALL, 30),
    ]);
    assert!(!g.state().is_public(), "private by default");
    let a = g.member(&["all"]);
    let by_admin = g.act(&a, edit_metadata(&g.id(), &[Edit::Public(true)]));
    assert!(!g.applied(&by_admin));
    let renamed = g.act(&a, edit_metadata(&g.id(), &[Edit::Name("Renamed".into())]));
    assert!(g.applied(&renamed), "the control: they may edit the group");
    assert!(!g.state().is_public());

    let owner = Person::from_nsec(&g.owner.reveal_nsec_for_backup()).unwrap();
    let by_owner = g.act(&owner, edit_metadata(&g.id(), &[Edit::Public(true)]));
    assert!(g.applied(&by_owner));
    assert!(g.state().is_public());

    g.clock += 1;
    let snap = g.state().to_snapshot();
    g.events
        .extend(snap.sign(&g.key, Timestamp::from(g.clock)).unwrap());
    assert!(g.state().is_public(), "folded into the snapshot");

    let back = g.act(&owner, edit_metadata(&g.id(), &[Edit::Public(false)]));
    assert!(g.applied(&back));
    assert!(!g.state().is_public());
}

/// "The owner has the last word": a snapshot that leaves out an action's
/// effect overrules it, because the action is folded.
#[test]
fn a_snapshot_overrules_an_action_it_left_out() {
    let mut g = G::new();
    let a = g.member(&["admin"]);
    let p = g.member(&[]);
    g.act(&a, put_user(&g.id(), &p.public_key(), &["mod"]));
    let mut snap = g.state().to_snapshot();
    snap.members.insert(p.public_key(), BTreeSet::new()); // owner disagrees
    g.clock += 1;
    g.events
        .extend(snap.sign(&g.key, Timestamp::from(g.clock)).unwrap());
    assert!(g.state().roles_of(&p.public_key()).unwrap().is_empty());
}

// --- what a node asks: Groups ------------------------------------------

#[test]
fn a_node_stores_only_what_its_groups_allow_and_serves_only_members() {
    let mut g = G::new();
    let (m, outsider) = (g.member(&[]), Person::generate());
    let now = Timestamp::from(g.clock + 10);
    let mut groups = Groups::new(m.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));

    let v = |ev: Event| Verified::check(ev).unwrap();
    let mine = v(m.sign(message(&g.id(), "general", "hi")).unwrap());
    let theirs = v(outsider.sign(message(&g.id(), "general", "hi")).unwrap());
    let other_group = v(m
        .sign(message(&GroupKey::generate().public_key(), "general", "x"))
        .unwrap());
    let a_dm = v(m.sign(EventBuilder::new(4u16.into(), "x")).unwrap());
    assert_eq!(groups.judge(&mine, now), Ok(Judgement::Store));
    assert_eq!(groups.judge(&theirs, now), Err(Refusal::NotAMember));
    assert_eq!(
        groups.judge(&other_group, now),
        Err(Refusal::GroupNotCarried)
    );
    assert_eq!(groups.judge(&a_dm, now), Err(Refusal::KindNotCarried(4)));

    // Reading: the member sees the message, the outsider sees nothing —
    // not the message, not the state.
    assert!(groups.may_read(&m.public_key(), &mine, now));
    assert!(!groups.may_read(&outsider.public_key(), &mine, now));
    for ev in &g.events {
        assert!(!groups.may_read(&outsider.public_key(), ev, now));
    }
    assert_eq!(groups.peers(), BTreeSet::from([g.owner.public_key()]));
}

#[test]
fn a_profile_is_kept_and_shown_only_among_people_who_share_a_group() {
    let mut g = G::new();
    let (m, outsider) = (g.member(&[]), Person::generate());
    let now = Timestamp::from(g.clock + 10);
    let mut groups = Groups::new(m.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));

    let profile = |p: &Person| {
        Verified::check(p.sign(hit_event::profile::with_name(None, "x")).unwrap()).unwrap()
    };
    let (mine, owners, theirs) = (profile(&m), profile(&g.owner), profile(&outsider));
    assert_eq!(groups.judge(&mine, now), Ok(Judgement::Store));
    assert_eq!(groups.judge(&owners, now), Ok(Judgement::Store));
    assert_eq!(groups.judge(&theirs, now), Err(Refusal::NotAMember));
    // The owner's profile is readable by a fellow member, not by a stranger;
    // and nobody's profile travels on a group's gossip.
    assert!(groups.may_read(&m.public_key(), &owners, now));
    assert!(!groups.may_read(&outsider.public_key(), &owners, now));
    assert!(groups.may_read(&outsider.public_key(), &theirs, now));
    assert_eq!(groups.gossip_group(&owners), None);
}

#[test]
fn private_channel_messages_never_go_over_the_groups_gossip() {
    let mut g = G::new();
    let m = g.member(&["mod"]);
    g.member(&[]);
    let mut groups = Groups::new(m.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));
    let public = m.sign(message(&g.id(), "general", "a")).unwrap();
    let private = m.sign(message(&g.id(), "mods", "b")).unwrap();
    assert_eq!(groups.gossip_group(&public), Some(g.id()));
    assert_eq!(groups.gossip_group(&private), None);
}

#[test]
fn an_event_from_the_future_is_refused() {
    let mut g = G::new();
    let m = g.member(&[]);
    let mut groups = Groups::new(m.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));
    let now = Timestamp::from(T0);
    let ev = Verified::check(
        m.sign(message(&g.id(), "general", "x").custom_created_at(now + 3_600))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(groups.judge(&ev, now), Err(Refusal::FromTheFuture));
}

#[test]
fn a_group_without_state_accepts_only_the_group_keys_own_events() {
    let g = G::new();
    let me = Person::generate();
    let mut groups = Groups::new(me.public_key());
    groups.carry(g.id());
    let now = Timestamp::from(T0 + 1);
    let state_ev = Verified::check(g.events[0].clone()).unwrap();
    assert_eq!(groups.judge(&state_ev, now), Ok(Judgement::Store));
    let msg = Verified::check(g.owner.sign(message(&g.id(), "general", "x")).unwrap()).unwrap();
    assert_eq!(groups.judge(&msg, now), Err(Refusal::NoState));
}

// --- reactions, deletions, presence (M4) --------------------------------

/// A group with a moderator and a plain member, as `Groups` for the plain
/// member, at a moment after everything.
fn m4_setup() -> (G, Person, Person, Groups, Timestamp) {
    let mut g = G::new();
    let moderator = g.member(&["mod"]);
    let plain = g.member(&[]);
    let mut groups = Groups::new(plain.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));
    let now = Timestamp::from(g.clock + 10);
    (g, moderator, plain, groups, now)
}

fn signed(p: &Person, b: EventBuilder, at: Timestamp) -> Verified {
    Verified::check(p.sign(b.custom_created_at(at)).unwrap()).unwrap()
}

#[test]
fn a_reaction_is_judged_like_a_message_and_must_point_at_one_thing() {
    let (g, moderator, plain, groups, now) = m4_setup();
    let msg = moderator.sign(message(&g.id(), "general", "hi")).unwrap();
    let mods_msg = moderator.sign(message(&g.id(), "mods", "secret")).unwrap();
    let react = |p: &Person, channel: &str, target: &Event, emoji: &str| {
        signed(
            p,
            group::reaction(&g.id(), channel, &target.id, &target.pubkey, emoji),
            now,
        )
    };

    assert_eq!(
        groups.judge(&react(&plain, "general", &msg, "👍"), now),
        Ok(Judgement::Store)
    );
    // In a channel the reactor cannot see: refused, and unreadable by them.
    let in_mods = react(&moderator, "mods", &mods_msg, "👀");
    assert_eq!(groups.judge(&in_mods, now), Ok(Judgement::Store));
    assert_eq!(
        groups.judge(&react(&plain, "mods", &mods_msg, "👀"), now),
        Err(Refusal::Lacks(
            Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES)
        ))
    );
    assert!(!groups.may_read(&plain.public_key(), &in_mods, now));
    assert!(groups.may_read(&moderator.public_key(), &in_mods, now));
    // A stranger, an empty reaction, an essay.
    assert_eq!(
        groups.judge(&react(&Person::generate(), "general", &msg, "👍"), now),
        Err(Refusal::NotAMember)
    );
    assert_eq!(
        groups.judge(&react(&plain, "general", &msg, ""), now),
        Err(Refusal::Malformed)
    );
    assert_eq!(
        groups.judge(&react(&plain, "general", &msg, &"x".repeat(33)), now),
        Err(Refusal::Malformed)
    );
    // Two targets in one reaction.
    let two = group::reaction(&g.id(), "general", &msg.id, &msg.pubkey, "👍")
        .tag(nostr::prelude::Tag::custom("e", [mods_msg.id.to_hex()]));
    assert_eq!(
        groups.judge(&signed(&plain, two, now), now),
        Err(Refusal::Malformed)
    );
    // Public channel: on gossip. Private: not.
    assert_eq!(groups.gossip_group(&msg), Some(g.id()));
    assert_eq!(groups.gossip_group(&in_mods), None);
}

#[test]
fn a_deletion_needs_a_target_and_a_channel_its_author_can_see() {
    let (g, moderator, plain, groups, now) = m4_setup();
    let msg = plain.sign(message(&g.id(), "general", "oops")).unwrap();
    let del = |p: &Person, channel: &str, targets: &[nostr::prelude::EventId]| {
        signed(p, group::deletion(&g.id(), channel, targets), now)
    };
    assert_eq!(
        groups.judge(&del(&plain, "general", &[msg.id]), now),
        Ok(Judgement::Store)
    );
    assert_eq!(
        groups.judge(&del(&plain, "general", &[]), now),
        Err(Refusal::Malformed)
    );
    assert_eq!(
        groups.judge(&del(&plain, "mods", &[msg.id]), now),
        Err(Refusal::Lacks(Permissions::VIEW_CHANNEL))
    );
    assert_eq!(
        groups.judge(&del(&moderator, "mods", &[msg.id]), now),
        Ok(Judgement::Store)
    );
}

#[test]
fn presence_is_for_members_only_fresh_only_and_goes_on_gossip() {
    let (g, _moderator, plain, groups, now) = m4_setup();
    let here = signed(&plain, group::presence(&g.id()), now);
    assert_eq!(groups.judge(&here, now), Ok(Judgement::Store));
    assert_eq!(groups.gossip_group(&here), Some(g.id()));
    let old = signed(
        &plain,
        group::presence(&g.id()),
        now - (hit_group::PRESENCE_MAX_AGE + 1),
    );
    assert_eq!(groups.judge(&old, now), Err(Refusal::Stale));
    let stranger = signed(&Person::generate(), group::presence(&g.id()), now);
    assert_eq!(groups.judge(&stranger, now), Err(Refusal::NotAMember));
}

// ========================================================================
// M5 — moderation without a server
//
// HIT_ENTER_2's refusal tests for ban, mute, slowmode, moderator deletes,
// channels, overrides and role editing, re-asked of the fold; then what only
// a mesh has: two equal admins banning each other at once, a ban arriving
// after the banned user's next message, a snapshot racing an action.
// ========================================================================

use hit_event::group::{Edit, ban_user, delete_event, edit_metadata, mute_user, unban_user};

impl G {
    fn owner(&self) -> Person {
        Person::from_nsec(&self.owner.reveal_nsec_for_backup()).unwrap()
    }

    /// An action by the owner.
    fn by_owner(&mut self, b: EventBuilder) -> Event {
        let o = self.owner();
        self.act(&o, b)
    }

    /// A person in the group with a fresh invite of the owner's, joined.
    fn joins(&mut self, p: &Person) -> Event {
        let code = format!("c{}", self.clock);
        self.by_owner(create_invite(&self.id(), &code, None));
        self.act(p, join(&self.id(), &code))
    }

    /// A message by `p` at the group's next second, and whether the state
    /// at that moment takes it.
    fn says(&mut self, p: &Person, channel: &str) -> Result<(), Refusal> {
        self.clock += 1;
        let ev = p
            .sign(message(&self.id(), channel, "x").custom_created_at(Timestamp::from(self.clock)))
            .unwrap();
        self.state().judge_message(&ev)
    }

    fn at(&self) -> Timestamp {
        Timestamp::from(self.clock)
    }
}

fn channel(id: &str, slowmode: u32) -> Channel {
    Channel {
        id: id.into(),
        name: id.into(),
        kind: ChannelKind::Text,
        slowmode,
    }
}

fn over(channel: &str, role: &str, allow: Permissions, deny: Permissions) -> Edit {
    Edit::Override(Override {
        channel: channel.into(),
        role: role.into(),
        allow: allow.bits(),
        deny: deny.bits(),
    })
}

// --- bans --------------------------------------------------------------

#[test]
fn a_plain_member_cannot_ban() {
    let mut g = G::new();
    let (a, b) = (g.member(&[]), g.member(&[]));
    let ev = g.act(&a, ban_user(&g.id(), &b.public_key()));
    assert!(!g.applied(&ev));
    assert!(g.state().is_member(&b.public_key()));
}

#[test]
fn a_moderator_can_ban_a_plain_member_but_not_another_moderator() {
    let mut g = G::new();
    let (m1, m2, p) = (g.member(&["mod"]), g.member(&["mod"]), g.member(&[]));
    let peer = g.act(&m1, ban_user(&g.id(), &m2.public_key()));
    assert!(!g.applied(&peer), "a moderator banned an equal");
    let ev = g.act(&m1, ban_user(&g.id(), &p.public_key()));
    assert!(g.applied(&ev));
    let s = g.state();
    assert!(!s.is_member(&p.public_key()));
    assert!(s.is_banned(&p.public_key()));
}

#[test]
fn nobody_can_ban_the_owner_including_the_owner() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let owner = g.owner.public_key();
    let by_admin = g.act(&admin, ban_user(&g.id(), &owner));
    let by_self = g.by_owner(ban_user(&g.id(), &owner));
    assert!(!g.applied(&by_admin));
    assert!(!g.applied(&by_self));
    assert!(!g.state().is_banned(&owner));
}

#[test]
fn a_banned_person_cannot_rejoin_with_a_valid_invite_until_unbanned() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    g.act(&m, ban_user(&g.id(), &p.public_key()));
    let back = g.joins(&p);
    assert!(!g.applied(&back), "a banned person walked back in");
    let unban = g.act(&m, unban_user(&g.id(), &p.public_key()));
    assert!(g.applied(&unban));
    assert!(
        !g.state().is_member(&p.public_key()),
        "an unban adds nobody"
    );
    let again = g.joins(&p);
    assert!(g.applied(&again));
}

#[test]
fn a_kick_is_not_a_ban() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let kick = g.act(&m, remove_user(&g.id(), &p.public_key()));
    assert!(g.applied(&kick));
    assert!(!g.state().is_banned(&p.public_key()));
    let back = g.joins(&p);
    assert!(g.applied(&back));
}

#[test]
fn someone_who_left_can_still_be_banned_and_stays_out() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    g.act(&p, leave(&g.id()));
    let ban = g.act(&m, ban_user(&g.id(), &p.public_key()));
    assert!(g.applied(&ban));
    let back = g.joins(&p);
    assert!(!g.applied(&back));
    // Kicking someone who is not here is nothing, and refused as such.
    let kick = g.act(&m, remove_user(&g.id(), &p.public_key()));
    assert!(!g.applied(&kick));
}

#[test]
fn unbanning_needs_ban_members_and_someone_banned() {
    let mut g = G::new();
    let (m, p, q) = (g.member(&["mod"]), g.member(&[]), g.member(&[]));
    g.act(&m, ban_user(&g.id(), &p.public_key()));
    let by_plain = g.act(&q, unban_user(&g.id(), &p.public_key()));
    assert!(!g.applied(&by_plain));
    let nobody = g.act(&m, unban_user(&g.id(), &q.public_key()));
    assert!(!g.applied(&nobody), "unbanned someone who was never banned");
}

#[test]
fn an_admin_cannot_put_a_banned_person_back_without_unbanning_first() {
    let mut g = G::new();
    let (admin, p) = (g.member(&["admin"]), g.member(&[]));
    g.act(&admin, ban_user(&g.id(), &p.public_key()));
    let put = g.act(&admin, put_user(&g.id(), &p.public_key(), &[]));
    assert!(!g.applied(&put));
    let by_owner = g.by_owner(put_user(&g.id(), &p.public_key(), &[]));
    assert!(!g.applied(&by_owner), "not even the owner skips the unban");
}

// --- mutes -------------------------------------------------------------

#[test]
fn a_plain_member_cannot_mute() {
    let mut g = G::new();
    let (a, b) = (g.member(&[]), g.member(&[]));
    let until = g.at() + 600;
    let ev = g.act(&a, mute_user(&g.id(), &b.public_key(), Some(until)));
    assert!(!g.applied(&ev));
    assert_eq!(g.says(&b, "general"), Ok(()));
}

#[test]
fn a_moderator_can_mute_and_unmute_a_plain_member() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let until = g.at() + 600;
    let mute = g.act(&m, mute_user(&g.id(), &p.public_key(), Some(until)));
    assert!(g.applied(&mute));
    assert_eq!(g.state().muted_until(&p.public_key(), g.at()), Some(until));
    assert_eq!(g.says(&p, "general"), Err(Refusal::Muted));
    let lift = g.act(&m, mute_user(&g.id(), &p.public_key(), None));
    assert!(g.applied(&lift));
    assert_eq!(g.says(&p, "general"), Ok(()));
}

#[test]
fn a_moderator_cannot_mute_an_admin_or_the_owner() {
    let mut g = G::new();
    let (m, admin) = (g.member(&["mod"]), g.member(&["admin"]));
    let until = g.at() + 600;
    let up = g.act(&m, mute_user(&g.id(), &admin.public_key(), Some(until)));
    assert!(!g.applied(&up));
    let owner = g.owner.public_key();
    let top = g.act(&admin, mute_user(&g.id(), &owner, Some(until)));
    assert!(!g.applied(&top));
}

#[test]
fn a_muted_member_cannot_post_and_can_again_once_it_expires() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let until = g.at() + 5;
    g.act(&m, mute_user(&g.id(), &p.public_key(), Some(until)));
    assert_eq!(g.says(&p, "general"), Err(Refusal::Muted));
    assert_eq!(
        g.says(&p, "mods"),
        Err(Refusal::Lacks(
            Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES)
        ))
    );
    g.clock = until.as_secs();
    assert_eq!(g.says(&p, "general"), Ok(()));
}

#[test]
fn a_mute_ending_before_it_starts_is_malformed_and_lifting_nothing_is_refused() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let past = g.at();
    let backwards = g.act(&m, mute_user(&g.id(), &p.public_key(), Some(past)));
    assert!(!g.applied(&backwards));
    let lift = g.act(&m, mute_user(&g.id(), &p.public_key(), None));
    assert!(!g.applied(&lift));
}

#[test]
fn a_muted_member_can_still_withdraw_their_own_words() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let mine = p.sign(message(&g.id(), "general", "regret")).unwrap();
    g.act(&m, mute_user(&g.id(), &p.public_key(), Some(g.at() + 600)));
    let mut groups = Groups::new(p.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));
    let now = Timestamp::from(g.clock + 1);
    let withdraw = Verified::check(
        p.sign(group::deletion(&g.id(), "general", &[mine.id]).custom_created_at(now))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(groups.judge(&withdraw, now), Ok(Judgement::Store));
}

// --- silences: what a peer drops when a removal reaches it late ---------

#[test]
fn a_ban_arriving_after_the_banned_users_next_message_silences_that_message() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    // P's message, signed a second after the ban, reached some peer first.
    let before = p
        .sign(message(&g.id(), "general", "before").custom_created_at(Timestamp::from(g.clock)))
        .unwrap();
    let ban = g.act(&m, ban_user(&g.id(), &p.public_key()));
    let after = p
        .sign(message(&g.id(), "general", "after").custom_created_at(ban.created_at + 1))
        .unwrap();
    let s = g.state();
    // The span says which of P's events the ban reaches: the one after it.
    assert!(s.silence_at(&p.public_key(), after.created_at).is_some());
    assert!(s.silence_at(&p.public_key(), before.created_at).is_none());
    // And a fresh judgement refuses both — P is not a member.
    assert_eq!(s.judge_message(&after), Err(Refusal::NotAMember));
}

#[test]
fn a_rejoin_ends_the_removal_span_and_only_the_gap_stays_silent() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let kick = g.act(&m, remove_user(&g.id(), &p.public_key()));
    g.clock += 10;
    let rejoin = g.joins(&p);
    assert!(g.applied(&rejoin));
    let s = g.state();
    let me = p.public_key();
    assert!(s.silence_at(&me, kick.created_at + 1).is_some(), "the gap");
    assert!(s.silence_at(&me, rejoin.created_at).is_none(), "back in");
    // A message backdated into the gap is refused even now they are back.
    let backdated = p
        .sign(message(&g.id(), "general", "x").custom_created_at(kick.created_at + 2))
        .unwrap();
    assert_eq!(s.judge_message(&backdated), Err(Refusal::NotAMember));
    assert_eq!(g.says(&p, "general"), Ok(()));
}

#[test]
fn a_mute_silences_only_its_own_span() {
    let mut g = G::new();
    let (m, p) = (g.member(&["mod"]), g.member(&[]));
    let until = g.at() + 30;
    let mute = g.act(&m, mute_user(&g.id(), &p.public_key(), Some(until)));
    let s = g.state();
    let me = p.public_key();
    assert!(s.silence_at(&me, mute.created_at - 1).is_none());
    assert!(s.silence_at(&me, mute.created_at).is_some_and(|x| x.mute));
    assert!(s.silence_at(&me, until).is_none());
}

#[test]
fn two_equal_rank_admins_banning_each_other_at_once_both_fail() {
    let mut g = G::new();
    let (a, b) = (g.member(&["admin"]), g.member(&["admin"]));
    // The same second, so the fold has no order between them.
    g.clock += 1;
    let at = Timestamp::from(g.clock);
    let ab = a
        .sign(ban_user(&g.id(), &b.public_key()).custom_created_at(at))
        .unwrap();
    let ba = b
        .sign(ban_user(&g.id(), &a.public_key()).custom_created_at(at))
        .unwrap();
    g.events.extend([ab.clone(), ba.clone()]);
    // The rank rule leaves the race no valid half: neither applies, on
    // every peer, whatever order they arrived in.
    let s = g.state();
    assert!(!s.applied(&ab.id) && !s.applied(&ba.id));
    assert!(s.is_member(&a.public_key()) && s.is_member(&b.public_key()));
}

#[test]
fn an_admin_and_a_moderator_banning_each_other_at_once_leaves_the_admin() {
    let mut g = G::new();
    let (admin, m) = (g.member(&["admin"]), g.member(&["mod"]));
    g.clock += 1;
    let at = Timestamp::from(g.clock);
    let up = m
        .sign(ban_user(&g.id(), &admin.public_key()).custom_created_at(at))
        .unwrap();
    let down = admin
        .sign(ban_user(&g.id(), &m.public_key()).custom_created_at(at))
        .unwrap();
    for order in [[up.clone(), down.clone()], [down.clone(), up.clone()]] {
        let mut events = g.events.clone();
        events.extend(order);
        let s = GroupState::fold(g.id(), &events).unwrap();
        assert!(s.applied(&down.id) && !s.applied(&up.id));
        assert!(s.is_member(&admin.public_key()));
        assert!(s.is_banned(&m.public_key()));
    }
}

#[test]
fn a_snapshot_racing_an_action_folds_what_it_saw_and_loses_what_it_did_not() {
    let mut g = G::new();
    let (m, p, q) = (g.member(&["mod"]), g.member(&[]), g.member(&[]));
    // The moderator bans P; the owner, not having seen it, compacts a
    // second later from the events it holds.
    let ban_p = g.act(&m, ban_user(&g.id(), &p.public_key()));
    let unseen: Vec<Event> = g
        .events
        .iter()
        .filter(|e| e.id != ban_p.id)
        .cloned()
        .collect();
    let snap = GroupState::fold(g.id(), &unseen).unwrap().to_snapshot();
    g.clock += 1;
    let signed = snap.sign(&g.key, Timestamp::from(g.clock)).unwrap();
    g.events.extend(signed);
    // A ban after the snapshot applies on top of it.
    let ban_q = g.act(&m, ban_user(&g.id(), &q.public_key()));
    let s = g.state();
    assert_eq!(s.seq(), 1);
    assert!(s.is_banned(&q.public_key()) && s.applied(&ban_q.id));
    // The honest limit: the ban the compactor never saw, dated before its
    // snapshot, is lost — visibly, and it can be redone.
    assert!(!s.is_banned(&p.public_key()));
    assert!(s.is_member(&p.public_key()));
    let again = g.act(&m, ban_user(&g.id(), &p.public_key()));
    assert!(g.applied(&again));
}

#[test]
fn a_compaction_keeps_bans_mutes_and_slowmode() {
    let mut g = G::new();
    let (admin, p, q) = (g.member(&["admin"]), g.member(&[]), g.member(&[]));
    g.act(&admin, ban_user(&g.id(), &p.public_key()));
    let until = g.at() + 3_600;
    g.act(&admin, mute_user(&g.id(), &q.public_key(), Some(until)));
    g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::Channel(channel("general", 30))]),
    );
    g.clock += 1;
    let snap = g.state().to_snapshot();
    g.events = snap
        .sign(&g.key, Timestamp::from(g.clock))
        .unwrap()
        .into_iter()
        .chain(
            g.events
                .iter()
                .filter(|e| e.kind == group::CREATE_GROUP)
                .cloned(),
        )
        .collect();
    let s = g.state();
    assert!(s.is_banned(&p.public_key()));
    assert_eq!(s.muted_until(&q.public_key(), g.at()), Some(until));
    assert_eq!(s.slowmode("general"), 30);
    // And the folded mute still silences, from the snapshot on.
    assert_eq!(g.says(&q, "general"), Err(Refusal::Muted));
    let back = g.joins(&p);
    assert!(!g.applied(&back));
}

// --- moderator deletes (9005) ------------------------------------------

fn groups_for(g: &G, me: &Person) -> Groups {
    let mut groups = Groups::new(me.public_key());
    groups.carry(g.id());
    groups.set_state(g.id(), Some(g.state()));
    groups
}

fn removal(g: &G, by: &Person, channel: &str, target: &Event) -> Verified {
    Verified::check(
        by.sign(
            delete_event(&g.id(), channel, &[target.id])
                .custom_created_at(Timestamp::from(g.clock + 1)),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_plain_member_cannot_remove_somebody_elses_message() {
    let mut g = G::new();
    let (a, b) = (g.member(&[]), g.member(&[]));
    let msg = b.sign(message(&g.id(), "general", "mine")).unwrap();
    let groups = groups_for(&g, &a);
    let now = Timestamp::from(g.clock + 1);
    assert_eq!(
        groups.judge(&removal(&g, &a, "general", &msg), now),
        Err(Refusal::Lacks(
            Permissions::VIEW_CHANNEL.union(Permissions::MANAGE_MESSAGES)
        ))
    );
}

#[test]
fn a_moderator_can_remove_anyones_message_including_a_higher_ranks() {
    let mut g = G::new();
    let (m, admin) = (g.member(&["mod"]), g.member(&["admin"]));
    let msg = admin.sign(message(&g.id(), "general", "hm")).unwrap();
    let groups = groups_for(&g, &m);
    let now = Timestamp::from(g.clock + 1);
    // HIT_ENTER_2's rule: the channel's MANAGE_MESSAGES, no rank.
    assert_eq!(
        groups.judge(&removal(&g, &m, "general", &msg), now),
        Ok(Judgement::Store)
    );
    // And a removal in public channel goes on gossip like a message.
    assert_eq!(
        groups.gossip_group(&removal(&g, &m, "general", &msg)),
        Some(g.id())
    );
}

#[test]
fn a_moderator_denied_manage_messages_in_one_channel_cannot_remove_there() {
    let mut g = G::new();
    let m = g.member(&["mod"]);
    let deny = g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "general",
            "mod",
            Permissions::NONE,
            Permissions::MANAGE_MESSAGES,
        )],
    ));
    assert!(g.applied(&deny));
    let msg = g.owner.sign(message(&g.id(), "general", "x")).unwrap();
    let in_mods = g.owner.sign(message(&g.id(), "mods", "y")).unwrap();
    let groups = groups_for(&g, &m);
    let now = Timestamp::from(g.clock + 1);
    assert!(
        groups
            .judge(&removal(&g, &m, "general", &msg), now)
            .is_err()
    );
    assert_eq!(
        groups.judge(&removal(&g, &m, "mods", &in_mods), now),
        Ok(Judgement::Store)
    );
}

#[test]
fn a_removal_without_targets_is_malformed() {
    let mut g = G::new();
    let m = g.member(&["mod"]);
    let groups = groups_for(&g, &m);
    let now = Timestamp::from(g.clock + 1);
    let empty = Verified::check(m.sign(delete_event(&g.id(), "general", &[])).unwrap()).unwrap();
    assert_eq!(groups.judge(&empty, now), Err(Refusal::Malformed));
}

// --- channels and slowmode (9002) ---------------------------------------

#[test]
fn a_plain_member_cannot_create_or_delete_a_channel() {
    let mut g = G::new();
    let p = g.member(&[]);
    let make = g.act(
        &p,
        edit_metadata(&g.id(), &[Edit::Channel(channel("x", 0))]),
    );
    let drop = g.act(
        &p,
        edit_metadata(&g.id(), &[Edit::ChannelDelete("general".into())]),
    );
    assert!(!g.applied(&make) && !g.applied(&drop));
}

#[test]
fn a_moderator_cannot_manage_channels_but_an_admin_can() {
    let mut g = G::new();
    let (m, admin) = (g.member(&["mod"]), g.member(&["admin"]));
    let by_mod = g.act(
        &m,
        edit_metadata(&g.id(), &[Edit::Channel(channel("x", 0))]),
    );
    assert!(!g.applied(&by_mod));
    let by_admin = g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::Channel(channel("x", 0))]),
    );
    assert!(g.applied(&by_admin));
    assert!(g.state().channels().iter().any(|c| c.id == "x"));
    let drop = g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::ChannelDelete("x".into())]),
    );
    assert!(g.applied(&drop));
    assert!(!g.state().channels().iter().any(|c| c.id == "x"));
}

#[test]
fn deleting_a_channel_takes_its_overrides_and_its_messages_have_nowhere_to_go() {
    let mut g = G::new();
    let p = g.member(&[]);
    g.by_owner(edit_metadata(
        &g.id(),
        &[Edit::ChannelDelete("mods".into())],
    ));
    let s = g.state();
    assert!(s.overrides().iter().all(|o| o.channel != "mods"));
    assert_eq!(g.says(&p, "mods"), Err(Refusal::NoSuchChannel));
}

#[test]
fn deleting_a_channel_that_does_not_exist_is_refused() {
    let mut g = G::new();
    let ev = g.by_owner(edit_metadata(
        &g.id(),
        &[Edit::ChannelDelete("nope".into())],
    ));
    assert!(!g.applied(&ev));
}

#[test]
fn a_plain_member_cannot_set_slowmode_and_an_admin_can_within_the_limit() {
    let mut g = G::new();
    let (p, admin) = (g.member(&[]), g.member(&["admin"]));
    let by_plain = g.act(
        &p,
        edit_metadata(&g.id(), &[Edit::Channel(channel("general", 10))]),
    );
    assert!(!g.applied(&by_plain));
    let too_slow = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[Edit::Channel(channel("general", group::MAX_SLOWMODE + 1))],
        ),
    );
    assert!(!g.applied(&too_slow));
    let ok = g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::Channel(channel("general", 10))]),
    );
    assert!(g.applied(&ok));
    assert_eq!(g.state().slowmode("general"), 10);
    assert_eq!(g.state().slowmode("mods"), 0);
}

#[test]
fn only_the_owner_renames_the_group() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let by_admin = g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::Name("Mine now".into())]),
    );
    assert!(!g.applied(&by_admin));
    let by_owner = g.by_owner(edit_metadata(&g.id(), &[Edit::Name("  Renamed  ".into())]));
    assert!(g.applied(&by_owner));
    assert_eq!(g.state().name(), "Renamed");
    let empty = g.by_owner(edit_metadata(&g.id(), &[Edit::Name("   ".into())]));
    assert!(!g.applied(&empty));
    let long = g.by_owner(edit_metadata(&g.id(), &[Edit::Name("x".repeat(65))]));
    assert!(!g.applied(&long));
}

#[test]
fn an_edit_batch_is_all_or_nothing() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    // A channel the admin may make, and a rename they may not.
    let ev = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[Edit::Channel(channel("new", 0)), Edit::Name("Nope".into())],
        ),
    );
    assert!(!g.applied(&ev));
    let s = g.state();
    assert!(!s.channels().iter().any(|c| c.id == "new"));
    assert_eq!(s.name(), "Test");
}

// --- channel overrides --------------------------------------------------

#[test]
fn a_plain_member_cannot_set_a_channel_override() {
    let mut g = G::new();
    let p = g.member(&[]);
    let ev = g.act(
        &p,
        edit_metadata(
            &g.id(),
            &[over(
                "mods",
                DEFAULT_ROLE,
                Permissions::VIEW_CHANNEL,
                Permissions::NONE,
            )],
        ),
    );
    assert!(!g.applied(&ev));
    assert!(
        !g.state()
            .effective(&p.public_key(), "mods")
            .contains(Permissions::VIEW_CHANNEL)
    );
}

#[test]
fn an_override_cannot_be_used_to_grant_what_the_actor_lacks() {
    // A keeper may manage roles but not messages.
    let keeper_perms = Permissions::DEFAULT_MEMBER.union(Permissions::MANAGE_ROLES);
    let mut g = G::with_roles(vec![
        role(DEFAULT_ROLE, Permissions::DEFAULT_MEMBER, 0),
        role("keeper", keeper_perms, 20),
    ]);
    let k = g.member(&["keeper"]);
    let grant = g.act(
        &k,
        edit_metadata(
            &g.id(),
            &[over(
                "general",
                DEFAULT_ROLE,
                Permissions::MANAGE_MESSAGES,
                Permissions::NONE,
            )],
        ),
    );
    assert!(!g.applied(&grant));
    // Denying is within their reach.
    let deny = g.act(
        &k,
        edit_metadata(
            &g.id(),
            &[over(
                "general",
                DEFAULT_ROLE,
                Permissions::NONE,
                Permissions::ATTACH_FILES,
            )],
        ),
    );
    assert!(g.applied(&deny));
}

#[test]
fn an_override_on_a_role_at_or_above_the_actor_is_refused() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let ev = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[over(
                "general",
                "admin",
                Permissions::NONE,
                Permissions::SEND_MESSAGES,
            )],
        ),
    );
    assert!(!g.applied(&ev));
}

#[test]
fn only_channel_scoped_bits_survive_an_override() {
    let mut g = G::new();
    let p = g.member(&[]);
    g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "general",
            DEFAULT_ROLE,
            Permissions::KICK_MEMBERS.union(Permissions::MANAGE_MESSAGES),
            Permissions::NONE,
        )],
    ));
    let s = g.state();
    let mine = s.effective(&p.public_key(), "general");
    assert!(mine.contains(Permissions::MANAGE_MESSAGES));
    assert!(
        !mine.contains(Permissions::KICK_MEMBERS),
        "a channel granted a group power"
    );
}

#[test]
fn an_empty_override_is_deleted_rather_than_stored() {
    let mut g = G::new();
    let before = g.state().overrides().len();
    g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "mods",
            DEFAULT_ROLE,
            Permissions::NONE,
            Permissions::NONE,
        )],
    ));
    let s = g.state();
    assert_eq!(s.overrides().len(), before - 1);
    // Group-only bits alone scope down to nothing, and are deleted too.
    g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "mods",
            "mod",
            Permissions::KICK_MEMBERS,
            Permissions::NONE,
        )],
    ));
    assert!(g.state().overrides().iter().all(|o| o.channel != "mods"));
}

#[test]
fn a_bit_in_both_allow_and_deny_resolves_to_allow() {
    let mut g = G::new();
    let p = g.member(&[]);
    g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "general",
            DEFAULT_ROLE,
            Permissions::SEND_MESSAGES,
            Permissions::SEND_MESSAGES,
        )],
    ));
    assert_eq!(g.says(&p, "general"), Ok(()));
}

#[test]
fn an_override_needs_a_channel_and_a_role_that_exist() {
    let mut g = G::new();
    let no_channel = g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "nowhere",
            DEFAULT_ROLE,
            Permissions::NONE,
            Permissions::SEND_MESSAGES,
        )],
    ));
    let no_role = g.by_owner(edit_metadata(
        &g.id(),
        &[over(
            "general",
            "ghost",
            Permissions::NONE,
            Permissions::SEND_MESSAGES,
        )],
    ));
    assert!(!g.applied(&no_channel) && !g.applied(&no_role));
}

// --- role definitions -----------------------------------------------------

fn role_edit(id: &str, perms: Permissions, position: i64) -> Edit {
    Edit::Role(role(id, perms, position))
}

#[test]
fn a_plain_member_and_a_moderator_cannot_define_roles() {
    let mut g = G::new();
    let (p, m) = (g.member(&[]), g.member(&["mod"]));
    for who in [&p, &m] {
        let ev = g.act(
            who,
            edit_metadata(
                &g.id(),
                &[role_edit("helper", Permissions::DEFAULT_MEMBER, 5)],
            ),
        );
        assert!(!g.applied(&ev));
    }
}

#[test]
fn an_admin_can_create_a_role_below_themselves() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let ev = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[role_edit("helper", Permissions::DEFAULT_MEMBER, 5)],
        ),
    );
    assert!(g.applied(&ev));
    assert!(g.state().roles().any(|r| r.id == "helper"));
}

#[test]
fn an_admin_cannot_create_a_role_at_or_above_themselves() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    for position in [20, 25] {
        let ev = g.act(
            &admin,
            edit_metadata(
                &g.id(),
                &[role_edit("peer", Permissions::DEFAULT_MEMBER, position)],
            ),
        );
        assert!(!g.applied(&ev), "made a role at position {position}");
    }
}

#[test]
fn an_admin_cannot_edit_the_admin_role_they_hold() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let ev = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[role_edit("admin", Permissions::DEFAULT_ADMIN, 15)],
        ),
    );
    assert!(!g.applied(&ev));
}

#[test]
fn an_admin_cannot_create_a_role_with_a_permission_they_lack() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let ev = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[role_edit(
                "renamer",
                Permissions::DEFAULT_MEMBER.union(Permissions::MANAGE_GROUP),
                5,
            )],
        ),
    );
    assert!(!g.applied(&ev));
}

#[test]
fn an_admin_cannot_move_a_role_from_below_to_above_themselves() {
    let mut g = G::new();
    let admin = g.member(&["admin"]);
    let ev = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[role_edit("mod", Permissions::DEFAULT_MODERATOR, 30)],
        ),
    );
    assert!(!g.applied(&ev));
}

#[test]
fn only_the_owner_may_edit_the_default_role_and_it_stays_at_the_bottom() {
    let mut g = G::new();
    let (admin, p) = (g.member(&["admin"]), g.member(&[]));
    let by_admin = g.act(
        &admin,
        edit_metadata(
            &g.id(),
            &[role_edit(DEFAULT_ROLE, Permissions::VIEW_CHANNEL, 0)],
        ),
    );
    assert!(!g.applied(&by_admin));
    let raised = g.by_owner(edit_metadata(
        &g.id(),
        &[role_edit(DEFAULT_ROLE, Permissions::DEFAULT_MEMBER, 3)],
    ));
    assert!(!g.applied(&raised));
    let read_only = g.by_owner(edit_metadata(
        &g.id(),
        &[role_edit(DEFAULT_ROLE, Permissions::VIEW_CHANNEL, 0)],
    ));
    assert!(g.applied(&read_only));
    assert_eq!(
        g.says(&p, "general"),
        Err(Refusal::Lacks(
            Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES)
        ))
    );
}

#[test]
fn the_default_role_cannot_be_deleted_not_even_by_the_owner() {
    let mut g = G::new();
    let ev = g.by_owner(edit_metadata(
        &g.id(),
        &[Edit::RoleDelete(DEFAULT_ROLE.into())],
    ));
    assert!(!g.applied(&ev));
}

#[test]
fn the_owner_mark_is_not_a_role_anyone_may_define() {
    let mut g = G::new();
    let ev = g.by_owner(edit_metadata(
        &g.id(),
        &[role_edit("owner", Permissions::ALL, 99)],
    ));
    assert!(!g.applied(&ev));
}

#[test]
fn a_deleted_role_takes_its_assignments_and_overrides_with_it() {
    let mut g = G::new();
    let m = g.member(&["mod"]);
    let ev = g.by_owner(edit_metadata(&g.id(), &[Edit::RoleDelete("mod".into())]));
    assert!(g.applied(&ev));
    let s = g.state();
    assert!(s.roles_of(&m.public_key()).is_some_and(BTreeSet::is_empty));
    assert!(s.overrides().iter().all(|o| o.role != "mod"));
    assert!(!s.base(&m.public_key()).contains(Permissions::KICK_MEMBERS));
}

#[test]
fn deleting_a_role_needs_to_outrank_it() {
    let mut g = G::new();
    let (m, admin) = (g.member(&["mod"]), g.member(&["admin"]));
    let own = g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::RoleDelete("admin".into())]),
    );
    assert!(!g.applied(&own));
    let below = g.act(
        &admin,
        edit_metadata(&g.id(), &[Edit::RoleDelete("mod".into())]),
    );
    assert!(g.applied(&below));
    assert!(
        !g.state()
            .base(&m.public_key())
            .contains(Permissions::KICK_MEMBERS)
    );
}

#[test]
fn the_owner_keeps_every_power_even_with_the_default_role_stripped() {
    let mut g = G::new();
    g.by_owner(edit_metadata(
        &g.id(),
        &[role_edit(DEFAULT_ROLE, Permissions::NONE, 0)],
    ));
    let o = g.owner();
    assert_eq!(g.says(&o, "general"), Ok(()));
    assert_eq!(g.state().base(&o.public_key()), Permissions::ALL);
}

#[test]
fn bits_from_a_newer_build_are_dropped_from_a_role_edit() {
    let mut g = G::new();
    let ev = g.by_owner(edit_metadata(
        &g.id(),
        &[Edit::Role(RoleDef {
            id: "future".into(),
            name: "future".into(),
            perms: Permissions::DEFAULT_MEMBER.bits() | (1 << 60),
            position: 5,
        })],
    ));
    assert!(g.applied(&ev));
    let r = g
        .state()
        .roles()
        .find(|r| r.id == "future")
        .cloned()
        .unwrap();
    assert_eq!(r.perms, Permissions::DEFAULT_MEMBER.bits());
}

#[test]
fn a_ban_in_the_same_second_as_the_join_silences_whichever_applies_first() {
    let mut g = G::new();
    let m = g.member(&["mod"]);
    let p = Person::generate();
    let code = "same-second";
    g.by_owner(create_invite(&g.id(), code, None));
    g.clock += 1;
    let at = Timestamp::from(g.clock);
    let joined = p.sign(join(&g.id(), code).custom_created_at(at)).unwrap();
    let banned = m
        .sign(ban_user(&g.id(), &p.public_key()).custom_created_at(at))
        .unwrap();
    g.events.extend([joined, banned]);
    let s = g.state();
    assert!(s.is_banned(&p.public_key()));
    assert!(!s.is_member(&p.public_key()));
    // Whatever the id order did, what P says from that second on is silenced.
    assert!(s.silence_at(&p.public_key(), at).is_some());
    assert!(s.silence_at(&p.public_key(), at + 5).is_some());
}

#[test]
fn a_removed_person_may_read_the_removal_and_nothing_else() {
    let mut g = G::new();
    let (m, p, q) = (g.member(&["mod"]), g.member(&[]), g.member(&[]));
    let said = q.sign(message(&g.id(), "general", "after")).unwrap();
    let ban = g.act(&m, ban_user(&g.id(), &p.public_key()));
    let kick_q = g.act(&m, remove_user(&g.id(), &q.public_key()));
    let s = g.state();
    let now = Timestamp::from(g.clock + 1);
    assert!(s.may_read(&p.public_key(), &ban, now));
    assert!(
        !s.may_read(&p.public_key(), &kick_q, now),
        "somebody else's removal"
    );
    assert!(!s.may_read(&p.public_key(), &said, now));
    for ev in g.events.iter().filter(|e| e.id != ban.id) {
        assert!(!s.may_read(&p.public_key(), ev, now));
    }
    // And a stranger reads none of it.
    let stranger = Person::generate().public_key();
    assert!(!s.may_read(&stranger, &ban, now));
}

// --- DMs (M7) ---------------------------------------------------------------

#[test]
fn a_dm_is_kept_by_its_recipient_and_their_named_anchors_and_read_by_nobody_else() {
    let (alice, bob, anchor, other) = (
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
    );
    let (to_bob, to_alice) = alice.wrap_dm(&bob.public_key(), "hi", None).unwrap();
    let now = Timestamp::now();
    let v = |e: &Event| Verified::check(e.clone()).unwrap();

    // Bob's node keeps the one to Bob, not the one to Alice.
    let at_bob = Groups::new(bob.public_key());
    assert_eq!(at_bob.judge(&v(&to_bob), now), Ok(Judgement::Store));
    assert_eq!(at_bob.judge(&v(&to_alice), now), Err(Refusal::NotOurs));

    // An anchor keeps it only once Bob's inbox list names it.
    let mut at_anchor = Groups::new(anchor.public_key());
    assert_eq!(at_anchor.judge(&v(&to_bob), now), Err(Refusal::NotOurs));
    at_anchor.set_inboxes(bob.public_key(), BTreeSet::from([anchor.public_key()]));
    assert_eq!(at_anchor.judge(&v(&to_bob), now), Ok(Judgement::Store));
    assert_eq!(
        at_anchor
            .served_by(&anchor.public_key())
            .collect::<Vec<_>>(),
        vec![&bob.public_key()]
    );

    // Reading: Bob and his anchor; not the sender's wrap, not a stranger.
    assert!(at_anchor.may_read(&bob.public_key(), &to_bob, now));
    assert!(at_anchor.may_read(&anchor.public_key(), &to_bob, now));
    assert!(!at_anchor.may_read(&other.public_key(), &to_bob, now));
    assert!(!at_anchor.may_read(&alice.public_key(), &to_bob, now));
    assert_eq!(
        at_anchor.gossip_group(&to_bob),
        None,
        "never on a group's gossip"
    );

    // Taking the anchor off the list stops it keeping new ones.
    at_anchor.set_inboxes(bob.public_key(), BTreeSet::new());
    assert_eq!(at_anchor.judge(&v(&to_bob), now), Err(Refusal::NotOurs));
}

// --- files (M8) -------------------------------------------------------------

#[test]
fn a_message_with_a_malformed_file_is_refused_and_a_good_one_is_judged_as_a_message() {
    let mut g = G::new();
    let p = g.member(&[]);
    let s = g.state();
    let good = group::Attachment {
        hash: "c".repeat(64),
        sha256: "d".repeat(64),
        size: 10,
        mime: "text/plain".into(),
        name: "notes.txt".into(),
    };
    let ok = p
        .sign(group::message_with(
            &g.id(),
            "general",
            "",
            std::slice::from_ref(&good),
        ))
        .unwrap();
    assert_eq!(s.judge_message(&ok), Ok(()));
    let bad = group::Attachment {
        sha256: "short".into(),
        ..good.clone()
    };
    let refused = p
        .sign(group::message_with(&g.id(), "general", "", &[bad]))
        .unwrap();
    assert_eq!(s.judge_message(&refused), Err(Refusal::Malformed));
    // In a channel they cannot see: refused as any message would be.
    let hidden = p
        .sign(group::message_with(&g.id(), "mods", "", &[good]))
        .unwrap();
    assert!(s.judge_message(&hidden).is_err());
}
