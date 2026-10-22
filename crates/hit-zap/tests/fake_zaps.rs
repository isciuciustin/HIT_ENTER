//! Zaps end to end against the fake network (no sats, no internet): the
//! quote, the zap paid through NWC, the receipt fetched and checked, and
//! the 1% fee exactly as `DESIGN.md` §11 has it — on top, a plain payment
//! with a memo that names nothing, none under 100 sats, and best-effort: a
//! fee that cannot be paid leaves the zap delivered.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use hit_event::{Person, zap};
use hit_zap::{Error, Fee, LightningAddress, Wallet, Zapper, fake::FakeLightning};
use nostr::prelude::{EventId, FinalizeEvent, Keys};

fn target() -> EventId {
    EventId::from_byte_array([42; 32])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zap_arrives_whole_and_the_fee_rides_on_top() {
    let net = FakeLightning::start().await;
    let (alice, bob) = (Person::generate(), Person::generate());
    net.account("alice", 10_000);
    let bob_addr: LightningAddress = net.account("bob", 0).parse().unwrap();
    let fee_addr: LightningAddress = net.account("hitenter", 0).parse().unwrap();
    let wallet = Wallet::parse(&net.connect_wallet("alice")).unwrap();
    let zapper = Zapper::new(Some(fee_addr))
        .unwrap()
        .with_timeouts(Duration::from_millis(200), Duration::from_secs(5));

    // --- 1,000 sats: 1,000 to Bob, 10 to HIT_ENTER, 1,010 from Alice. ----
    let quote = zapper.quote(&bob_addr, 1_000).await.unwrap();
    assert_eq!((quote.amount_sats, quote.fee_sats, quote.total_sats()), (1_000, 10, 1_010));
    assert_eq!(quote.provider, net.provider());
    let request = alice
        .sign(zap::request(
            &bob.public_key(),
            &target(),
            1_000_000,
            &[net.relay_url()],
            &quote.params.lnurl,
            "great file",
            false,
        ))
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    let (receipt, z) = zapped.receipt.expect("a receipt");
    assert_eq!(z.amount_msat, 1_000_000);
    assert_eq!(z.sender, Some(alice.public_key()));
    assert_eq!((z.recipient, z.target), (bob.public_key(), target()));
    assert_eq!(z.comment, "great file");
    assert_eq!(zap::check_receipt(&receipt).unwrap(), z);
    assert_eq!(zapped.fee.await.unwrap(), Fee::Paid);
    assert_eq!(net.balance_sats("bob"), 1_000, "the zap arrives whole");
    assert_eq!(net.balance_sats("hitenter"), 10);
    assert_eq!(net.balance_sats("alice"), 10_000 - 1_010);

    // The fee payment names nothing: a plain payment, the fixed memo, no
    // zap request — so no message id and no pubkey went with it.
    let fee = net
        .payments()
        .into_iter()
        .find(|p| p.to == "hitenter")
        .unwrap();
    assert_eq!(fee.msat, 10_000);
    assert_eq!(fee.comment.as_deref(), Some(hit_zap::FEE_MEMO));
    assert!(fee.zap_request.is_none());
    let memo = fee.comment.unwrap();
    for secret in [
        target().to_hex(),
        alice.public_key().to_hex(),
        bob.public_key().to_hex(),
    ] {
        assert!(!memo.contains(&secret));
    }

    // --- 50 sats: no fee. ------------------------------------------------
    let quote = zapper.quote(&bob_addr, 50).await.unwrap();
    assert_eq!(quote.fee_sats, 0);
    let request = alice
        .sign(zap::request(&bob.public_key(), &target(), 50_000, &[net.relay_url()], &quote.params.lnurl, "", false))
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    assert!(zapped.receipt.is_some());
    assert_eq!(zapped.fee.await.unwrap(), Fee::None);
    assert_eq!(net.balance_sats("hitenter"), 10, "nothing charged under 100");
    assert_eq!(net.balance_sats("bob"), 1_050);

    // --- The fee fails (no route, twice): the zap stands. ----------------
    net.fail_payments_to("hitenter");
    let quote = zapper.quote(&bob_addr, 1_000).await.unwrap();
    let request = alice
        .sign(zap::request(&bob.public_key(), &target(), 1_000_000, &[net.relay_url()], &quote.params.lnurl, "", false))
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    assert!(zapped.receipt.is_some(), "the zap's receipt still arrives");
    assert!(matches!(zapped.fee.await.unwrap(), Fee::Failed(_)));
    assert_eq!(net.balance_sats("bob"), 2_050, "delivered");
    assert_eq!(net.balance_sats("hitenter"), 10, "and no fee");
    net.heal("hitenter");

    // --- An anonymous zap: a throwaway key, no sender. -------------------
    let quote = zapper.quote(&bob_addr, 21).await.unwrap();
    let request = zap::request(&bob.public_key(), &target(), 21_000, &[net.relay_url()], &quote.params.lnurl, "", true)
        .finalize(&Keys::generate())
        .unwrap();
    let zapped = zapper.send(&wallet, &quote, &request).await.unwrap();
    assert_eq!(zapped.receipt.unwrap().1.sender, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_paid_when_the_zap_cannot_be() {
    let net = FakeLightning::start().await;
    let (carol, bob) = (Person::generate(), Person::generate());
    net.account("carol", 5);
    let bob_addr: LightningAddress = net.account("bob", 0).parse().unwrap();
    let fee_addr: LightningAddress = net.account("hitenter", 0).parse().unwrap();
    let wallet = Wallet::parse(&net.connect_wallet("carol")).unwrap();
    let zapper = Zapper::new(Some(fee_addr))
        .unwrap()
        .with_timeouts(Duration::from_millis(200), Duration::from_secs(2));

    // Carol's wallet has 5 sats: the zap fails, and no fee is attempted.
    let quote = zapper.quote(&bob_addr, 1_000).await.unwrap();
    let request = carol
        .sign(zap::request(&bob.public_key(), &target(), 1_000_000, &[net.relay_url()], &quote.params.lnurl, "", false))
        .unwrap();
    let err = zapper.send(&wallet, &quote, &request).await.unwrap_err();
    assert!(matches!(err, Error::Wallet(_)), "{err}");
    assert!(net.payments().is_empty());

    // A server that bills more than was asked is caught before paying.
    net.account("carol", 100_000);
    net.overcharge("bob", 1);
    let err = zapper.send(&wallet, &quote, &request).await.unwrap_err();
    assert_eq!(err, Error::BadInvoice("wrong amount"));
    assert!(net.payments().is_empty());

    // An amount the address does not take, and something not an address.
    assert!(matches!(zapper.quote(&bob_addr, 0).await, Err(Error::Amount(..))));
    assert!("not an address".parse::<LightningAddress>().is_err());
    assert_eq!(Wallet::parse("nostr+walletconnect://nope").unwrap_err(), Error::BadConnection);
}
