//! M13's public groups, on loopback: a stranger walks in through a link
//! with no invite in it, and waits before posting — and a private group's
//! door stays shut.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{loopback_addr, mesh, messages, start};
use hit_event::{
    MemoryStore, Person,
    group::{Channel, ChannelKind, DEFAULT_ROLE, Edit, Override},
};
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

/// Read before joining: a stranger's node follows a public group's link,
/// holds its open channel and not its mods-only one, cannot post — and a
/// private group's link is refused and put down.
#[tokio::test]
async fn a_stranger_reads_a_public_group_before_joining() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, reader) = (Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let r = start(dirs[1].path(), &reader).await;
    mesh(&[&a, &r]);
    let g = a
        .create_group("Town", &MemoryStore::default())
        .await
        .unwrap();
    a.edit(
        &g,
        &[
            Edit::Channel(Channel {
                id: "mods".into(),
                name: "mods".into(),
                kind: ChannelKind::Text,
                slowmode: 0,
            }),
            Edit::Override(Override {
                channel: "mods".into(),
                role: DEFAULT_ROLE.into(),
                allow: 0,
                deny: 1, // view
            }),
        ],
    )
    .await
    .unwrap();
    a.say(&g, "general", "welcome, everyone").await.unwrap();
    a.say(&g, "mods", "staff only").await.unwrap();

    // Private: following is refused, and the group put down.
    let peers = vec![(a.id(), Some(loopback_addr(&a)))];
    let private = InviteLink {
        group: g,
        code: None,
        peers: peers.clone(),
    };
    assert!(r.follow(&private).await.is_err());
    assert!(r.group(&g).is_none());
    assert!(messages(&r, &g).await.is_empty());

    // Public: the open channel arrives, the mods' does not. A second
    // apart from the channel edit: a node dates its own second action of a
    // kind in one second a second ahead, to keep their order — and a join
    // dated before the group went public is refused.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    a.edit(&g, &[Edit::Public(true)]).await.unwrap();
    let link = a.public_link(&g, Some(loopback_addr(&a))).await.unwrap();
    r.follow(&link).await.unwrap();
    let held = messages(&r, &g).await;
    assert!(held.contains(&"welcome, everyone".to_owned()), "{held:?}");
    assert!(!held.contains(&"staff only".to_owned()), "{held:?}");
    let st = r.group(&g).unwrap();
    assert!(st.is_public() && !st.is_member(&reader.public_key()));
    assert!(r.say(&g, "general", "hi").await.is_err(), "a reader posted");

    // And may join from there.
    r.join(&link).await.unwrap();
    assert!(r.group(&g).unwrap().is_member(&reader.public_key()));

    for n in [a, r] {
        n.shutdown().await;
    }
}
