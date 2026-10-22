//! Permissions: what a member is allowed to do, as a bitset.
//!
//! **Moved from HIT_ENTER_2** (`he2-proto/src/perm.rs`, the owner's code,
//! same licence; `docs/SESSION_LOG.md`). What changed: the bits are carried
//! in group-key-signed `39003` roles and `39000` overrides as decimal
//! strings instead of a `space.db` column, and every peer enforces them
//! instead of one host. The wire-stability rule is unchanged and matters
//! more: a renumbered bit would re-grant a power on every peer at once.
//!
//! ## The bits are wire-stable
//!
//! A bit position is part of the protocol. **Never renumber one, never reuse a
//! retired one.** A role's permissions are an integer in a signed event, so
//! renumbering would silently re-grant a different power to every role that
//! held the old one — the kind of change that looks like a refactor and reads,
//! afterwards, like a breach.
//!
//! ## Why a `u64` and not a set of strings
//!
//! One integer, one `&` to check, and a role's grant is atomic. The cost is
//! the 53-bit ceiling below — kept although our tags carry the number as a
//! string, because a client may still parse it into a JSON number.

/// What a member may do, as a set of bits.
///
/// See [`Permissions::MAX_BIT`] for the ceiling on its bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions(pub u64);

impl Permissions {
    /// Read a channel and its history.
    ///
    /// Denying this is how a private channel is built, and it is the one that
    /// has to be checked on *reads* as well as writes — backfill, resume and
    /// fan-out all consult it.
    pub const VIEW_CHANNEL: Self = Self(1 << 0);
    /// Post, edit your own, and delete your own.
    pub const SEND_MESSAGES: Self = Self(1 << 1);
    /// Delete **somebody else's** message. The moderator bit.
    pub const MANAGE_MESSAGES: Self = Self(1 << 2);
    /// Create, delete and rename channels, and set slowmode.
    pub const MANAGE_CHANNELS: Self = Self(1 << 3);
    /// Mint an invite link.
    pub const CREATE_INVITE: Self = Self(1 << 4);
    /// See and revoke invites minted by other people.
    ///
    /// Separate from [`Self::CREATE_INVITE`] because "may bring a friend" and
    /// "may audit how everyone else got in" are different amounts of trust.
    pub const MANAGE_INVITES: Self = Self(1 << 5);
    /// Remove a membership. They can come back with a fresh invite.
    pub const KICK_MEMBERS: Self = Self(1 << 6);
    /// Refuse the connection outright, until un-banned.
    pub const BAN_MEMBERS: Self = Self(1 << 7);
    /// Take away [`Self::SEND_MESSAGES`] for a while, without removing anyone.
    pub const MUTE_MEMBERS: Self = Self(1 << 8);
    /// Create roles, edit their bits, and assign them.
    ///
    /// The dangerous one: it is the bit that can grant every other bit. The
    /// fold guards it with an escalation check — see [`crate::state`] — so
    /// holding it does not mean holding everything.
    pub const MANAGE_ROLES: Self = Self(1 << 9);
    /// Rename the group and change its settings.
    pub const MANAGE_GROUP: Self = Self(1 << 10);
    /// Post a file alongside a message.
    ///
    /// Separate from [`Self::SEND_MESSAGES`] and channel-scoped, because
    /// "may talk here" and "may put a 50 MB file on every member's disk here" are
    /// different amounts of trust and often different *channels*: an
    /// announcements channel that accepts uploads from everybody is a way to
    /// fill every member's disk, and text is the thing a community actually needs
    /// everywhere.
    ///
    /// New at M7, at the next free bit. Nothing is grandfathered by adding
    /// it — nobody could attach a file before it existed — but the three
    /// seeded roles are updated by `0003_attachments.sql` so that a space
    /// created last week behaved like one created today (HIT_ENTER_2's history;
    /// here every group is born with the bit).
    pub const ATTACH_FILES: Self = Self(1 << 11);

    /// The highest bit any permission uses.
    ///
    /// **This must stay below 53.** These cross the wire as a JSON number, and
    /// IEEE-754 doubles — which is what every JSON parser that is not
    /// arbitrary-precision gives you — represent integers exactly only up to
    /// 2^53. A permission at bit 53 or above would round on the way through
    /// some client and grant or drop a power silently. The assertion below
    /// fails the build rather than letting that ship; if the bits ever run
    /// out, the fix is to send them as a decimal *string*, not to raise this.
    pub const MAX_BIT: u32 = 11;

    /// Everything. What the owner effectively holds, and the ceiling on any
    /// role.
    pub const ALL: Self = Self((1 << (Self::MAX_BIT + 1)) - 1);

    /// Nothing.
    pub const NONE: Self = Self(0);

    /// The permissions a per-channel override may allow or deny.
    ///
    /// The rest are group-wide: "may ban people" does not mean anything
    /// different in one channel than another, and letting it be overridden
    /// per channel would multiply the places a ban has to be checked without
    /// making any of them more expressive.
    pub const CHANNEL_SCOPED: Self = Self(
        Self::VIEW_CHANNEL.0
            | Self::SEND_MESSAGES.0
            | Self::MANAGE_MESSAGES.0
            | Self::ATTACH_FILES.0,
    );

    /// What a brand-new member gets: read, write, attach, and bring a friend.
    ///
    /// Deliberately includes [`Self::CREATE_INVITE`]. A public community that
    /// only the owner can invite anyone to does not grow, and the invite is
    /// already revocable, expiring and use-capped — the controls are on the
    /// link rather than on the permission.
    pub const DEFAULT_MEMBER: Self = Self(
        Self::VIEW_CHANNEL.0 | Self::SEND_MESSAGES.0 | Self::CREATE_INVITE.0 | Self::ATTACH_FILES.0,
    );

    /// The default moderator ladder rung: the day-to-day tools, and nothing
    /// that changes who holds power.
    pub const DEFAULT_MODERATOR: Self = Self(
        Self::DEFAULT_MEMBER.0
            | Self::MANAGE_MESSAGES.0
            | Self::KICK_MEMBERS.0
            | Self::BAN_MEMBERS.0
            | Self::MUTE_MEMBERS.0,
    );

    /// The default admin rung: everything a moderator has, plus the structure
    /// of the group. Not [`Self::MANAGE_GROUP`] — renaming the place people
    /// joined is the owner's.
    pub const DEFAULT_ADMIN: Self = Self(
        Self::DEFAULT_MODERATOR.0
            | Self::MANAGE_CHANNELS.0
            | Self::MANAGE_INVITES.0
            | Self::MANAGE_ROLES.0,
    );

    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Drops any bit this build does not define.
    ///
    /// Applied to everything read off the wire *and* everything read out of
    /// a stored event, because both can contain bits written by a newer build —
    /// a role signed by an owner running a later
    /// client. An undefined bit is not a permission this code can enforce, so
    /// carrying it around would mean storing a grant nothing honours.
    pub const fn sanitized(self) -> Self {
        Self(self.0 & Self::ALL.0)
    }

    /// True when **every** bit in `needed` is present.
    ///
    /// Note "every": callers pass a single permission almost always, but a
    /// compound check has to be an AND. An OR would let one of two required
    /// powers stand in for both.
    pub const fn contains(self, needed: Self) -> bool {
        (self.0 & needed.0) == needed.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every bit in `self` that is not in `other` — "what would this grant
    /// that that one does not".
    ///
    /// The escalation check is written in terms of this: an actor may not
    /// hand out what they do not hold.
    pub const fn beyond(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// Every permission, with the name it is spelled by on a command line and in
/// a UI.
///
/// **One canonical spelling, here**, so that `hit-cli`, the app and any log
/// line agree. A name is not a wire format — the bitset is — so these may be
/// renamed if a better word turns up, unlike the bit positions, which may
/// never move.
///
/// Ordered by bit, so a printed list reads the same way every time.
pub const NAMED: &[(&str, Permissions)] = &[
    ("view", Permissions::VIEW_CHANNEL),
    ("send", Permissions::SEND_MESSAGES),
    ("manage-messages", Permissions::MANAGE_MESSAGES),
    ("manage-channels", Permissions::MANAGE_CHANNELS),
    ("invite", Permissions::CREATE_INVITE),
    ("manage-invites", Permissions::MANAGE_INVITES),
    ("kick", Permissions::KICK_MEMBERS),
    ("ban", Permissions::BAN_MEMBERS),
    ("mute", Permissions::MUTE_MEMBERS),
    ("manage-roles", Permissions::MANAGE_ROLES),
    ("manage-group", Permissions::MANAGE_GROUP),
    ("attach", Permissions::ATTACH_FILES),
];

/// A name that is not a permission.
///
/// Carries the offending word so a caller can say which one, and nothing
/// else: the list of valid names is a constant anybody can print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPermission(pub String);

impl std::fmt::Display for UnknownPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown permission {:?}", self.0)
    }
}

impl std::error::Error for UnknownPermission {}

impl Permissions {
    /// The names of the bits that are set, in bit order.
    pub fn names(self) -> Vec<&'static str> {
        NAMED
            .iter()
            .filter(|(_, perm)| self.contains(*perm))
            .map(|(name, _)| *name)
            .collect()
    }

    /// Parses a comma-separated list of names.
    ///
    /// `all` and `none` are accepted as whole-list shorthands, and an empty
    /// string is `none` — which matters because "clear every permission" is a
    /// thing somebody types, and making them write a magic word for it is the
    /// kind of friction that ends in a bitset pasted from a log.
    pub fn parse_list(list: &str) -> Result<Self, UnknownPermission> {
        let mut out = Self::NONE;
        for word in list.split(',') {
            let word = word.trim();
            match word {
                "" => continue,
                "all" => out = out.union(Self::ALL),
                "none" => {}
                _ => match NAMED.iter().find(|(name, _)| *name == word) {
                    Some((_, perm)) => out = out.union(*perm),
                    None => return Err(UnknownPermission(word.to_owned())),
                },
            }
        }
        Ok(out)
    }
}

impl std::fmt::Display for Permissions {
    /// The names, comma-separated, or `none`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = self.names();
        if names.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&names.join(","))
        }
    }
}

// The 53-bit ceiling, enforced at compile time rather than remembered.
const _: () = assert!(
    Permissions::MAX_BIT < 53,
    "permission bits cross the wire as a JSON number and must stay exactly \
     representable as an f64; send them as a string before going past bit 52"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bit_positions_are_the_ones_that_shipped() {
        // A role's grant is stored as this integer, in every group's signed `39003` that
        // exists. Renumbering a bit would silently re-grant a different power
        // to every role that held the old one — the change that looks like a
        // refactor and reads, afterwards, like a breach. Adding one at the
        // next free position is the only safe edit, and this list is what
        // makes the difference visible in a diff.
        let expected: &[(u32, Permissions)] = &[
            (0, Permissions::VIEW_CHANNEL),
            (1, Permissions::SEND_MESSAGES),
            (2, Permissions::MANAGE_MESSAGES),
            (3, Permissions::MANAGE_CHANNELS),
            (4, Permissions::CREATE_INVITE),
            (5, Permissions::MANAGE_INVITES),
            (6, Permissions::KICK_MEMBERS),
            (7, Permissions::BAN_MEMBERS),
            (8, Permissions::MUTE_MEMBERS),
            (9, Permissions::MANAGE_ROLES),
            (10, Permissions::MANAGE_GROUP),
            (11, Permissions::ATTACH_FILES),
        ];
        for (bit, permission) in expected {
            assert_eq!(permission.bits(), 1 << bit, "bit {bit} moved");
        }
        assert_eq!(
            Permissions::MAX_BIT as usize,
            expected.len() - 1,
            "a permission was added or removed without updating this list"
        );
        assert_eq!(NAMED.len(), expected.len(), "every bit needs a name");
    }

    #[test]
    fn uploading_is_channel_scoped_and_talking_is_separate_from_it() {
        // The reason `ATTACH_FILES` is its own bit: "may talk here" and "may
        // put a 50 MB file on every member's disk here" are different amounts of
        // trust, and an announcements channel needs to be able to allow the
        // first and refuse the second.
        assert!(Permissions::CHANNEL_SCOPED.contains(Permissions::ATTACH_FILES));
        assert!(!Permissions::SEND_MESSAGES.contains(Permissions::ATTACH_FILES));
        assert!(Permissions::DEFAULT_MEMBER.contains(Permissions::ATTACH_FILES));
    }

    #[test]
    fn contains_requires_every_bit() {
        let both = Permissions::KICK_MEMBERS.union(Permissions::BAN_MEMBERS);
        assert!(both.contains(Permissions::KICK_MEMBERS));
        assert!(both.contains(both));
        // The one an OR would get wrong.
        assert!(!Permissions::KICK_MEMBERS.contains(both));
    }

    #[test]
    fn all_contains_every_named_permission() {
        for perm in [
            Permissions::VIEW_CHANNEL,
            Permissions::SEND_MESSAGES,
            Permissions::MANAGE_MESSAGES,
            Permissions::MANAGE_CHANNELS,
            Permissions::CREATE_INVITE,
            Permissions::MANAGE_INVITES,
            Permissions::KICK_MEMBERS,
            Permissions::BAN_MEMBERS,
            Permissions::MUTE_MEMBERS,
            Permissions::MANAGE_ROLES,
            Permissions::MANAGE_GROUP,
        ] {
            assert!(Permissions::ALL.contains(perm), "{perm:?} missing from ALL");
            assert_eq!(
                perm.sanitized(),
                perm,
                "{perm:?} does not survive a sanitize"
            );
        }
    }

    #[test]
    fn an_unknown_bit_from_a_newer_build_is_dropped() {
        let from_the_future = Permissions(Permissions::ALL.bits() | (1 << 40));
        assert_eq!(from_the_future.sanitized(), Permissions::ALL);
    }

    #[test]
    fn only_channel_scoped_permissions_are_overridable() {
        assert!(Permissions::CHANNEL_SCOPED.contains(Permissions::VIEW_CHANNEL));
        assert!(Permissions::CHANNEL_SCOPED.contains(Permissions::SEND_MESSAGES));
        assert!(Permissions::CHANNEL_SCOPED.contains(Permissions::MANAGE_MESSAGES));
        // Banning is not a per-channel idea.
        assert!(!Permissions::CHANNEL_SCOPED.contains(Permissions::BAN_MEMBERS));
        assert!(!Permissions::CHANNEL_SCOPED.contains(Permissions::MANAGE_ROLES));
    }

    #[test]
    fn the_default_ladder_climbs() {
        assert!(Permissions::DEFAULT_MODERATOR.contains(Permissions::DEFAULT_MEMBER));
        assert!(Permissions::DEFAULT_ADMIN.contains(Permissions::DEFAULT_MODERATOR));
        assert!(Permissions::ALL.contains(Permissions::DEFAULT_ADMIN));
    }

    /// The rungs are defaults, not the ladder itself — but a moderator who
    /// can edit roles is an admin, and an admin who can rename the group is
    /// the owner. Both would make the ladder meaningless.
    #[test]
    fn the_default_rungs_stop_where_they_should() {
        assert!(!Permissions::DEFAULT_MEMBER.contains(Permissions::MANAGE_MESSAGES));
        assert!(!Permissions::DEFAULT_MODERATOR.contains(Permissions::MANAGE_ROLES));
        assert!(!Permissions::DEFAULT_ADMIN.contains(Permissions::MANAGE_GROUP));
    }

    #[test]
    fn every_permission_has_exactly_one_name() {
        assert_eq!(
            NAMED.len() as u32,
            Permissions::MAX_BIT + 1,
            "a permission without a name cannot be typed or printed"
        );
        let mut seen = Permissions::NONE;
        for (_, perm) in NAMED {
            assert!(
                seen.intersection(*perm).is_empty(),
                "{perm:?} is named twice"
            );
            seen = seen.union(*perm);
        }
        assert_eq!(seen, Permissions::ALL, "NAMED must cover every bit");
    }

    #[test]
    fn names_round_trip_through_parsing() {
        for perms in [
            Permissions::NONE,
            Permissions::ALL,
            Permissions::DEFAULT_MEMBER,
            Permissions::DEFAULT_MODERATOR,
            Permissions::DEFAULT_ADMIN,
        ] {
            let printed = perms.to_string();
            assert_eq!(
                Permissions::parse_list(&printed),
                Ok(perms),
                "{printed} did not round trip"
            );
        }
    }

    #[test]
    fn the_shorthands_and_the_empty_string_work() {
        assert_eq!(Permissions::parse_list("all"), Ok(Permissions::ALL));
        assert_eq!(Permissions::parse_list("none"), Ok(Permissions::NONE));
        // "clear every permission" is a thing somebody types.
        assert_eq!(Permissions::parse_list(""), Ok(Permissions::NONE));
        assert_eq!(
            Permissions::parse_list(" kick , ban "),
            Ok(Permissions::KICK_MEMBERS.union(Permissions::BAN_MEMBERS)),
            "whitespace around a name is not an error"
        );
    }

    #[test]
    fn an_unknown_name_says_which_one() {
        assert_eq!(
            Permissions::parse_list("kick,delete-everything"),
            Err(UnknownPermission("delete-everything".into()))
        );
    }

    #[test]
    fn beyond_is_what_an_actor_would_be_handing_out() {
        let actor = Permissions::DEFAULT_MODERATOR;
        let granting = Permissions::MANAGE_ROLES.union(Permissions::KICK_MEMBERS);
        // Kick they already hold; manage-roles they do not.
        assert_eq!(granting.beyond(actor), Permissions::MANAGE_ROLES);
        assert!(Permissions::KICK_MEMBERS.beyond(actor).is_empty());
    }
}
