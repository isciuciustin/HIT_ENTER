//! M10's "done when" (`docs/PLAN.md`), automated on loopback against the
//! fake Lightning network (`hit_zap::fake`) — every hop real except the
//! sats:
//!
//! **Alice zaps Bob's message 1,000 sats; Bob's wallet receives 1,000 and
//! HIT_ENTER's 10; the total appears in Alice's app, Bob's and a third
//! member's (Carol's); a forged receipt — valid signature, wrong key — is
//! shown nowhere; a 50-sat zap charges no fee; and a fee payment forced to
//! fail leaves the zap delivered and counted.**
//!
//! The receipt's two roads: to the group's anchor's `ws://`, which carries
//! it in (Alice's app never sees it), and to a relay the sender reads,
//! after which her app carries it in. The real thing, with real sats and
//! real wallets, is `docs/TESTING.md` §7.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../hit-node/tests/common/mod.rs"]
mod common;

use std::time::Duration;

use common::{eventually, loopback, loopback_addr, mesh, start};
use hit_anchor::ws;
use hit_event::{
    MemoryStore, Person,
    zap::{self, Invoice},
};
use hit_node::Node;
use hit_zap::{Fee, LightningAddress, Wallet, Zapper, fake::FakeLightning, relay::Relay};
use nostr::prelude::{
    Event, EventId, FinalizeEvent, IntoEventBuilder, Keys, nip57::ZapReceipt,
};
use sha2::{Digest, Sha256};

async fn sats(n: &Node, id: EventId) -> u64 {
    n.zap_totals(&[id])
        .await
        .unwrap()
        .get(&id)
        .map_or(0, |t| t.msat / 1000)
}

/// Syncs every member with the anchor until each shows `want` sats on `id`.
async fn everyone_sees(nodes: &[&Node], anchor: &Node, id: EventId, want: u64) -> bool {
    eventually(Duration::from_secs(15), || async {
        let mut all = true;
        for x in nodes {
            let _ = x.sync_with(loopback(anchor)).await;
            all &= sats(x, id).await == want;
        }
        all
    })
    .await
}

/// A receipt as a provider would sign it — by `provider`, whoever that is.
fn receipt_signed_by(provider: &Keys, request: &Event, msat: u64) -> Event {
    let inv = Invoice {
        currency: "bc".into(),
        amount_msat: Some(msat),
        timestamp: 1_700_000_000,
        payment_hash: Some([3; 32]),
        description_hash: Some(Sha256::digest(request.as_json().as_bytes()).into()),
        description: None,
        expiry: 3600,
    };
    ZapReceipt::new(inv.encode_unsigned(), request)
        .into_event_builder()
        .finalize(provider)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zap_is_paid_whole_counted_everywhere_and_a_forgery_nowhere() {
    let net = FakeLightning::start().await;
    net.account("alice", 10_000);
    let bob_addr = net.account("bob", 0);
    let fee_addr: LightningAddress = net.account("hitenter", 0).parse().unwrap();
    let wallet = Wallet::parse(&net.connect_wallet("alice")).unwrap();
    let zapper = Zapper::new(Some(fee_addr))
        .unwrap()
        .with_timeouts(Duration::from_millis(200), Duration::from_secs(3));

    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let people: Vec<Person> = (0..4).map(|_| Person::generate()).collect();
    let (alice, bob, carol, anchor) = (&people[0], &people[1], &people[2], &people[3]);
    let a = start(dirs[0].path(), alice).await;
    let b = start(dirs[1].path(), bob).await;
    let c = start(dirs[2].path(), carol).await;
    let n = start(dirs[3].path(), anchor).await;
    mesh(&[&a, &b, &c, &n]);
    let g = a
        .create_group("Zappers", &MemoryStore::default())
        .await
        .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    n.join(&link).await.unwrap();
    n.set_anchor_profile("the group's anchor").await.unwrap();
    b.join(&link).await.unwrap();
    c.join(&link).await.unwrap();

    // The anchor serves its bridge and says where.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", listener.local_addr().unwrap());
    let bridge = ws::Bridge::new(n.clone(), Person::from_nsec(&anchor.reveal_nsec_for_backup()).unwrap(), "anchor".into(), None);
    let serving = tokio::spawn(async move { ws::serve_on(bridge, listener).await });
    n.set_anchor_relay(Some(&ws_url)).await.unwrap();

    // Bob's Lightning address, and the key its server signs with — what
    // his app reads from the server when he sets it.
    b.set_lud16(Some((&bob_addr, &net.provider()))).await.unwrap();
    let file = b.say(&g, "general", "my file, zap it").await.unwrap();
    for x in [&a, &b, &c] {
        x.sync_with(loopback(&n)).await.unwrap();
    }
    for x in [&a, &b, &c] {
        x.sync_with(loopback(&n)).await.unwrap();
    }
    assert_eq!(a.receipt_relays(&g).await.unwrap(), vec![ws_url.clone()]);
    let (addr, key) = a.lud16_of(&bob.public_key()).await.unwrap().unwrap();
    assert_eq!(key, Some(net.provider()));
    let bob_addr: LightningAddress = addr.parse().unwrap();

    // --- 1,000 sats, the receipt to the anchor. --------------------------
    let quote = zapper.quote(&bob_addr, 1_000).await.unwrap();
    assert_eq!(quote.total_sats(), 1_010, "the dialog's total");
    let relays = a.receipt_relays(&g).await.unwrap();
    let request = a
        .sign_unjudged(zap::request(&bob.public_key(), &file.id, 1_000_000, &relays, &quote.params.lnurl, "for the file", false))
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    assert_eq!(zapped.fee.await.unwrap(), Fee::Paid);
    assert_eq!(net.balance_sats("bob"), 1_000, "arrives as 1,000, not 990");
    assert_eq!(net.balance_sats("hitenter"), 10);
    // The anchor carried it in; every member counts it.
    assert!(
        everyone_sees(&[&a, &b, &c], &n, file.id, 1_000).await,
        "a {} b {} c {}",
        sats(&a, file.id).await,
        sats(&b, file.id).await,
        sats(&c, file.id).await
    );
    let t = c.zap_totals(&[file.id]).await.unwrap()[&file.id].clone();
    assert_eq!((t.count, t.zappers), (1, vec![alice.public_key()]));

    // --- 50 sats, the receipt to a relay Alice reads; she carries it. ---
    let quote = zapper.quote(&bob_addr, 50).await.unwrap();
    assert_eq!(quote.fee_sats, 0);
    let request = a
        .sign_unjudged(zap::request(&bob.public_key(), &file.id, 50_000, &[net.relay_url()], &quote.params.lnurl, "", false))
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    let (receipt, _) = zapped.receipt.expect("Alice fetched the receipt");
    assert_eq!(zapped.fee.await.unwrap(), Fee::None);
    assert!(a.carry_zap(&receipt).await.unwrap().is_some());
    assert!(a.carry_zap(&receipt).await.unwrap().is_none(), "carried once");
    assert!(everyone_sees(&[&a, &b, &c], &n, file.id, 1_050).await);
    assert_eq!(net.balance_sats("hitenter"), 10, "no fee under 100 sats");

    // --- A forgery: valid signature, wrong key, a million sats. ---------
    let fake_request = c
        .sign_unjudged(zap::request(&bob.public_key(), &file.id, 1_000_000_000, std::slice::from_ref(&ws_url), "LNURL1X", "", false))
        .unwrap();
    let forged = receipt_signed_by(&Keys::generate(), &fake_request, 1_000_000_000);
    zap::check_receipt(&forged).expect("well-formed — only the key is wrong");
    // The anchor refuses it at the door…
    let mut r = Relay::connect(&ws_url).await.unwrap();
    assert!(r.publish(&forged, Duration::from_secs(5)).await.is_err());
    // …an honest app refuses to carry it…
    assert!(matches!(a.carry_zap(&forged).await, Err(hit_node::NodeError::NotTheirProvider)));
    // …and Carol, lying, carries it anyway to everyone: stored, shown
    // nowhere.
    let z = zap::check_receipt(&forged).unwrap();
    let lie = c.sign_unjudged(zap::carrier(&g, "general", &forged, &z)).unwrap();
    for x in [&a, &b, &n] {
        let (stored, why) = c.push_unjudged(loopback(x), lie.clone()).await.unwrap();
        assert!(stored, "{why}");
    }
    assert!(everyone_sees(&[&a, &b, &c], &n, file.id, 1_050).await, "the forgery counted");

    // --- The fee forced to fail: the zap stands and is counted. ---------
    net.fail_payments_to("hitenter");
    let quote = zapper.quote(&bob_addr, 1_000).await.unwrap();
    let request = a
        .sign_unjudged(zap::request(&bob.public_key(), &file.id, 1_000_000, &relays, &quote.params.lnurl, "", false))
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    assert!(matches!(zapped.fee.await.unwrap(), Fee::Failed(_)));
    assert_eq!(net.balance_sats("bob"), 2_050);
    assert_eq!(net.balance_sats("hitenter"), 10);
    assert!(everyone_sees(&[&a, &b, &c], &n, file.id, 2_050).await);

    // Nothing in the fee payments names the message or anyone in it.
    for p in net.payments().into_iter().filter(|p| p.to == "hitenter") {
        assert!(p.zap_request.is_none());
        assert_eq!(p.comment.as_deref(), Some(hit_zap::FEE_MEMO));
    }
    serving.abort();
    for x in [a, b, c, n] {
        x.shutdown().await;
    }
}
