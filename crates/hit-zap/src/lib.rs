//! Zaps (M10, `docs/DESIGN.md` §11): pay a member for a message or a file,
//! from your own wallet, and see what each one was zapped.
//!
//! **The only crate with an HTTP client**, and the only one with a
//! WebSocket client: the chat plane never speaks either (CLAUDE.md). What
//! leaves the machine from here, and to whom:
//!
//! - **The recipient's Lightning address server** (HTTPS, [`lnurl`]): the
//!   zap request — sender (unless anonymous), recipient, amount, comment,
//!   the zapped event's id and the relays named for the receipt. It also
//!   sees this machine's IP. That is NIP-57, and the zap dialog says so.
//! - **The sender's wallet** (NIP-47 over its own relay, [`nwc`]): the
//!   invoices to pay, encrypted to the wallet.
//! - **The receipt relays** ([`relay`]): a subscription for the receipt.
//!   For a group, only its own anchor's `wss://` — never a public relay
//!   (`DESIGN.md` §11).
//! - **HIT_ENTER's fee address** ([`fee`]): a plain payment of the fee,
//!   with a fixed memo and nothing else — no message id, no pubkey.
//!
//! What a receipt *means* is `hit-event`'s ([`hit_event::zap`]) and
//! whether a group takes it is `hit-group`'s; this crate fetches and pays.
//!
//! [`fake`] (feature `fake`) is an in-process Lightning network for tests:
//! address servers, NWC wallets and a relay — no sats and no internet.

pub mod address;
pub mod fee;
pub mod lnurl;
pub mod nwc;
pub mod relay;
pub mod zap;

#[cfg(feature = "fake")]
pub mod fake;

pub use address::LightningAddress;
pub use fee::{FEE_ADDRESS, FEE_MEMO, FEE_MIN_SATS, FEE_PERCENT, fee_for};
pub use nwc::Wallet;
pub use zap::{Fee, Quote, Zapped, Zapper};

use std::time::Duration;

/// How long one HTTP request to an address server may take.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// The largest answer read from an address server.
pub const MAX_HTTP_BYTES: usize = 64 * 1024;

/// What went wrong, in words the app can show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("not a Lightning address: {0}")]
    Address(String),
    #[error("could not reach {0}")]
    Unreachable(String),
    #[error("the Lightning address server said: {0}")]
    Server(String),
    #[error("this Lightning address does not take zaps (no Nostr support)")]
    NotZappable,
    #[error("{0} sats is outside what this address takes ({1}–{2})")]
    Amount(u64, u64, u64),
    #[error("the invoice does not match what was asked: {0}")]
    BadInvoice(&'static str),
    #[error("not a wallet connection string")]
    BadConnection,
    #[error("the wallet said: {0}")]
    Wallet(String),
    #[error("the wallet did not answer in time — the payment may still go through")]
    WalletTimeout,
    #[error("relay {0}: {1}")]
    Relay(String, String),
    #[error("no receipt arrived in time")]
    NoReceipt,
}

/// rustls needs a process-wide crypto provider when, as here, the crates
/// using it are built without one: iroh's `ring`. Installing it twice is
/// harmless.
pub(crate) fn tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Is `host` (with or without a port) this machine? The one place plain
/// `http://` and `ws://` are allowed: tests and a fake network.
pub(crate) fn is_loopback(host: &str) -> bool {
    let h = host
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(host, |(h, _)| h);
    let h = h.trim_start_matches('[').trim_end_matches(']');
    h == "localhost"
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}
