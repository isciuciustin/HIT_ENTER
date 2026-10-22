//! M3's "done when" (`docs/PLAN.md`), automated on loopback:
//!
//! three peers in a group; **the owner quits**; the other two keep talking,
//! and a fourth person joins from an invite with the owner still gone; the
//! owner comes back and sees everything. And a non-member who dials a
//! member and asks for the group's events gets nothing.
//!
//! Plus what M3 adds on the way: messages arrive live over the group's
//! gossip, with no sync, and a non-member cannot join that gossip either.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{eventually, loopback, loopback_addr, mesh, messages, start};
use hit_event::{MemoryStore, Person};
use hit_node::Node;
use nostr::prelude::{Event, Filter, PublicKey, SingleLetterTag};
use tokio::sync::broadcast;

const LIVE: Duration = Duration::from_secs(15);

/// Says `text` in `general` until `rx` hears it: the first gossip message
/// can go out before the swarm has formed.
async fn until_heard(
    from: &Node,
    g: &PublicKey,
    rx: &mut broadcast::Receiver<Event>,
    text: &str,
) -> bool {
    for _ in 0..8 {
        from.say(g, "general", text).await.unwrap();
        if hears_within(rx, text, Duration::from_secs(2)).await {
            return true;
        }
    }
    false
}

/// Waits up to `within` for a message with `text` on `rx`.
async fn hears_within(rx: &mut broadcast::Receiver<Event>, text: &str, within: Duration) -> bool {
    tokio::time::timeout(within, async {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.content == text => return true,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

async fn hears(rx: &mut broadcast::Receiver<Event>, text: &str) -> bool {
    hears_within(rx, text, LIVE).await
}

async fn member(node: &Node, g: &PublicKey, who: &PublicKey) -> bool {
    node.group(g).is_some_and(|s| s.is_member(who))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_group_outlives_its_owner_and_strangers_get_nothing() {
    let dirs: Vec<_> = (0..5).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, carol, dave, eve) = (
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
    );
    let keys = MemoryStore::default();

    // --- Three peers in a group. --------------------------------------
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let c = start(dirs[2].path(), &carol).await;
    mesh(&[&a, &b, &c]);
    let g = a.create_group("Friends", &keys).await.unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    c.join(&link).await.unwrap();
    // B learns of C's join (and C's binding) the ordinary way.
    b.sync_with(loopback(&a)).await.unwrap();
    for n in [&a, &b, &c] {
        for p in [&alice, &bob, &carol] {
            assert!(member(n, &g, &p.public_key()).await);
        }
    }

    // Live: a message reaches the others over gossip, with no sync at all.
    let (mut b_rx, mut c_rx) = (b.subscribe(), c.subscribe());
    assert!(
        until_heard(&a, &g, &mut b_rx, "ping").await,
        "B never heard A over gossip"
    );
    a.say(&g, "general", "live from alice").await.unwrap();
    assert!(hears(&mut b_rx, "live from alice").await, "B, gossip");
    assert!(hears(&mut c_rx, "live from alice").await, "C, gossip");

    // From here on every node also syncs, as the app does.
    let loops: Vec<_> = [&b, &c]
        .iter()
        .map(|n| n.spawn_sync_loop(Duration::from_millis(500)))
        .collect();

    // --- The owner quits. ---------------------------------------------
    a.shutdown().await;

    // The other two keep talking.
    b.say(&g, "general", "bob, owner gone").await.unwrap();
    c.say(&g, "general", "carol, owner gone").await.unwrap();
    assert!(
        eventually(LIVE, || async {
            let (bm, cm) = (messages(&b, &g).await, messages(&c, &g).await);
            bm.contains(&"carol, owner gone".to_owned())
                && cm.contains(&"bob, owner gone".to_owned())
        })
        .await,
        "B and C stopped talking without the owner"
    );

    // A fourth person joins from an invite — B's — with the owner gone.
    let d = start(dirs[3].path(), &dave).await;
    mesh(&[&b, &c, &d]);
    let invite = b
        .invite(
            &g,
            Some(Duration::from_secs(3_600)),
            Some(loopback_addr(&b)),
        )
        .await
        .unwrap();
    d.join(&invite).await.unwrap();
    assert!(member(&d, &g, &dave.public_key()).await);
    let d_loop = d.spawn_sync_loop(Duration::from_millis(500));
    d.say(&g, "general", "dave joined while alice was away")
        .await
        .unwrap();
    assert!(
        eventually(LIVE, || async {
            member(&c, &g, &dave.public_key()).await
                && messages(&c, &g)
                    .await
                    .contains(&"dave joined while alice was away".to_owned())
        })
        .await,
        "C never saw D join and speak"
    );

    // --- The owner comes back and sees everything. --------------------
    let a = start(dirs[0].path(), &alice).await;
    mesh(&[&a, &b, &c, &d]);
    let a_loop = a.spawn_sync_loop(Duration::from_millis(500));
    let everything = [
        "live from alice",
        "bob, owner gone",
        "carol, owner gone",
        "dave joined while alice was away",
    ];
    assert!(
        eventually(LIVE, || async {
            let am = messages(&a, &g).await;
            member(&a, &g, &dave.public_key()).await
                && everything.iter().all(|t| am.contains(&(*t).to_owned()))
        })
        .await,
        "the owner came back and did not see everything: {:?}",
        messages(&a, &g).await
    );

    // --- A non-member dials a member and asks for the group's events. --
    let e = start(dirs[4].path(), &eve).await;
    mesh(&[&e, &b]);
    let by_group = Filter::new().custom_tag(SingleLetterTag::LOWERCASE_H, g.to_hex());
    let by_state = Filter::new().author(g);
    // The control: a member asking the same thing gets the history.
    assert!(
        !d.fetch(loopback(&b), by_group.clone())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(e.fetch(loopback(&b), by_group).await.unwrap().is_empty());
    assert!(e.fetch(loopback(&b), by_state).await.unwrap().is_empty());
    // Asking for everything: the only thing served is B's own binding, which
    // the stranger was handed in the session's first frame anyway.
    let all = e.fetch(loopback(&b), Filter::new()).await.unwrap();
    assert!(
        all.iter()
            .all(|ev| ev.pubkey == bob.public_key() && ev.kind == hit_event::DEVICE_BINDING),
        "a stranger was served something: {:?}",
        all.iter().map(|e| e.kind.as_u16()).collect::<Vec<_>>()
    );
    // Negentropy too: nothing to reconcile with a stranger.
    let synced = e.sync_with(loopback(&b)).await.unwrap();
    assert_eq!((synced.fetched, synced.pushed), (0, 0), "{synced}");
    // Nor can it join the group's gossip: the gate closes the connection.
    let conn = e
        .endpoint()
        .connect(loopback(&b), &hit_proto::gossip_alpn(&g))
        .await;
    if let Ok(conn) = conn {
        let closed = tokio::time::timeout(LIVE, conn.closed()).await;
        assert!(closed.is_ok(), "B kept a stranger's gossip connection open");
    }

    for l in loops.into_iter().chain([d_loop, a_loop]) {
        l.abort();
    }
    for n in [a, b, c, d, e] {
        n.shutdown().await;
    }
}

/// Kicked means gone from gossip too: the removed member's live gossip
/// connection is closed, and nothing new reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_member_stops_hearing_the_group() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, mallory) = (Person::generate(), Person::generate(), Person::generate());
    let keys = MemoryStore::default();
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let m = start(dirs[2].path(), &mallory).await;
    mesh(&[&a, &b, &m]);
    let g = a.create_group("Kick", &keys).await.unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    m.join(&link).await.unwrap();

    let mut m_rx = m.subscribe();
    assert!(
        until_heard(&a, &g, &mut m_rx, "before").await,
        "M never heard the group"
    );

    a.remove(&g, &mallory.public_key()).await.unwrap();
    assert!(!member(&a, &g, &mallory.public_key()).await);
    // B learns of the removal by sync (or gossip); either way, then:
    b.sync_with(loopback(&a)).await.unwrap();
    assert!(!member(&b, &g, &mallory.public_key()).await);

    a.say(&g, "general", "after").await.unwrap();
    assert!(
        !hears_within(&mut m_rx, "after", Duration::from_secs(3)).await,
        "a removed member still heard the group"
    );
    // And a sync serves it nothing new.
    m.sync_with(loopback(&a)).await.unwrap();
    assert!(!messages(&m, &g).await.contains(&"after".to_owned()));

    for n in [a, b, m] {
        n.shutdown().await;
    }
}
