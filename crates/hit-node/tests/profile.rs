//! M4's profiles and reachability, on loopback: a name travels to the people
//! who share a group with its owner, by sync, and to nobody else.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{loopback, loopback_addr, mesh, start};
use hit_event::{MemoryStore, Person};

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
