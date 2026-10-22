//! M7's "done when" (`docs/PLAN.md`), automated on loopback:
//!
//! **a DM to your friend while their app is closed arrives when they open
//! it, via an anchor — and a scan of the bytes of that anchor's database
//! after delivery finds neither the text nor the sender's npub.**
//! (HIT_ENTER_2's `a_space_holds_nothing_it_could_log_in_with`, reborn.)
//!
//! The anchor serves Bob (it pins a group of his, and his inbox list names
//! it); it has never shared a group with Alice. Alice finds it through Bob's
//! inbox list, which she holds because she shares another group with Bob.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{eventually, loopback, loopback_addr, mesh, start};
use hit_event::{MemoryStore, Person, dm};
use nostr::prelude::{Filter, ToBech32};

const TEXT: &str = "a secret for bob, sent while he sleeps";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_waits_at_an_anchor_that_cannot_read_it_or_say_who_sent_it() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, anchor) = (Person::generate(), Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let n = start(dirs[2].path(), &anchor).await;
    // Where everyone is, as discovery would say. Alice knows the anchor's
    // address but has nothing else of it.
    mesh(&[&a, &b, &n]);
    let keys = MemoryStore::default();

    // Bob's home group, pinned by the anchor.
    let home = b.create_group("Bob's", &keys).await.unwrap();
    n.join(
        &b.invite(&home, None, Some(loopback_addr(&b)))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    n.set_anchor_profile("bob's anchor").await.unwrap();
    // A group Alice and Bob share; the anchor is not in it.
    let shared = a.create_group("Friends", &keys).await.unwrap();
    b.join(
        &a.invite(&shared, None, Some(loopback_addr(&a)))
            .await
            .unwrap(),
    )
    .await
    .unwrap();

    // Bob names the anchor as his inbox; the anchor and Alice learn it.
    b.sync_with(loopback(&n)).await.unwrap();
    b.set_inbox(&[anchor.public_key()]).await.unwrap();
    n.sync_with(loopback(&b)).await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    assert_eq!(
        a.inbox_of(&bob.public_key()).await.unwrap(),
        vec![(anchor.public_key(), Some(n.id()))]
    );

    // Bob closes the app.
    let b_dir = dirs[1].path().to_path_buf();
    b.shutdown().await;

    // Alice writes. It reaches the anchor, sealed.
    let sent = a.send_dm(&bob.public_key(), TEXT).await.unwrap();
    assert_eq!(sent.text, TEXT);
    let wraps = Filter::new().kind(dm::GIFT_WRAP);
    assert!(
        eventually(Duration::from_secs(15), || async {
            !n.store().query(&wraps).await.unwrap().is_empty()
        })
        .await,
        "the DM never reached Bob's anchor"
    );
    // The anchor holds exactly one: Bob's. Not Alice's own copy.
    let held = n.store().query(&wraps).await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(dm::recipient_of(&held[0]), Some(bob.public_key()));
    // Alice closes the app too.
    a.shutdown().await;

    // Bob opens his: the DM arrives, from the anchor.
    let b = start(&b_dir, &bob).await;
    mesh(&[&b, &n]);
    let n_loop = n.spawn_sync_loop(Duration::from_millis(500));
    let b_loop = b.spawn_sync_loop(Duration::from_millis(500));
    assert!(
        eventually(Duration::from_secs(15), || async {
            b.dms().await.unwrap().iter().any(|d| d.text == TEXT)
        })
        .await,
        "Bob never got the DM"
    );
    let got = b.dms().await.unwrap();
    assert_eq!(got[0].from, alice.public_key());
    assert_eq!(got[0].id, sent.id);
    n_loop.abort();
    b_loop.abort();

    // The scan: every byte of the anchor's data directory — the database,
    // its WAL and shared memory — after a clean shutdown.
    n.shutdown().await;
    // SQLite folds the WAL into the database and removes it as the last
    // connection closes, after shutdown returns: let it settle, and skip a
    // file that goes while we look (its contents are in `node.db` then).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let alice_hex = alice.public_key().to_hex();
    let alice_npub = alice.public_key().to_bech32().unwrap();
    let alice_raw = alice.public_key().to_bytes();
    let mut scanned = 0;
    for entry in std::fs::read_dir(dirs[2].path()).unwrap() {
        let path = entry.unwrap().path();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        scanned += bytes.len();
        let name = path.display();
        assert!(!contains(&bytes, TEXT.as_bytes()), "the text is in {name}");
        assert!(
            !contains(&bytes, alice_hex.as_bytes()),
            "Alice's key (hex) is in {name}"
        );
        assert!(
            !contains(&bytes, alice_npub.as_bytes()),
            "Alice's npub is in {name}"
        );
        assert!(
            !contains(&bytes, &alice_raw),
            "Alice's key (raw) is in {name}"
        );
    }
    assert!(
        scanned > 10_000,
        "scanned only {scanned} bytes — nothing there?"
    );
    // And the control: Bob — whom the anchor serves — *is* in there, as the
    // recipient NIP-17 does not hide. The scan can find a key when one is.
    let bob_hex = bob.public_key().to_hex();
    let db = std::fs::read(dirs[2].path().join("node.db")).unwrap();
    let wal = std::fs::read(dirs[2].path().join("node.db-wal")).unwrap_or_default();
    assert!(contains(&db, bob_hex.as_bytes()) || contains(&wal, bob_hex.as_bytes()));

    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_between_two_open_apps_arrives_directly() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob) = (Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    mesh(&[&a, &b]);
    let g = a
        .create_group("Two", &MemoryStore::default())
        .await
        .unwrap();
    b.join(&a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap())
        .await
        .unwrap();
    a.sync_with(loopback(&b)).await.unwrap();

    // No sync loop: the send hands it over itself.
    a.send_dm(&bob.public_key(), "quick one").await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            b.dms().await.unwrap().iter().any(|d| d.text == "quick one")
        })
        .await
    );
    // A reply, and both sides see the conversation in order.
    b.send_dm(&alice.public_key(), "got it").await.unwrap();
    assert!(
        eventually(Duration::from_secs(10), || async {
            a.dms().await.unwrap().len() == 2
        })
        .await
    );
    let texts: Vec<String> = a.dms().await.unwrap().into_iter().map(|d| d.text).collect();
    assert!(texts.contains(&"quick one".to_owned()) && texts.contains(&"got it".to_owned()));
    // A third member of the group sees neither.
    a.shutdown().await;
    b.shutdown().await;
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
