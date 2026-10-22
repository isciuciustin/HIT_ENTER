//! A zap, start to finish (`DESIGN.md` §11):
//!
//! 1. [`Zapper::quote`] — ask the recipient's Lightning address what it
//!    takes; the amount, the fee and the total, before anything is paid.
//! 2. The caller signs the zap request (the person's key is the node's,
//!    never this crate's) — [`hit_event::zap::request`].
//! 3. [`Zapper::send`] — fetch the invoice and check it, open the receipt
//!    subscription, pay through the wallet, then the fee (best-effort),
//!    then wait for the receipt and check it against the provider key the
//!    quote saw.
//!
//! Without a wallet connection: [`Zapper::invoice`] and
//! [`Zapper::fee_invoice`] for QR codes, and [`await_receipt`] after.

use std::time::Duration;

use hit_event::zap::{Zap, check_receipt};
use nostr::prelude::{Event, Filter, Kind, PublicKey, Timestamp};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    Error, FEE_MEMO, LightningAddress, Wallet, fee_for,
    lnurl::{self, Http, PayParams},
    relay::{Relay, sub_id},
};

/// How long to wait for a receipt after paying. A provider publishes it
/// within seconds; past this the zap has still happened, its total just
/// arrives later — by the anchor, or not at all.
pub const RECEIPT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long before the fee's one retry (DESIGN.md §11: retried once, then
/// dropped).
pub const FEE_RETRY: Duration = Duration::from_secs(60);

/// What a zap will cost, before anything is paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub params: PayParams,
    /// The key the recipient's provider signs receipts with.
    pub provider: PublicKey,
    pub amount_sats: u64,
    /// HIT_ENTER's fee, paid separately on top; 0 under the threshold or
    /// with no fee address.
    pub fee_sats: u64,
}

impl Quote {
    pub fn total_sats(&self) -> u64 {
        self.amount_sats + self.fee_sats
    }
}

/// What became of the fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fee {
    /// None owed: under the threshold, or no fee address.
    None,
    Paid,
    /// Failed twice; dropped. The zap stands.
    Failed(String),
}

/// A zap that was paid.
#[derive(Debug)]
pub struct Zapped {
    pub preimage: String,
    /// The receipt, checked — `None` if none arrived in time; the zap was
    /// still paid.
    pub receipt: Option<(Event, Zap)>,
    /// The fee, being paid after the zap. Never awaited before the zap is
    /// reported: the recipient's money does not wait on ours.
    pub fee: JoinHandle<Fee>,
}

#[derive(Debug, Clone)]
pub struct Zapper {
    http: Http,
    fee_to: Option<LightningAddress>,
    fee_retry: Duration,
    receipt_timeout: Duration,
}

impl Zapper {
    /// `fee_to` is HIT_ENTER's fee address ([`crate::FEE_ADDRESS`]); `None`
    /// charges no fee.
    pub fn new(fee_to: Option<LightningAddress>) -> Result<Self, Error> {
        Ok(Self {
            http: Http::new()?,
            fee_to,
            fee_retry: FEE_RETRY,
            receipt_timeout: RECEIPT_TIMEOUT,
        })
    }

    /// Shorter waits, for tests.
    pub fn with_timeouts(mut self, fee_retry: Duration, receipt: Duration) -> Self {
        self.fee_retry = fee_retry;
        self.receipt_timeout = receipt;
        self
    }

    /// The key `address`'s server signs zap receipts with — what a person's
    /// profile names when they set their address, so every member can check
    /// their receipts. An address that takes no zaps is refused.
    pub async fn provider_of(&self, address: &LightningAddress) -> Result<PublicKey, Error> {
        lnurl::pay_params(&self.http, address)
            .await?
            .nostr_pubkey
            .ok_or(Error::NotZappable)
    }

    /// What zapping `amount_sats` to `address` costs. Refuses an address
    /// that does not take zaps — a payment without a receipt would show
    /// nowhere — and an amount it does not take.
    pub async fn quote(&self, address: &LightningAddress, amount_sats: u64) -> Result<Quote, Error> {
        let params = lnurl::pay_params(&self.http, address).await?;
        let provider = params.nostr_pubkey.ok_or(Error::NotZappable)?;
        let msat = amount_sats.saturating_mul(1000);
        if amount_sats == 0 || msat < params.min_msat || msat > params.max_msat {
            return Err(Error::Amount(
                amount_sats,
                params.min_msat.div_ceil(1000),
                params.max_msat / 1000,
            ));
        }
        let fee_sats = if self.fee_to.is_some() { fee_for(amount_sats) } else { 0 };
        Ok(Quote {
            params,
            provider,
            amount_sats,
            fee_sats,
        })
    }

    /// The zap's invoice for `zap_request`, checked: the amount, and its
    /// commitment to the request.
    pub async fn invoice(&self, quote: &Quote, zap_request: &Event) -> Result<String, Error> {
        lnurl::invoice(
            &self.http,
            &quote.params,
            quote.amount_sats * 1000,
            Some(zap_request),
            None,
        )
        .await
        .map(|(pr, _)| pr)
    }

    /// The fee's invoice: a plain payment to HIT_ENTER's address with
    /// [`FEE_MEMO`] — no zap request, no message id, no pubkey.
    pub async fn fee_invoice(&self, fee_sats: u64) -> Result<String, Error> {
        let to = self.fee_to.as_ref().ok_or(Error::Address("no fee address".into()))?;
        let params = lnurl::pay_params(&self.http, to).await?;
        lnurl::invoice(&self.http, &params, fee_sats * 1000, None, Some(FEE_MEMO))
            .await
            .map(|(pr, _)| pr)
    }

    /// Pays the zap through `wallet`, then the fee, then waits for the
    /// receipt on `zap_request`'s relays. An error means the zap was not
    /// paid — and so no fee was either.
    pub async fn send(&self, wallet: &Wallet, quote: &Quote, zap_request: &Event) -> Result<Zapped, Error> {
        let bolt11 = self.invoice(quote, zap_request).await?;
        // Subscribed before paying, so a receipt published at once is not
        // missed by a relay that does not keep it.
        let watch = Watch::open(&relays_of(zap_request), zap_request, quote.provider).await;
        let preimage = wallet.pay(&bolt11).await?;
        let fee = self.spawn_fee(wallet.clone(), quote.fee_sats);
        let receipt = watch.wait(self.receipt_timeout).await;
        Ok(Zapped {
            preimage,
            receipt,
            fee,
        })
    }

    /// Pays `fee_sats` best-effort: once now, once after
    /// [`FEE_RETRY`], then dropped.
    fn spawn_fee(&self, wallet: Wallet, fee_sats: u64) -> JoinHandle<Fee> {
        let this = self.clone();
        tokio::spawn(async move {
            if fee_sats == 0 || this.fee_to.is_none() {
                return Fee::None;
            }
            let attempt = || async {
                let pr = this.fee_invoice(fee_sats).await?;
                wallet.pay(&pr).await
            };
            match attempt().await {
                Ok(_) => Fee::Paid,
                Err(first) => {
                    tracing::info!("fee payment failed ({first}); retrying once");
                    tokio::time::sleep(this.fee_retry).await;
                    match attempt().await {
                        Ok(_) => Fee::Paid,
                        Err(e) => {
                            tracing::info!("fee payment failed again ({e}); dropped");
                            Fee::Failed(e.to_string())
                        }
                    }
                }
            }
        })
    }
}

/// The relays a zap request names for its receipt.
pub fn relays_of(zap_request: &Event) -> Vec<String> {
    zap_request
        .tags
        .iter()
        .find_map(|t| match t.as_slice() {
            [n, rest @ ..] if n == "relays" => Some(rest.to_vec()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Waits up to `timeout` for the receipt of `zap_request` on its relays,
/// signed by `provider` — for a zap paid some other way (a QR code).
pub async fn await_receipt(
    zap_request: &Event,
    provider: PublicKey,
    timeout: Duration,
) -> Option<(Event, Zap)> {
    Watch::open(&relays_of(zap_request), zap_request, provider)
        .await
        .wait(timeout)
        .await
}

/// A receipt subscription on every relay a request names, the first valid
/// receipt winning.
struct Watch {
    rx: mpsc::Receiver<(Event, Zap)>,
    tasks: Vec<JoinHandle<()>>,
}

impl Watch {
    async fn open(relays: &[String], request: &Event, provider: PublicKey) -> Self {
        let (tx, rx) = mpsc::channel(4);
        let mut tasks = Vec::new();
        let target = hit_event::group::targets_of(request).first().copied();
        let recipient = request.tags.public_keys().next();
        let (Some(target), Some(recipient)) = (target, recipient) else {
            return Self { rx, tasks };
        };
        let filter = Filter::new()
            .kind(Kind::ZapReceipt)
            .author(provider)
            .pubkey(recipient)
            .event(target)
            .since(Timestamp::from(request.created_at.as_secs().saturating_sub(600)));
        for url in relays {
            let Ok(mut relay) = Relay::connect(url).await else {
                tracing::info!("receipt relay {url} unreachable");
                continue;
            };
            let sub = sub_id();
            if relay.subscribe(&sub, filter.clone()).await.is_err() {
                continue;
            }
            let (tx, request_id) = (tx.clone(), request.id);
            tasks.push(tokio::spawn(async move {
                while let Ok(Some(ev)) = relay.next_event(&sub, false).await {
                    match check_receipt(&ev) {
                        Ok(zap) if zap.request == request_id && zap.provider == provider => {
                            let _ = tx.send((ev, zap)).await;
                            return;
                        }
                        _ => {}
                    }
                }
            }));
        }
        Self { rx, tasks }
    }

    async fn wait(mut self, timeout: Duration) -> Option<(Event, Zap)> {
        let got = tokio::time::timeout(timeout, self.rx.recv()).await.ok().flatten();
        for t in &self.tasks {
            t.abort();
        }
        got
    }
}
