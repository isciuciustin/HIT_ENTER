//! M2's "done when" (`docs/PLAN.md`), automated on loopback — now inside a
//! group, since M3 replaced M2's roster and test notes with groups and
//! their messages. The same sequence by hand, with three CLI processes, is
//! in `docs/TESTING.md`.
//!
//! Two nodes exchange signed events; one is stopped, the other writes twenty
//! more, the first comes back and negentropy fetches exactly those twenty
//! and no duplicates; a third node presenting a copied binding is refused,
//! and with its own binding it is served nothing.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{loopback, loopback_addr, mesh, messages, start};
use hit_event::{MemoryStore, Person};
use hit_node::{Config, Node, NodeError, Relays};

#[tokio::test]
async fn two_relays_converge_after_one_was_away_and_a_copied_binding_is_refused() {
    let (dir_a, dir_b, dir_c) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (alice, bob, carol) = (Person::generate(), Person::generate(), Person::generate());
    let keys = MemoryStore::default();

    let a = start(dir_a.path(), &alice).await;
    let b = start(dir_b.path(), &bob).await;
    mesh(&[&a, &b]);
    let a_id = a.id();
    let g = a.create_group("M2", &keys).await.unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();

    // Two nodes exchange signed events.
    for i in 0..5 {
        a.say(&g, "general", &format!("from alice {i}"))
            .await
            .unwrap();
    }
    b.say(&g, "general", "from bob, before").await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    assert_eq!(
        (messages(&a, &g).await.len(), messages(&b, &g).await.len()),
        (6, 6)
    );

    // A is stopped. B writes twenty more.
    let a_binding = a.binding();
    a.shutdown().await;
    for i in 0..20 {
        b.say(&g, "general", &format!("from bob while alice was away {i}"))
            .await
            .unwrap();
    }

    // A comes back — same data dir, same device — and fetches exactly those
    // twenty, with no duplicates.
    let a = start(dir_a.path(), &alice).await;
    assert_eq!(a.id(), a_id, "the device key survived the restart");
    mesh(&[&a, &b]);
    let back = a.sync_with(loopback(&b)).await.unwrap();
    assert_eq!(back.fetched, 20, "{back}");
    assert_eq!(back.duplicates, 0, "{back}");
    assert_eq!(back.pushed, 0, "{back}");
    assert_eq!(
        (messages(&a, &g).await.len(), messages(&b, &g).await.len()),
        (26, 26)
    );

    // Nothing changed: one round, nothing moves.
    let idle = a.sync_with(loopback(&b)).await.unwrap();
    assert_eq!(
        (idle.rounds, idle.fetched, idle.pushed),
        (1, 0, 0),
        "{idle}"
    );

    // A third node presents A's binding, copied. Its connection says a
    // different device, so B refuses it — and stores nothing from it.
    let c = start(dir_c.path(), &carol).await;
    mesh(&[&a, &b, &c]);
    let before = b.store().count().await.unwrap();
    match c.sync_presenting(loopback(&b), a_binding).await {
        Err(NodeError::RefusedByPeer(why)) => assert!(why.contains("different device"), "{why}"),
        other => panic!("a copied binding was not refused: {other:?}"),
    }
    // With its own, valid binding it gets a session — and is served nothing:
    // carol is in no group with B.
    let stranger = c.sync_with(loopback(&b)).await.unwrap();
    assert_eq!((stranger.fetched, stranger.pushed), (0, 0), "{stranger}");
    assert_eq!(
        b.store().count().await.unwrap(),
        before,
        "B stored something from C"
    );

    // Invited, the same node joins and has the whole history.
    let link = b.invite(&g, None, Some(loopback_addr(&b))).await.unwrap();
    let joined = c.join(&link).await.unwrap();
    assert_eq!(joined.duplicates, 0, "{joined}");
    assert_eq!(messages(&c, &g).await.len(), 26);

    for n in [a, b, c] {
        n.shutdown().await;
    }
}

#[tokio::test]
async fn a_revoked_device_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let alice = Person::generate();
    let a = start(dir.path(), &alice).await;
    let device = a.id();
    // The person revokes this device (from another one), and the revocation
    // reaches this node's store.
    let revocation = hit_event::sign_revocation(
        &alice,
        device,
        nostr::prelude::Timestamp::now() + std::time::Duration::from_secs(1),
    )
    .unwrap();
    a.store()
        .insert(&hit_event::Verified::check(revocation).unwrap())
        .await
        .unwrap();
    a.shutdown().await;
    match Node::start(
        common::same_person(&alice),
        Config {
            data_dir: dir.path().to_path_buf(),
            relays: Relays::Disabled,
            port: None,
        },
    )
    .await
    {
        Err(NodeError::DeviceRevoked) => {}
        other => panic!("a revoked device started: {:?}", other.map(|n| n.id())),
    }
}
