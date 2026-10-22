//! LNURL-pay (LUD-06, LUD-12, LUD-16) with NIP-57's additions: ask a
//! Lightning address what it takes, then ask it for an invoice — and check
//! the invoice is for what was asked **before** anything pays it.

use hit_event::zap::{Invoice, lnurl_encode};
use nostr::prelude::{Event, PublicKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Error, HTTP_TIMEOUT, LightningAddress, MAX_HTTP_BYTES, is_loopback, tls};

/// What a Lightning address takes, from its LNURL-pay endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayParams {
    pub address: LightningAddress,
    callback: reqwest::Url,
    pub min_msat: u64,
    pub max_msat: u64,
    /// NIP-57: the key the server signs receipts with — `None` if it does
    /// not take zaps (a plain payment still works: the fee).
    pub nostr_pubkey: Option<PublicKey>,
    /// LUD-12: the longest comment it takes; 0 for none.
    pub comment_allowed: usize,
    metadata: String,
    /// The address's URL, bech32-encoded: NIP-57's `lnurl` tag.
    pub lnurl: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParamsJson {
    tag: Option<String>,
    callback: Option<String>,
    min_sendable: Option<u64>,
    max_sendable: Option<u64>,
    metadata: Option<String>,
    #[serde(default)]
    allows_nostr: bool,
    nostr_pubkey: Option<String>,
    #[serde(default)]
    comment_allowed: usize,
    status: Option<String>,
    reason: Option<String>,
}

#[derive(Deserialize)]
struct InvoiceJson {
    pr: Option<String>,
    status: Option<String>,
    reason: Option<String>,
}

/// An HTTP client for address servers: a timeout, no redirects (a redirect
/// is somewhere the address did not name), no cookies, a plain user agent.
#[derive(Debug, Clone)]
pub struct Http(reqwest::Client);

impl Http {
    pub fn new() -> Result<Self, Error> {
        tls();
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("HIT_ENTER")
            .build()
            .map(Self)
            .map_err(|e| Error::Unreachable(e.to_string()))
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, url: reqwest::Url) -> Result<T, Error> {
        let host = url.host_str().unwrap_or("").to_string();
        let mut resp = self
            .0
            .get(url)
            .send()
            .await
            .map_err(|_| Error::Unreachable(host.clone()))?;
        let mut body = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|_| Error::Unreachable(host.clone()))?
        {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_HTTP_BYTES {
                return Err(Error::Server("answer too large".into()));
            }
        }
        serde_json::from_slice(&body).map_err(|_| Error::Server(format!("{host} did not answer with LNURL")))
    }
}

/// Asks `address` what it takes.
pub async fn pay_params(http: &Http, address: &LightningAddress) -> Result<PayParams, Error> {
    let url = reqwest::Url::parse(&address.url()).map_err(|_| Error::Address(address.to_string()))?;
    let p: ParamsJson = http.get(url).await?;
    if p.status.as_deref() == Some("ERROR") {
        return Err(Error::Server(p.reason.unwrap_or_default()));
    }
    if p.tag.as_deref() != Some("payRequest") {
        return Err(Error::Server("not a pay request".into()));
    }
    let callback = p
        .callback
        .and_then(|c| reqwest::Url::parse(&c).ok())
        .ok_or_else(|| Error::Server("no callback".into()))?;
    let local = is_loopback(address.domain());
    let callback_ok = match callback.scheme() {
        "https" => !callback.host_str().is_some_and(is_loopback) || local,
        "http" => local && callback.host_str().is_some_and(is_loopback),
        _ => false,
    };
    if !callback_ok {
        return Err(Error::Server("callback is not https".into()));
    }
    let (min, max) = (p.min_sendable.unwrap_or(1_000), p.max_sendable.unwrap_or(0));
    if min == 0 || max < min {
        return Err(Error::Server("no amount it takes".into()));
    }
    Ok(PayParams {
        lnurl: lnurl_encode(&address.url()),
        address: address.clone(),
        callback,
        min_msat: min,
        max_msat: max,
        nostr_pubkey: if p.allows_nostr {
            p.nostr_pubkey.and_then(|k| PublicKey::from_hex(&k).ok())
        } else {
            None
        },
        comment_allowed: p.comment_allowed,
        metadata: p.metadata.unwrap_or_default(),
    })
}

/// Asks for an invoice of `amount_msat`: a zap's, carrying `zap_request`,
/// or a plain payment with an optional LUD-12 `comment` (the fee's memo).
/// The invoice must be for exactly the amount; otherwise nothing pays it.
/// A plain payment's must commit to the metadata. A zap's should commit to
/// the zap request (NIP-57), but Primal's does not (2026-10-17: one fixed
/// hash whatever the request) — refusing it refused zaps to every Primal
/// user, so a zap invoice that does not commit is paid, and logged.
pub async fn invoice(
    http: &Http,
    params: &PayParams,
    amount_msat: u64,
    zap_request: Option<&Event>,
    comment: Option<&str>,
) -> Result<(String, Invoice), Error> {
    if amount_msat < params.min_msat || amount_msat > params.max_msat {
        return Err(Error::Amount(
            amount_msat / 1000,
            params.min_msat.div_ceil(1000),
            params.max_msat / 1000,
        ));
    }
    let mut url = params.callback.clone();
    let description = zap_request.map(|z| z.as_json());
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("amount", &amount_msat.to_string());
        if let Some(d) = &description {
            q.append_pair("nostr", d);
            q.append_pair("lnurl", &params.lnurl);
        }
        if let Some(c) = comment.filter(|c| !c.is_empty() && c.chars().count() <= params.comment_allowed) {
            q.append_pair("comment", c);
        }
    }
    let r: InvoiceJson = http.get(url).await?;
    if r.status.as_deref() == Some("ERROR") {
        return Err(Error::Server(r.reason.unwrap_or_default()));
    }
    let pr = r.pr.ok_or_else(|| Error::Server("no invoice".into()))?;
    let inv = Invoice::decode(&pr).map_err(|_| Error::BadInvoice("unreadable"))?;
    if inv.currency != "bc" && !is_loopback(params.address.domain()) {
        return Err(Error::BadInvoice("not a mainnet invoice"));
    }
    if inv.amount_msat != Some(amount_msat) {
        return Err(Error::BadInvoice("wrong amount"));
    }
    let committed = description.as_deref().unwrap_or(&params.metadata);
    let hash: [u8; 32] = Sha256::digest(committed.as_bytes()).into();
    // LUD-06 asks the metadata's hash; some servers put a plain description
    // on a non-zap invoice instead.
    if inv.description_hash != Some(hash) {
        if description.is_some() {
            tracing::warn!("a zap invoice from {} does not commit to the zap request; paying it anyway", params.callback);
        } else if inv.description_hash.is_some() {
            return Err(Error::BadInvoice("does not commit to the metadata"));
        }
    }
    Ok((pr, inv))
}
