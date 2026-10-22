//! M8's "done when" (`docs/PLAN.md`), automated on loopback:
//!
//! **a file posted while its uploader is online is later downloaded,
//! byte-for-byte, by a member after the uploader has gone offline, from
//! another member's cache.** And the serving gate: a stranger is refused,
//! and so is a member asking for a file in a channel they cannot see.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{loopback, loopback_addr, mesh, start};
use hit_event::{
    MemoryStore, Person,
    group::{Channel, ChannelKind, DEFAULT_ROLE, Edit, Override},
};
use hit_group::Permissions;

/// 300 KB that compress to nothing useful: a file iroh-blobs must send in
/// many chunks, verified as it goes.
fn bytes() -> Vec<u8> {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..300_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_outlives_its_uploader_in_another_members_cache_and_is_served_only_to_readers() {
    let dirs: Vec<_> = (0..5).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, carol, dave, eve) = (
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
    );
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let c = start(dirs[2].path(), &carol).await;
    let d = start(dirs[3].path(), &dave).await;
    let e = start(dirs[4].path(), &eve).await;
    mesh(&[&a, &b, &c, &d, &e]);
    let g = a
        .create_group("Files", &MemoryStore::default())
        .await
        .unwrap();
    // A channel only Bob (made a moderator) and Alice can see.
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
                deny: Permissions::VIEW_CHANNEL.bits(),
            }),
            Edit::Override(Override {
                channel: "mods".into(),
                role: "mod".into(),
                allow: Permissions::VIEW_CHANNEL.bits(),
                deny: 0,
            }),
        ],
    )
    .await
    .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    for n in [&b, &c, &d] {
        n.join(&link).await.unwrap();
    }
    a.put(&g, &bob.public_key(), &["mod"]).await.unwrap();

    // Alice posts a file, and one in the private channel.
    let original = bytes();
    let path = dirs[0].path().join("holiday photo.jpg");
    std::fs::write(&path, &original).unwrap();
    let file = a.attach(&path).await.unwrap();
    assert_eq!((file.size, file.mime.as_str()), (300_000, "image/jpeg"));
    assert_eq!(file.name, "holiday photo.jpg");
    a.say_with(&g, "general", "look", std::slice::from_ref(&file))
        .await
        .unwrap();
    let secret_path = dirs[0].path().join("plans.txt");
    std::fs::write(&secret_path, b"only for the mods").unwrap();
    let secret = a.attach(&secret_path).await.unwrap();
    a.say_with(&g, "mods", "", std::slice::from_ref(&secret))
        .await
        .unwrap();
    for n in [&b, &c, &d] {
        n.sync_with(loopback(&a)).await.unwrap();
    }

    // Bob opens it: fetched from Alice, byte for byte, now in his cache.
    b.fetch_file(&file.hash).await.unwrap();
    assert_eq!(b.file_bytes(&file.hash).await.unwrap(), original);
    b.fetch_file(&secret.hash).await.unwrap();

    // Alice goes offline. Carol opens it: from Bob's cache.
    a.shutdown().await;
    c.fetch_file(&file.hash).await.unwrap();
    assert_eq!(c.file_bytes(&file.hash).await.unwrap(), original);
    let out = dirs[2].path().join("saved.jpg");
    c.save_file(&file.hash, &out).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), original);

    // The gate. A stranger asking Bob for it by hash: refused.
    assert!(e.fetch_file_from(loopback(&b), &file.hash).await.is_err());
    // Dave, a member who cannot see #mods, asking Bob for its file: refused.
    assert!(d.fetch_file_from(loopback(&b), &secret.hash).await.is_err());
    assert!(d.fetch_file(&secret.hash).await.is_err());
    // ...and the same Dave gets the public one.
    d.fetch_file_from(loopback(&b), &file.hash).await.unwrap();
    assert_eq!(d.file_bytes(&file.hash).await.unwrap(), original);

    for n in [b, c, d, e] {
        n.shutdown().await;
    }
}
