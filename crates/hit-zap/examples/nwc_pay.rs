//! Pays one bolt11 invoice through an NWC connection — a phone wallet
//! scanning the app's QR code, when driving the windows against the fake
//! network (`docs/TESTING.md` §7a).
//!
//! ```text
//! cargo run -p hit-zap --example nwc_pay -- '<nostr+walletconnect://…>' <bolt11>
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(nwc), Some(bolt11)) = (args.next(), args.next()) else {
        eprintln!("usage: nwc_pay <nostr+walletconnect://…> <bolt11>");
        std::process::exit(2);
    };
    let wallet = hit_zap::Wallet::parse(&nwc).expect("a connection string");
    match wallet.pay(&bolt11).await {
        Ok(_) => println!("paid"),
        Err(e) => {
            eprintln!("not paid: {e}");
            std::process::exit(1);
        }
    }
}
