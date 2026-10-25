//! *Browse communities* (M13): search and rank the public groups announced
//! on public relays. Pure — the relays are asked elsewhere.
//!
//! **What a rank can trust.** An announcement is signed by its group's key,
//! so its *author* is certain; everything it says — name, about, member
//! count — is the group's own word. So:
//!
//! - **Size counts in its logarithm, and only up to a ceiling**: a group
//!   claiming a billion members ranks as one claiming [`MEMBER_CEILING`],
//!   and 10,000 beats 100 by 2×, not 100×. Lying buys little.
//! - **Freshness halves the score every [`HALF_LIFE`]**: a live group is
//!   re-announced at least daily by its owner's app, so a group nobody
//!   runs sinks, and after [`STALE_AFTER`] it is gone from the list.
//! - **A search ranks by where the words matched first** — the whole name,
//!   then a word of the name beginning with them, then anywhere in the
//!   name, then only in *about* — and by score within each. Every word of
//!   the query must match somewhere.
//!
//! What it does not do: verify a count, catch a group copying another's
//! name, or know what your friends joined. Said in `PLAN.md`.

use hit_event::announce::Announcement;
use nostr::prelude::Timestamp;

/// Claims above this many members rank as this many.
pub const MEMBER_CEILING: u64 = 100_000;
/// Every this many seconds since its announcement, a group's score halves.
pub const HALF_LIFE: u64 = 7 * 24 * 60 * 60;
/// Announced longer ago than this: not shown.
pub const STALE_AFTER: u64 = 30 * 24 * 60 * 60;

/// How well a query matched: higher is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Matched {
    About,
    InName,
    NameWord,
    Name,
}

/// `found`, searched for `query` (empty: everything) and ranked best first.
pub fn rank(found: Vec<Announcement>, query: &str, now: Timestamp) -> Vec<Announcement> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut scored: Vec<(Matched, f64, Announcement)> = found
        .into_iter()
        .filter(|a| now.as_secs().saturating_sub(a.at.as_secs()) <= STALE_AFTER)
        .filter_map(|a| {
            let m = matches(&a, query, &words)?;
            Some((m, score(&a, now), a))
        })
        .collect();
    scored.sort_by(|(ma, sa, a), (mb, sb, b)| {
        mb.cmp(ma)
            .then(sb.total_cmp(sa))
            .then(b.at.cmp(&a.at))
            .then(a.group.cmp(&b.group))
    });
    scored.into_iter().map(|(_, _, a)| a).collect()
}

/// Size in its logarithm, halved every [`HALF_LIFE`] of silence.
fn score(a: &Announcement, now: Timestamp) -> f64 {
    let members = a.members.min(MEMBER_CEILING) as f64;
    let age = now.as_secs().saturating_sub(a.at.as_secs()) as f64;
    (1.0 + members).ln() * 0.5_f64.powf(age / HALF_LIFE as f64)
}

/// Where `words` matched `a`, at best; `None` if one of them matched
/// nowhere. No words match everything, all alike.
fn matches(a: &Announcement, query: &str, words: &[String]) -> Option<Matched> {
    if words.is_empty() {
        return Some(Matched::About);
    }
    let name = a.name.to_lowercase();
    let about = a.about.to_lowercase();
    if !words.iter().all(|w| name.contains(w) || about.contains(w)) {
        return None;
    }
    let whole = query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    Some(if name == whole {
        Matched::Name
    } else if words.iter().all(|w| {
        name.split(|c: char| !c.is_alphanumeric())
            .any(|n| n.starts_with(w.as_str()))
    }) {
        Matched::NameWord
    } else if words.iter().all(|w| name.contains(w)) {
        Matched::InName
    } else {
        Matched::About
    })
}

#[cfg(test)]
mod tests {
    use hit_event::GroupKey;

    use super::*;

    const NOW: u64 = 1_800_000_000;
    const DAY: u64 = 24 * 60 * 60;

    fn a(name: &str, about: &str, members: u64, days_ago: u64) -> Announcement {
        Announcement {
            group: GroupKey::generate().public_key(),
            name: name.into(),
            about: about.into(),
            members,
            link: String::new(),
            at: Timestamp::from(NOW - days_ago * DAY),
        }
    }

    fn names(list: &[Announcement]) -> Vec<&str> {
        list.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn larger_and_fresher_first_and_the_stale_gone() {
        let list = vec![
            a("Small", "", 10, 0),
            a("Big", "", 5_000, 0),
            a("Big but silent", "", 5_000, 21),
            a("Dead", "", 50_000, 31),
        ];
        let ranked = rank(list, "", Timestamp::from(NOW));
        // Three weeks of silence cost a 5,000-member group more than a
        // live 10-member one.
        assert_eq!(names(&ranked), ["Big", "Small", "Big but silent"]);
    }

    #[test]
    fn a_lie_about_size_buys_little() {
        let list = vec![
            a("Honest", "", 5_000, 0),
            a("Liar", "", u64::MAX, 0),
            a("Liar, a month quiet", "", u64::MAX, 28),
        ];
        let ranked = rank(list, "", Timestamp::from(NOW));
        // The liar is first — but by the ceiling's log, not a billion to
        // one, and a fortnight's silence costs it more than the lie won.
        assert_eq!(names(&ranked)[0], "Liar");
        let honest = score(&a("Honest", "", 5_000, 0), Timestamp::from(NOW));
        let liar = score(&a("Liar", "", u64::MAX, 0), Timestamp::from(NOW));
        assert!(liar / honest < 1.4, "{liar} vs {honest}");
        let quiet = score(&a("q", "", u64::MAX, 14), Timestamp::from(NOW));
        assert!(quiet < honest);
    }

    #[test]
    fn a_search_ranks_the_name_before_the_about_and_needs_every_word() {
        let list = vec![
            a("Unturned", "the official community", 10, 0),
            a("Unturned Modding", "maps and mods", 500, 0),
            a("Survival games", "unturned, rust, dayz", 9_000, 0),
            a("Cooking", "recipes", 9_000, 0),
            a("Returned items", "", 9_000, 0),
        ];
        let now = Timestamp::from(NOW);
        let ranked = rank(list.clone(), "unturned", now);
        // The exact name, then a name starting with it, then the about.
        assert_eq!(
            names(&ranked),
            ["Unturned", "Unturned Modding", "Survival games"]
        );
        // Case and spacing do not matter; every word must match.
        assert_eq!(names(&rank(list.clone(), "  MOD  unturned ", now)), ["Unturned Modding"]);
        assert!(rank(list.clone(), "unturned cooking", now).is_empty());
        // A word inside the name ranks after a name word starting with it,
        // however much larger.
        let inside = vec![
            a("Commodity traders", "", 50_000, 0),
            a("Unturned Modding", "", 50, 0),
        ];
        assert_eq!(
            names(&rank(inside, "mod", now)),
            ["Unturned Modding", "Commodity traders"]
        );
    }
}
