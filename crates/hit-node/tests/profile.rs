//! M4's profiles and reachability, on loopback: a name travels to the people
//! who share a group with its owner, by sync, and to nobody else.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{loopback, loopback_addr, mesh, start};
use hit_event::{DeviceKey, MemoryStore, Person, Timestamp, sign_binding};

#[tokio::test]
async fn a_name_reaches_group_members_and_not_strangers() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, eve) = (Person::generate(), Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let e = start(dirs[2].path(), &eve).await;
    mesh(&[&a, &b, &e]);

    // Named before any group exists: kept, and shown to nobody yet.
    a.set_name("Alice").await.unwrap();
    a.set_name("Alice A.").await.unwrap();
    e.set_name("Eve").await.unwrap();
    let g = a
        .create_group("Names", &MemoryStore::default())
        .await
        .unwrap();
    b.join(&a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap())
        .await
        .unwrap();
    b.set_name("Bob").await.unwrap();
    b.sync_with(loopback(&a)).await.unwrap();

    let (ap, bp) = (alice.public_key(), bob.public_key());
    let at_b = b.names(&[ap, bp]).await.unwrap();
    assert_eq!(at_b.get(&ap).map(String::as_str), Some("Alice A."));
    assert_eq!(at_b.get(&bp).map(String::as_str), Some("Bob"));
    assert_eq!(
        a.names(&[bp]).await.unwrap().get(&bp).map(String::as_str),
        Some("Bob")
    );

    // A stranger syncs with a member: no names either way.
    e.sync_with(loopback(&a)).await.unwrap();
    assert!(e.names(&[ap, bp]).await.unwrap().is_empty());
    assert!(a.names(&[eve.public_key()]).await.unwrap().is_empty());

    // Reachable: whoever completed a session with us, either way.
    let near = Duration::from_secs(60);
    assert!(a.reachable(near).contains(&bp), "B dialled A just now");

    for n in [a, b, e] {
        n.shutdown().await;
    }
}

/// A member offline past their binding's expiry: the binding stays in the
/// store, but no peer would take it, so it is not offered. Before, the
/// VPS anchor refused it every sync round (2026-10-24).
#[tokio::test]
async fn an_expired_binding_is_not_offered() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, carol) = (Person::generate(), Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let c = start(dirs[2].path(), &carol).await;
    mesh(&[&a, &b, &c]);
    let g = a
        .create_group("Bindings", &MemoryStore::default())
        .await
        .unwrap();
    for n in [&b, &c] {
        n.join(&a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap())
            .await
            .unwrap();
    }

    // Bob's second device, bound for one second, reaches Alice while fresh.
    let short = sign_binding(
        &bob,
        &DeviceKey::generate(),
        Timestamp::now(),
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(a.accept(short).await.unwrap().0);
    tokio::time::sleep(Duration::from_millis(2100)).await;

    let stats = c.sync_with(loopback(&a)).await.unwrap();
    assert_eq!(stats.refused, 0, "{stats}");

    for n in [a, b, c] {
        n.shutdown().await;
    }
}
