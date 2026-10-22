//! M4b on loopback: reactions and their withdrawal travel like messages; a
//! message its author withdrew stays withdrawn even when a peer that still
//! holds it pushes it back; presence arrives over gossip and is never
//! stored.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{loopback, loopback_addr, mesh, start};
use hit_event::{
    MemoryStore, Person,
    group::{CHAT_MESSAGE, REACTION},
};
use hit_node::{Node, NodeError};
use nostr::prelude::{Event, PublicKey};

async fn said(n: &Node, g: &PublicKey) -> Vec<Event> {
    n.channel_events(g, "general", &[CHAT_MESSAGE, REACTION])
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reactions_and_withdrawals_travel_and_a_withdrawn_message_stays_gone() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob) = (Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    mesh(&[&a, &b]);
    let g = a.create_group("R", &MemoryStore::default()).await.unwrap();
    b.join(&a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap())
        .await
        .unwrap();

    let msg = a.say(&g, "general", "react to me").await.unwrap();
    b.sync_with(loopback(&a)).await.unwrap();
    let thumbs = b.react(&g, "general", &msg.id, "👍").await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    assert!(said(&a, &g).await.iter().any(|e| e.id == thumbs.id));

    // Nobody withdraws somebody else's words.
    assert!(matches!(
        b.delete(&g, "general", &[msg.id]).await,
        Err(NodeError::NotYours)
    ));

    // Bob takes his reaction back: gone at Alice after a sync.
    b.delete(&g, "general", &[thumbs.id]).await.unwrap();
    assert!(!said(&b, &g).await.iter().any(|e| e.id == thumbs.id));
    a.sync_with(loopback(&b)).await.unwrap();
    assert!(!said(&a, &g).await.iter().any(|e| e.id == thumbs.id));

    // Alice withdraws her message while Bob is away. Bob still holds it, and
    // the sync that brings him the withdrawal also offers Alice the message
    // back: she refuses it, and he drops it.
    let b_dir = dirs[1].path().to_path_buf();
    b.shutdown().await;
    a.delete(&g, "general", &[msg.id]).await.unwrap();
    let b = start(&b_dir, &bob).await;
    mesh(&[&a, &b]);
    assert!(said(&b, &g).await.iter().any(|e| e.id == msg.id));
    let stats = b.sync_with(loopback(&a)).await.unwrap();
    assert_eq!(
        stats.pushed, 0,
        "Alice took her withdrawn message back: {stats}"
    );
    assert!(said(&a, &g).await.is_empty());
    assert!(said(&b, &g).await.is_empty());

    // Presence: over gossip, never stored.
    let before = (
        a.store().count().await.unwrap(),
        b.store().count().await.unwrap(),
    );
    // Watched for on Alice's subscription: `present()` alone would also
    // count Bob's sync sessions, which prove nothing about gossip.
    let mut rx = a.subscribe();
    let mut heard = false;
    for _ in 0..30 {
        b.announce_presence().await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(ev) = rx.try_recv() {
            heard |= ev.kind == hit_event::group::PRESENCE && ev.pubkey == bob.public_key();
        }
        if heard {
            break;
        }
    }
    assert!(heard, "Alice never heard Bob's presence");
    assert_eq!(
        (
            a.store().count().await.unwrap(),
            b.store().count().await.unwrap()
        ),
        before,
        "a presence was stored"
    );

    a.shutdown().await;
    b.shutdown().await;
}
