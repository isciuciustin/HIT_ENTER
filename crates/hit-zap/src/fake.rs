//! An in-process Lightning network, for tests (feature `fake`): one HTTP
//! server on loopback that is
//!
//! - **a Lightning address server** for every account (`name@127.0.0.1:port`)
//!   with NIP-57 support — it issues invoices that commit to the zap
//!   request and, once one is paid, signs a receipt and publishes it to
//!   every relay the request names (its own in-process, others over
//!   WebSocket, as a real one does);
//! - **a Nostr relay** at `/relay`, which keeps every event and streams new
//!   ones;
//! - **a wallet service** for every NWC connection it hands out: requests
//!   that reach the relay are decrypted and paid from the account's
//!   balance, and the answer published back.
//!
//! Invoices are real bolt11 strings with a zero signature
//! ([`Invoice::encode_unsigned`]) — nothing outside this module would pay
//! one. Payments to an account can be made to fail, which is how the fee's
//! best-effort path is tested.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use hit_event::zap::Invoice;
use nostr::prelude::{
    ClientMessage, Event, EventBuilder, Filter, FinalizeEvent, IntoEventBuilder, Keys, Kind,
    MatchEventOptions, PublicKey, RelayMessage, Tag, Timestamp,
    nip04, nip44,
    nip47::{NostrWalletConnectUri, Request, RequestParams},
    nip57::ZapReceipt,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

/// A payment the fake network settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payment {
    pub from: String,
    pub to: String,
    pub msat: u64,
    /// LUD-12's comment on a plain payment.
    pub comment: Option<String>,
    /// The zap request, for a zap.
    pub zap_request: Option<Event>,
}

struct Pending {
    to: String,
    msat: u64,
    bolt11: String,
    zap_request: Option<Event>,
    comment: Option<String>,
    paid: bool,
}

struct Ledger {
    balances: HashMap<String, u64>,
    invoices: HashMap<[u8; 32], Pending>,
    /// Wallet service key → (its keys, the account it spends from).
    wallets: HashMap<PublicKey, (Keys, String)>,
    events: Vec<Event>,
    payments: Vec<Payment>,
    failing: HashSet<String>,
    /// Extra msat an account's server puts on every invoice: a lying server.
    overcharge: HashMap<String, u64>,
    next_hash: u64,
}

struct Inner {
    addr: SocketAddr,
    /// The key this network's address server signs receipts with.
    provider: Keys,
    ledger: Mutex<Ledger>,
    live: broadcast::Sender<Event>,
}

/// A running fake network. Dropping it does not stop the server; it lives
/// as long as the test's runtime.
#[derive(Clone)]
pub struct FakeLightning(Arc<Inner>);

impl FakeLightning {
    pub async fn start() -> Self {
        #[allow(clippy::expect_used)]
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        #[allow(clippy::expect_used)]
        let addr = listener.local_addr().expect("local addr");
        let inner = Arc::new(Inner {
            addr,
            provider: Keys::generate(),
            ledger: Mutex::new(Ledger {
                balances: HashMap::new(),
                invoices: HashMap::new(),
                wallets: HashMap::new(),
                events: Vec::new(),
                payments: Vec::new(),
                failing: HashSet::new(),
                overcharge: HashMap::new(),
                next_hash: 1,
            }),
            live: broadcast::channel(256).0,
        });
        let app = Router::new()
            .route("/.well-known/lnurlp/{name}", get(params))
            .route("/lnurlp/{name}/callback", get(callback))
            .route("/relay", get(relay))
            .with_state(inner.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self(inner)
    }

    /// Opens an account with `sats`; its Lightning address.
    pub fn account(&self, name: &str, sats: u64) -> String {
        self.ledger()
            .balances
            .insert(name.to_string(), sats * 1000);
        format!("{name}@{}", self.0.addr)
    }

    /// An NWC connection string that spends from `name`'s account.
    pub fn connect_wallet(&self, name: &str) -> String {
        let service = Keys::generate();
        let app = Keys::generate();
        let info = EventBuilder::new(Kind::WalletConnectInfo, "pay_invoice")
            .tag(Tag::custom("encryption", ["nip44_v2 nip04"]))
            .finalize(&service);
        let mut l = self.ledger();
        if let Ok(info) = info {
            l.events.push(info);
        }
        l.wallets
            .insert(service.public_key(), (service.clone(), name.to_string()));
        #[allow(clippy::expect_used)]
        let relay = nostr::prelude::RelayUrl::parse(&self.relay_url()).expect("relay url");
        NostrWalletConnectUri::new(service.public_key(), vec![relay], app.secret_key().clone(), None)
            .to_string()
    }

    pub fn relay_url(&self) -> String {
        format!("ws://{}/relay", self.0.addr)
    }

    /// The key receipts from this network are signed with.
    pub fn provider(&self) -> PublicKey {
        self.0.provider.public_key()
    }

    pub fn balance_sats(&self, name: &str) -> u64 {
        self.ledger().balances.get(name).copied().unwrap_or(0) / 1000
    }

    /// Every payment settled so far, in order.
    pub fn payments(&self) -> Vec<Payment> {
        self.ledger().payments.clone()
    }

    /// Payments to `name` fail with "no route" until [`Self::heal`].
    pub fn fail_payments_to(&self, name: &str) {
        self.ledger().failing.insert(name.to_string());
    }

    pub fn heal(&self, name: &str) {
        self.ledger().failing.remove(name);
    }

    /// `name`'s address server adds `extra_sats` to every invoice it
    /// issues — what a client must catch before it pays.
    pub fn overcharge(&self, name: &str, extra_sats: u64) {
        self.ledger()
            .overcharge
            .insert(name.to_string(), extra_sats * 1000);
    }

    /// Events the relay holds of `kind`.
    pub fn events_of(&self, kind: Kind) -> Vec<Event> {
        self.ledger()
            .events
            .iter()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }

    /// Publishes `event` on the relay, as anyone could.
    pub fn publish(&self, event: Event) {
        publish(&self.0, event);
    }

    #[allow(clippy::expect_used)]
    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.0.ledger.lock().expect("ledger")
    }
}

#[allow(clippy::expect_used)]
fn ledger(inner: &Inner) -> std::sync::MutexGuard<'_, Ledger> {
    inner.ledger.lock().expect("ledger")
}

fn publish(inner: &Inner, event: Event) {
    ledger(inner).events.push(event.clone());
    let _ = inner.live.send(event);
}

const METADATA: &str = "[[\"text/plain\",\"a fake Lightning account\"]]";

async fn params(State(inner): State<Arc<Inner>>, Path(name): Path<String>) -> Json<Value> {
    if !ledger(&inner).balances.contains_key(&name) {
        return Json(json!({"status": "ERROR", "reason": "no such account"}));
    }
    Json(json!({
        "tag": "payRequest",
        "callback": format!("http://{}/lnurlp/{name}/callback", inner.addr),
        "minSendable": 1_000,
        "maxSendable": 100_000_000_000u64,
        "metadata": METADATA,
        "allowsNostr": true,
        "nostrPubkey": inner.provider.public_key().to_hex(),
        "commentAllowed": 140,
    }))
}

async fn callback(
    State(inner): State<Arc<Inner>>,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let err = |r: &str| Json(json!({"status": "ERROR", "reason": r}));
    let Some(msat) = q.get("amount").and_then(|a| a.parse::<u64>().ok()) else {
        return err("no amount");
    };
    let zap_request = match q.get("nostr") {
        None => None,
        Some(json) => {
            // NIP-57 Appendix D: a valid signature, kind 9734, one `p`, and
            // the amount, if given, the same.
            let Ok(ev) = Event::from_json(json) else {
                return err("bad zap request");
            };
            let amount_ok = ev.tags.iter().all(|t| match t.as_slice() {
                [n, v, ..] if n == "amount" => v.parse::<u64>().ok() == Some(msat),
                _ => true,
            });
            if ev.kind != Kind::ZapRequest || ev.verify().is_err() || !amount_ok {
                return err("bad zap request");
            }
            Some(ev)
        }
    };
    let description = zap_request
        .as_ref()
        .map_or_else(|| METADATA.to_string(), |e| e.as_json());
    let mut l = ledger(&inner);
    if !l.balances.contains_key(&name) {
        return err("no such account");
    }
    let msat = msat + l.overcharge.get(&name).copied().unwrap_or(0);
    let mut hash = [0u8; 32];
    hash[..8].copy_from_slice(&l.next_hash.to_be_bytes());
    l.next_hash += 1;
    let bolt11 = Invoice {
        currency: "bcrt".into(),
        amount_msat: Some(msat),
        timestamp: Timestamp::now().as_secs(),
        payment_hash: Some(hash),
        description_hash: Some(Sha256::digest(description.as_bytes()).into()),
        description: None,
        expiry: 3600,
    }
    .encode_unsigned();
    l.invoices.insert(
        hash,
        Pending {
            to: name,
            msat,
            bolt11: bolt11.clone(),
            zap_request,
            comment: q.get("comment").cloned(),
            paid: false,
        },
    );
    Json(json!({"pr": bolt11, "routes": []}))
}

async fn relay(State(inner): State<Arc<Inner>>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| serve_socket(inner, socket))
}

async fn send(socket: &mut WebSocket, msg: RelayMessage<'_>) -> bool {
    socket
        .send(Message::Text(msg.as_json().into()))
        .await
        .is_ok()
}

async fn serve_socket(inner: Arc<Inner>, mut socket: WebSocket) {
    let mut live = inner.live.subscribe();
    let mut subs: HashMap<nostr::prelude::SubscriptionId, Vec<Filter>> = HashMap::new();
    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(Message::Text(text))) = msg else {
                    if matches!(msg, Some(Ok(_))) { continue; }
                    return;
                };
                let Ok(m) = ClientMessage::from_json(text.as_str()) else { continue };
                match m {
                    ClientMessage::Event(ev) => {
                        let ev = ev.into_owned();
                        let ok = ev.verify().is_ok();
                        if !send(&mut socket, RelayMessage::ok(ev.id, ok, if ok { "" } else { "invalid: signature" })).await {
                            return;
                        }
                        if ok {
                            publish(&inner, ev.clone());
                            if ev.kind == Kind::WalletConnectRequest {
                                wallet_service(&inner, &ev);
                            }
                        }
                    }
                    ClientMessage::Req { subscription_id, filters } => {
                        let filters: Vec<Filter> = filters.into_iter().map(|f| f.into_owned()).collect();
                        let stored: Vec<Event> = ledger(&inner)
                            .events
                            .iter()
                            .filter(|e| filters.iter().any(|f| f.match_event(e, MatchEventOptions::new())))
                            .cloned()
                            .collect();
                        let sub = subscription_id.into_owned();
                        for e in stored {
                            if !send(&mut socket, RelayMessage::event(sub.clone(), e)).await {
                                return;
                            }
                        }
                        if !send(&mut socket, RelayMessage::eose(sub.clone())).await {
                            return;
                        }
                        subs.insert(sub, filters);
                    }
                    ClientMessage::Close(sub) => {
                        subs.remove(&*sub);
                    }
                    _ => {}
                }
            }
            ev = live.recv() => {
                let Ok(ev) = ev else { continue };
                for (sub, filters) in &subs {
                    if filters.iter().any(|f| f.match_event(&ev, MatchEventOptions::new()))
                        && !send(&mut socket, RelayMessage::event(sub.clone(), ev.clone())).await
                    {
                        return;
                    }
                }
            }
        }
    }
}

/// Answers an NWC request that reached the relay, if it is for one of
/// this network's wallets.
fn wallet_service(inner: &Inner, req: &Event) {
    let Some(service_pk) = req.tags.public_keys().next() else {
        return;
    };
    let Some((service, payer)) = ledger(inner).wallets.get(&service_pk).cloned() else {
        return;
    };
    let nip44 = req.tags.iter().any(|t| match t.as_slice() {
        [n, v, ..] if n == "encryption" => v == "nip44_v2",
        _ => false,
    });
    let plain = if nip44 {
        nip44::decrypt(service.secret_key(), &req.pubkey, &req.content)
    } else {
        nip04::decrypt(service.secret_key(), &req.pubkey, &req.content)
    };
    let answer = match plain.ok().and_then(|p| Request::from_json(&p).ok()) {
        Some(Request {
            params: RequestParams::PayInvoice(p),
            ..
        }) => match pay(inner, &payer, &p.invoice) {
            Ok(preimage) => json!({"result_type": "pay_invoice", "result": {"preimage": preimage}}),
            Err((code, message)) => {
                json!({"result_type": "pay_invoice", "error": {"code": code, "message": message}})
            }
        },
        _ => json!({"result_type": "pay_invoice", "error": {"code": "NOT_IMPLEMENTED", "message": "only pay_invoice"}}),
    };
    let text = answer.to_string();
    let content = if nip44 {
        nip44::encrypt(service.secret_key(), &req.pubkey, &text, nip44::Version::V2)
    } else {
        nip04::encrypt(service.secret_key(), &req.pubkey, &text)
    };
    let Ok(content) = content else { return };
    let mut b = EventBuilder::new(Kind::WalletConnectResponse, content)
        .tag(Tag::public_key(req.pubkey))
        .tag(Tag::event(req.id));
    if nip44 {
        b = b.tag(Tag::custom("encryption", ["nip44_v2"]));
    }
    if let Ok(ev) = b.finalize(&service) {
        publish(inner, ev);
    }
}

/// Settles `bolt11` from `payer`'s account; a zap's receipt is published.
fn pay(inner: &Inner, payer: &str, bolt11: &str) -> Result<String, (&'static str, String)> {
    let inv = Invoice::decode(bolt11).map_err(|_| ("OTHER", "unreadable invoice".to_string()))?;
    let hash = inv.payment_hash.ok_or(("NOT_FOUND", "no payment hash".to_string()))?;
    let receipt = {
        let mut l = ledger(inner);
        let (to, msat, paid) = {
            let p = l
                .invoices
                .get(&hash)
                .ok_or(("NOT_FOUND", "no such invoice on this network".to_string()))?;
            (p.to.clone(), p.msat, p.paid)
        };
        if paid {
            return Err(("OTHER", "already paid".into()));
        }
        if l.failing.contains(&to) {
            return Err(("PAYMENT_FAILED", "no route".into()));
        }
        let balance = l.balances.get(payer).copied().unwrap_or(0);
        if balance < msat {
            return Err(("INSUFFICIENT_BALANCE", "not enough sats".into()));
        }
        l.balances.insert(payer.to_string(), balance - msat);
        *l.balances.entry(to.clone()).or_insert(0) += msat;
        let (bolt11, zap_request, comment) = match l.invoices.get_mut(&hash) {
            Some(p) => {
                p.paid = true;
                (p.bolt11.clone(), p.zap_request.clone(), p.comment.clone())
            }
            None => return Err(("INTERNAL", "lost".into())),
        };
        l.payments.push(Payment {
            from: payer.to_string(),
            to,
            msat,
            comment,
            zap_request: zap_request.clone(),
        });
        zap_request.map(|r| (bolt11, r))
    };
    let preimage = hex::encode(Sha256::digest(hash));
    if let Some((bolt11, request)) = receipt {
        let ours = format!("ws://{}/relay", inner.addr);
        let names_us = crate::zap::relays_of(&request).iter().any(|r| r.trim_end_matches('/') == ours);
        let signed = ZapReceipt::new(bolt11, &request)
            .preimage(preimage.clone())
            .into_event_builder()
            .finalize(&inner.provider);
        if let Ok(receipt) = signed {
            if names_us {
                publish(inner, receipt.clone());
            }
            // Every other relay the request names gets it the way a real
            // address server delivers it: as a client, over WebSocket.
            for url in crate::zap::relays_of(&request) {
                if url.trim_end_matches('/') == ours {
                    continue;
                }
                let receipt = receipt.clone();
                tokio::spawn(async move {
                    if let Ok(mut r) = crate::relay::Relay::connect(&url).await {
                        let _ = r.publish(&receipt, std::time::Duration::from_secs(10)).await;
                    }
                });
            }
        }
    }
    Ok(preimage)
}
