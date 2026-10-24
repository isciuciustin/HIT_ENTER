//! M5's "done when" (`docs/PLAN.md`), automated on loopback:
//!
//! **a banned user's patched client posts into the group and no honest peer
//! stores or shows it.** The patched client is a node that signs and hands
//! events to peers without judging them first ("the CLI, lying").
//!
//! On the way, what only a mesh has: the banned user's next message reached
//! a peer *before* the ban did, and every peer drops it once the ban
//! arrives. Plus a moderator's removal that a peer still holding the
//! message cannot undo by pushing it back, and slowmode.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{eventually, loopback, loopback_addr, mesh, messages, start};
use hit_event::{
    MemoryStore, Person,
    group::{self, ChannelKind, Edit},
};
use hit_node::Node;
use nostr::prelude::{PublicKey, Timestamp};

const LIVE: Duration = Duration::from_secs(15);

async fn holds(n: &Node, g: &PublicKey, text: &str) -> bool {
    messages(n, g).await.iter().any(|m| m == text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_banned_users_patched_client_posts_and_no_honest_peer_keeps_it() {
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, carol, mallory) = (
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
    );
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let c = start(dirs[2].path(), &carol).await;
    let m = start(dirs[3].path(), &mallory).await;
    mesh(&[&a, &b, &c, &m]);
    let g = a
        .create_group("Mod", &MemoryStore::default())
        .await
        .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    for n in [&b, &c, &m] {
        n.join(&link).await.unwrap();
    }
    a.put(&g, &bob.public_key(), &["mod"]).await.unwrap();
    for n in [&b, &c, &m] {
        n.sync_with(loopback(&a)).await.unwrap();
    }
    let loops: Vec<_> = [&a, &b, &c]
        .iter()
        .map(|n| n.spawn_sync_loop(Duration::from_millis(300)))
        .collect();

    // Mallory says something legitimately, and — dated a few seconds ahead,
    // inside the clock skew every peer allows — her "next message", which
    // reaches Carol before the ban does.
    let now = Timestamp::now();
    let early = m
        .sign_unjudged(group::message(&g, "general", "early").custom_created_at(now - 3))
        .unwrap();
    let next = m
        .sign_unjudged(group::message(&g, "general", "next").custom_created_at(now + 5))
        .unwrap();
    for ev in [early, next] {
        let (ok, why) = m.push_unjudged(loopback(&c), ev).await.unwrap();
        assert!(ok, "Carol refused a member's message: {why}");
    }
    assert!(holds(&c, &g, "next").await);

    // Bob, a moderator, bans her.
    let ban = b.ban(&g, &mallory.public_key()).await.unwrap();
    assert!(ban.created_at <= now + 5);
    // Every honest peer learns of the ban — and Carol drops the message that
    // beat it there. The one before the ban stays: a ban is not a purge of
    // history.
    let settled = eventually(LIVE, || async {
        let mut ok = true;
        for n in [&a, &b, &c] {
            ok &= n
                .group(&g)
                .is_some_and(|s| s.is_banned(&mallory.public_key()));
            ok &= !holds(n, &g, "next").await;
        }
        ok && holds(&c, &g, "early").await
    })
    .await;
    if !settled {
        for (name, n) in [("A", &a), ("B", &b), ("C", &c)] {
            eprintln!(
                "{name}: banned {:?}, holds next {}, holds early {}",
                n.group(&g).map(|s| s.is_banned(&mallory.public_key())),
                holds(n, &g, "next").await,
                holds(n, &g, "early").await,
            );
        }
    }
    assert!(
        settled,
        "the ban did not settle, or Carol kept the late message"
    );

    // Now the patched client: messages dated now and backdated before the
    // ban, a reaction, straight to every honest peer, and her own invite
    // spent again.
    let mut lies = Vec::new();
    for (text, at) in [
        ("after", Timestamp::now()),
        ("backdated", ban.created_at - 10),
    ] {
        lies.push(
            m.sign_unjudged(group::message(&g, "general", text).custom_created_at(at))
                .unwrap(),
        );
    }
    let target = a.say(&g, "general", "react to me").await.unwrap();
    lies.push(
        m.sign_unjudged(group::reaction(
            &g,
            "general",
            &target.id,
            &alice.public_key(),
            "🙂",
        ))
        .unwrap(),
    );
    for peer in [&a, &b, &c] {
        for ev in &lies {
            let (ok, why) = m.push_unjudged(loopback(peer), ev.clone()).await.unwrap();
            assert!(!ok, "an honest peer took a banned user's event");
            assert!(why.starts_with("blocked:"), "{why}");
        }
    }
    // Dated apart from her first join: the same join in the same second is
    // the same event, and a peer answers a duplicate with "already have it".
    // Mallory's own app learns she was banned: the removal is the one thing
    // a removed person may still read.
    m.sync_with(loopback(&a)).await.unwrap();
    let hers = m.group(&g).unwrap();
    assert!(hers.is_banned(&mallory.public_key()) && !hers.is_member(&mallory.public_key()));
    let rejoin = m
        .sign_unjudged(group::join(&g, link.code.as_deref().unwrap_or_default()).custom_created_at(Timestamp::now() + 2))
        .unwrap();
    let (ok, _) = m.push_unjudged(loopback(&a), rejoin).await.unwrap();
    assert!(!ok, "a banned user walked back in on her old invite");

    // Nobody stores or shows any of it — not now, not after more syncs.
    tokio::time::sleep(Duration::from_secs(2)).await;
    for n in [&a, &b, &c] {
        for text in ["after", "backdated", "next"] {
            assert!(!holds(n, &g, text).await, "{text} is on an honest peer");
        }
        let reactions = n
            .channel_events(&g, "general", &[group::REACTION])
            .await
            .unwrap();
        assert!(reactions.iter().all(|r| r.pubkey != mallory.public_key()));
        assert!(!n.group(&g).unwrap().is_member(&mallory.public_key()));
    }

    for l in loops {
        l.abort();
    }
    for n in [a, b, c, m] {
        n.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moderators_removal_sticks_and_slowmode_holds() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, carol) = (Person::generate(), Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let c = start(dirs[2].path(), &carol).await;
    mesh(&[&a, &b, &c]);
    let g = a.create_group("Rm", &MemoryStore::default()).await.unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    c.join(&link).await.unwrap();
    a.put(&g, &bob.public_key(), &["mod"]).await.unwrap();
    b.sync_with(loopback(&a)).await.unwrap();
    c.sync_with(loopback(&a)).await.unwrap();

    // Carol says something; Bob (a moderator) and Alice have it.
    let rude = c.say(&g, "general", "something rude").await.unwrap();
    a.sync_with(loopback(&c)).await.unwrap();
    b.sync_with(loopback(&c)).await.unwrap();
    // Carol goes away still holding it. Bob removes it; Alice learns.
    let c_dir = dirs[2].path().to_path_buf();
    c.shutdown().await;
    b.remove_events(&g, "general", &[rude.id]).await.unwrap();
    assert!(!holds(&b, &g, "something rude").await);
    a.sync_with(loopback(&b)).await.unwrap();
    assert!(!holds(&a, &g, "something rude").await);
    // Carol comes back; her sync offers the message back and fetches the
    // removal. Alice refuses it; Carol drops it.
    let c = start(&c_dir, &carol).await;
    mesh(&[&a, &b, &c]);
    let stats = c.sync_with(loopback(&a)).await.unwrap();
    assert_eq!(
        stats.pushed, 0,
        "Alice took a removed message back: {stats}"
    );
    assert!(!holds(&a, &g, "something rude").await);
    assert!(!holds(&c, &g, "something rude").await);

    // A plain member's removal of somebody else's message is refused.
    let mine = a.say(&g, "general", "the owner's").await.unwrap();
    c.sync_with(loopback(&a)).await.unwrap();
    assert!(c.remove_events(&g, "general", &[mine.id]).await.is_err());

    // Slowmode: an admin edit, then a second message inside it is refused.
    a.edit(
        &g,
        &[Edit::Channel(group::Channel {
            id: "general".into(),
            name: "general".into(),
            kind: ChannelKind::Text,
            slowmode: 60,
        })],
    )
    .await
    .unwrap();
    c.sync_with(loopback(&a)).await.unwrap();
    assert_eq!(c.group(&g).unwrap().slowmode("general"), 60);
    c.say(&g, "general", "one").await.unwrap();
    let second = c.say(&g, "general", "two").await;
    assert!(
        matches!(&second, Err(e) if e.to_string().contains("slowmode")),
        "{second:?}"
    );

    for n in [a, b, c] {
        n.shutdown().await;
    }
}
