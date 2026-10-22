//! Nostr Wallet Connect (NIP-47): ask the person's own wallet to pay an
//! invoice. The connection string is a secret — it can spend up to the
//! budget the wallet gave it — so it lives in the OS keychain like the
//! nsec, is never logged, and [`Wallet`]'s `Debug` prints none of it.

use std::{fmt, time::Duration};

use nostr::prelude::{
    Event, Filter, Kind,
    nip47::{Nip47Ciphers, NostrWalletConnectUri, PayInvoiceRequest, Request, Response},
};

use crate::{
    Error,
    relay::{Relay, sub_id},
};

/// How long a payment may take. Lightning payments settle in seconds; a
/// wallet that has not answered by then may still pay, and the error says
/// so.
pub const PAY_TIMEOUT: Duration = Duration::from_secs(60);
const INFO_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Wallet {
    uri: NostrWalletConnectUri,
}

impl fmt::Debug for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wallet")
            .field("wallet", &self.uri.public_key.to_hex())
            .finish_non_exhaustive()
    }
}

impl Wallet {
    /// Parses a `nostr+walletconnect://…` string.
    pub fn parse(connection: &str) -> Result<Self, Error> {
        NostrWalletConnectUri::parse(connection.trim())
            .map(|uri| Self { uri })
            .map_err(|_| Error::BadConnection)
    }

    /// The wallet's relays, for the app to show where payments go.
    pub fn relays(&self) -> Vec<String> {
        self.uri.relays.iter().map(|r| r.to_string()).collect()
    }

    /// Pays `bolt11`; the payment's preimage on success.
    pub async fn pay(&self, bolt11: &str) -> Result<String, Error> {
        let mut last = Error::Relay(String::new(), "no relay".into());
        for url in &self.uri.relays {
            let mut relay = match Relay::connect(url.as_str()).await {
                Ok(r) => r,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            return self.pay_on(&mut relay, bolt11).await;
        }
        Err(last)
    }

    async fn pay_on(&self, relay: &mut Relay, bolt11: &str) -> Result<String, Error> {
        let cipher = self.cipher(relay).await;
        let request = Request::pay_invoice(PayInvoiceRequest::new(bolt11))
            .to_event(&self.uri, cipher)
            .map_err(|_| Error::BadConnection)?;
        let sub = sub_id();
        relay
            .subscribe(
                &sub,
                Filter::new()
                    .kind(Kind::WalletConnectResponse)
                    .author(self.uri.public_key)
                    .event(request.id),
            )
            .await?;
        relay.publish(&request, PUBLISH_TIMEOUT).await?;
        let answer = tokio::time::timeout(PAY_TIMEOUT, async {
            loop {
                if let Some(ev) = relay.next_event(&sub, false).await?
                    && let Ok(r) = Response::from_event(&self.uri, &ev, cipher)
                {
                    return Ok::<_, Error>(r);
                }
            }
        })
        .await
        .map_err(|_| Error::WalletTimeout)??;
        relay.close(&sub).await;
        if let Some(e) = answer.error {
            return Err(Error::Wallet(e.message));
        }
        answer
            .to_pay_invoice()
            .map(|r| r.preimage)
            .map_err(|_| Error::Wallet("an answer that is not a payment".into()))
    }

    /// The encryption the wallet says it speaks (its kind 13194 info
    /// event): NIP-44 if it lists it, else NIP-04 — NIP-47's default for a
    /// wallet that says nothing.
    async fn cipher(&self, relay: &mut Relay) -> Nip47Ciphers {
        let sub = sub_id();
        let info: Option<Event> = async {
            relay
                .subscribe(
                    &sub,
                    Filter::new()
                        .kind(Kind::WalletConnectInfo)
                        .author(self.uri.public_key)
                        .limit(1),
                )
                .await
                .ok()?;
            tokio::time::timeout(INFO_TIMEOUT, relay.next_event(&sub, true))
                .await
                .ok()?
                .ok()?
        }
        .await;
        relay.close(&sub).await;
        let nip44 = info.is_some_and(|ev| {
            ev.tags.iter().any(|t| match t.as_slice() {
                [n, v, ..] if n == "encryption" => v.split_whitespace().any(|c| c == "nip44_v2"),
                _ => false,
            })
        });
        if nip44 {
            Nip47Ciphers::NIP44V2
        } else {
            Nip47Ciphers::NIP04
        }
    }
}
