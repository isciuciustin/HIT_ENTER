//! M13's public groups, on loopback: a stranger walks in through a link
//! with no invite in it, and waits before posting — and a private group's
//! door stays shut.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{loopback_addr, mesh, start};
use hit_event::{MemoryStore, Person, group::Edit};
use hit_proto::invite::InviteLink;

#[tokio::test]
async fn a_stranger_joins_a_public_group_without_an_invite_and_waits_to_post() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, stranger) = (Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let s = start(dirs[1].path(), &stranger).await;
    mesh(&[&a, &s]);
    let g = a
        .create_group("Open", &MemoryStore::default())
        .await
        .unwrap();

    // Private: no public link to give, and a code-less join is refused.
    assert!(a.public_link(&g, Some(loopback_addr(&a))).await.is_err());
    let shut = InviteLink {
        group: g,
        code: None,
        peers: vec![(a.id(), Some(loopback_addr(&a)))],
    };
    assert!(s.join(&shut).await.is_err(), "a private group let a stranger in");

    // Public: the link has no code, and opens the door.
    a.edit(&g, &[Edit::Public(true)]).await.unwrap();
    let link = a.public_link(&g, Some(loopback_addr(&a))).await.unwrap();
    assert_eq!(link.code, None);
    assert!(!link.to_string().contains("&c="));
    s.join(&link).await.unwrap();
    assert!(
        a.group(&g)
            .is_some_and(|st| st.is_member(&stranger.public_key())),
        "Alice's node counts the stranger a member"
    );

    // A newcomer: their own node refuses their first post, with the wait.
    let err = s.say(&g, "general", "hello!").await.unwrap_err().to_string();
    assert!(err.contains("new here"), "{err}");

    for n in [a, s] {
        n.shutdown().await;
    }
}
