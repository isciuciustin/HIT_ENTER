//! HIT_ENTER's fee: **1% of a zap, on top, as a second payment from the
//! sender's own wallet** (`DESIGN.md` §11, decided 2026-09-27). The two
//! numbers live here and nowhere else.
//!
//! - The recipient gets exactly what the sender chose; the fee is never
//!   taken out of it, and HIT_ENTER is never in the zap's path.
//! - Nothing under [`FEE_MIN_SATS`]: Lightning has no fraction of a sat, and
//!   a 21-sat zap's fee would be 0.21.
//! - The zap first, the fee after, best-effort ([`crate::zap`]).
//! - A plain payment with [`FEE_MEMO`]: no zap request, no receipt, no
//!   message id, no pubkey.

/// The percentage.
pub const FEE_PERCENT: u64 = 1;
/// Zaps below this many sats carry no fee.
pub const FEE_MIN_SATS: u64 = 100;
/// The memo on every fee payment, when the address takes one — the same
/// words every time, so it says nothing about the zap.
pub const FEE_MEMO: &str = "HIT_ENTER fee";

/// **HIT_ENTER's Lightning address for fees** (owner, 2026-10-17). A
/// static LNURL-pay file on hitenter.app (`~/Work/hitenter-site`) that
/// names the owner's own wallet's callback: changing wallets is a change to
/// that file, not to the app. The wallet must pass the plain-payment check
/// (`lnurl::invoice`) — Primal's does not; Wallet of Satoshi's does.
pub const FEE_ADDRESS: Option<&str> = Some("fee@hitenter.app");

/// The fee on a zap of `amount_sats`: `floor(amount / 100)`, and none under
/// [`FEE_MIN_SATS`].
pub fn fee_for(amount_sats: u64) -> u64 {
    if amount_sats < FEE_MIN_SATS {
        0
    } else {
        amount_sats * FEE_PERCENT / 100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_percent_rounded_down_and_nothing_under_a_hundred() {
        assert_eq!(fee_for(21), 0);
        assert_eq!(fee_for(50), 0);
        assert_eq!(fee_for(99), 0);
        assert_eq!(fee_for(100), 1);
        assert_eq!(fee_for(199), 1);
        assert_eq!(fee_for(1_000), 10);
        assert_eq!(fee_for(5_000), 50);
        assert_eq!(fee_for(1_000_000), 10_000);
    }
}
