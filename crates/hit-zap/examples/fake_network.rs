//! The fake Lightning network (`hit_zap::fake`) as a process of its own,
//! for driving the app's windows against it (`docs/TESTING.md` §7a): it
//! opens the accounts named on the command line (`name=sats`), prints each
//! one's Lightning address and an NWC string for it, then serves until
//! killed, printing every payment it settles.
//!
//! ```text
//! cargo run -p hit-zap --features fake --example fake_network -- alice=10000 bob=0 hitenter=0
//! ```
//!
//! No sats and no internet: loopback only, invoices nobody else would pay.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use hit_zap::fake::FakeLightning;

#[tokio::main]
async fn main() {
    let net = FakeLightning::start().await;
    println!("relay {}", net.relay_url());
    for arg in std::env::args().skip(1) {
        let (name, sats) = arg.split_once('=').expect("name=sats");
        let address = net.account(name, sats.parse().expect("sats"));
        println!("address {name} {address}");
        println!("nwc {name} {}", net.connect_wallet(name));
    }
    let mut seen = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let payments = net.payments();
        for p in &payments[seen..] {
            println!(
                "paid {} -> {} {} sats{}{}",
                p.from,
                p.to,
                p.msat / 1000,
                if p.zap_request.is_some() { " (zap)" } else { "" },
                p.comment.as_deref().map(|c| format!(" memo {c:?}")).unwrap_or_default()
            );
        }
        seen = payments.len();
    }
}
