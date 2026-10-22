//! The anchor's `ws://` bridge: an ordinary Nostr relay, for NIP-29 clients
//! (Chachi, 0xchat) — `DESIGN.md` §2's first bridge, M6.
//!
//! **Plain `ws://`, on whatever address the operator gives** (localhost, in
//! `docs/SELF_HOSTING.md`); `wss://` is a reverse proxy in front of it. No
//! certificate code here, and nothing here a member's app also does.
//!
//! What it serves, and to whom:
//!
//! - **Nothing before NIP-42 authentication.** A client signs the relay's
//!   challenge with *its person's key* — the same npub as in HIT_ENTER — and
//!   from then on is answered exactly as `hit/nostr/1` answers that npub:
//!   [`hit_node::Node::read_as`], per event, by `hit-group`. A member reads
//!   what a member may; a stranger, nothing.
//! - **Group state, translated** ([`crate::nip29`]): NIP-29 clients get
//!   `39000`–`39003` signed by this relay, never the group key's own, which
//!   they would not understand.
//! - **Writes go through the node's one gate** ([`hit_node::Node::accept`]).
//!   A NIP-29 client's message has no `channel` tag, so the group refuses it
//!   — said in the `OK`. Reading is M6's done-when; posting from those
//!   clients is a later protocol decision.
//! - **Zap receipts (M10) without authentication**: a Lightning address
//!   server delivers a kind 9735 here when a zap request named this relay;
//!   the anchor checks it and carries it into the zapped message's group
//!   ([`hit_node::Node::carry_zap`]).
//! - NIP-11 on a plain `GET`, with the CORS header NIP-11 asks for.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use hit_event::{Person, group};
use hit_node::Node;
use nostr::prelude::{
    ClientMessage, Event, Filter, Kind, MatchEventOptions, PublicKey, RelayMessage, SubscriptionId,
    Timestamp,
};
use tokio::sync::broadcast::error::RecvError;

use crate::nip29;

/// Most filters one `REQ` may carry, and events one filter is answered
/// with: `hit/nostr/1`'s limits.
const MAX_FILTERS: usize = hit_proto::limits::MAX_FILTERS_PER_REQ;
const MAX_EVENTS: usize = hit_proto::limits::MAX_EVENTS_PER_FILTER;
/// How old an `AUTH` may be.
const AUTH_MAX_AGE: u64 = 600;

/// What the bridge needs: the node, and the key it signs translations and
/// NIP-11 with — the anchor's own person.
pub struct Bridge {
    node: Node,
    relay: Person,
    name: String,
    /// The public `wss://` URL, if the operator said; when set, an `AUTH`
    /// must name it.
    url: Option<String>,
}

impl Bridge {
    pub fn new(node: Node, relay: Person, name: String, url: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            node,
            relay,
            name,
            url,
        })
    }
}

/// Serves `bridge` on `addr` until the task is dropped.
pub async fn serve(bridge: Arc<Bridge>, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_on(bridge, listener).await
}

pub async fn serve_on(
    bridge: Arc<Bridge>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let app = Router::new().route("/", get(root)).with_state(bridge);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn root(
    State(b): State<Arc<Bridge>>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    _headers: HeaderMap,
) -> Response {
    // Not an upgrade: a plain GET, answered with NIP-11.
    if let Ok(ws) = ws {
        return ws.on_upgrade(move |socket| session(b, socket));
    }
    // NIP-11.
    let doc = serde_json::json!({
        "name": b.name,
        "description": "A HIT_ENTER anchor. Groups here are members-only: authenticate (NIP-42) with your own key.",
        "pubkey": b.relay.public_key().to_hex(),
        "supported_nips": [1, 11, 29, 42, 57],
        "software": "hit-anchor",
        "version": env!("CARGO_PKG_VERSION"),
        "limitation": { "auth_required": true, "restricted_writes": true },
    });
    let mut res = doc.to_string().into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/nostr+json"),
    );
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    res
}

/// The group key's own state events: a NIP-29 client gets translations
/// instead.
fn native_state(ev: &Event) -> bool {
    group::STATE_KINDS.contains(&ev.kind)
}

fn could_ask_for_state(f: &Filter) -> bool {
    f.kinds.as_ref().is_none_or(|ks| {
        [
            nip29::NIP29_METADATA,
            nip29::NIP29_ADMINS,
            nip29::NIP29_MEMBERS,
            nip29::NIP29_ROLES,
        ]
        .iter()
        .any(|k| ks.contains(k))
    })
}

struct Conn {
    b: Arc<Bridge>,
    challenge: String,
    reader: Option<PublicKey>,
    subs: HashMap<SubscriptionId, Vec<Filter>>,
}

async fn send(socket: &mut WebSocket, msg: &RelayMessage<'_>) -> bool {
    socket
        .send(Message::Text(msg.as_json().into()))
        .await
        .is_ok()
}

async fn session(b: Arc<Bridge>, mut socket: WebSocket) {
    let mut c = Conn {
        challenge: challenge(),
        b,
        reader: None,
        subs: HashMap::new(),
    };
    tracing::info!("wss: a client connected");
    if !send(&mut socket, &RelayMessage::auth(c.challenge.clone())).await {
        return;
    }
    let mut live = c.b.node.subscribe();
    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    if !c.handle(&mut socket, text.as_str()).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            ev = live.recv() => match ev {
                Ok(ev) => {
                    if !c.live(&mut socket, &ev).await {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = send(&mut socket, &RelayMessage::notice("lagged: some live events were skipped; re-subscribe")).await;
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
}

fn challenge() -> String {
    let mut b = [0u8; 16];
    // A failed RNG leaves a zero challenge, which no AUTH can have signed
    // for a different relay session anyway, but say so.
    if getrandom::fill(&mut b).is_err() {
        tracing::warn!("no randomness for an AUTH challenge");
    }
    hex::encode(b)
}

impl Conn {
    /// One client message. `false` ends the session.
    async fn handle(&mut self, socket: &mut WebSocket, text: &str) -> bool {
        let Ok(msg) = ClientMessage::from_json(text) else {
            return send(
                socket,
                &RelayMessage::notice("error: not a NIP-01 client message"),
            )
            .await;
        };
        match msg {
            ClientMessage::Auth(ev) => {
                let ev = ev.into_owned();
                let id = ev.id;
                match self.check_auth(&ev) {
                    Ok(()) => {
                        self.reader = Some(ev.pubkey);
                        send(socket, &RelayMessage::ok(id, true, "")).await
                    }
                    Err(why) => send(socket, &RelayMessage::ok(id, false, why)).await,
                }
            }
            ClientMessage::Req {
                subscription_id,
                filters,
            } => {
                let sub = subscription_id.into_owned();
                let Some(reader) = self.reader else {
                    return send(
                        socket,
                        &RelayMessage::closed(
                            sub,
                            "auth-required: groups here are members-only; authenticate (NIP-42) with your own key",
                        ),
                    )
                    .await;
                };
                if filters.len() > MAX_FILTERS {
                    return send(
                        socket,
                        &RelayMessage::closed(sub, "invalid: too many filters"),
                    )
                    .await;
                }
                let filters: Vec<Filter> = filters.into_iter().map(|f| f.into_owned()).collect();
                let mut seen = std::collections::HashSet::new();
                for f in &filters {
                    let mut out = self.answer(&reader, f).await;
                    out.retain(|e| seen.insert(e.id));
                    for ev in out {
                        if !send(socket, &RelayMessage::event(sub.clone(), ev)).await {
                            return false;
                        }
                    }
                }
                self.subs.insert(sub.clone(), filters);
                send(socket, &RelayMessage::eose(sub)).await
            }
            ClientMessage::Close(sub) => {
                self.subs.remove(sub.as_ref());
                true
            }
            ClientMessage::Event(ev) => {
                let ev = ev.into_owned();
                let id = ev.id;
                // A zap receipt (M10), from the recipient's Lightning
                // address server — which does not authenticate, and need
                // not: the receipt is checked by NIP-57's rules and against
                // the recipient's own provider key, and only a zap of a
                // message held here is carried in. The receipt itself is
                // not kept, and nothing is served back.
                if ev.kind == hit_event::zap::ZAP_RECEIPT {
                    let (ok, why) = match self.b.node.carry_zap(&ev).await {
                        Ok(_) => {
                            tracing::info!("zap receipt {id} carried in");
                            (true, String::new())
                        }
                        Err(e) => {
                            tracing::warn!("zap receipt {id} refused: {e}");
                            (false, format!("blocked: {e}"))
                        }
                    };
                    return send(socket, &RelayMessage::ok(id, ok, why)).await;
                }
                if self.reader.is_none() {
                    return send(
                        socket,
                        &RelayMessage::ok(id, false, "auth-required: authenticate (NIP-42) first"),
                    )
                    .await;
                }
                if ev.kind == group::CHAT_MESSAGE && group::channel_of(&ev).is_none() {
                    return send(
                        socket,
                        &RelayMessage::ok(
                            id,
                            false,
                            "blocked: this group's messages need a channel tag; post from HIT_ENTER for now",
                        ),
                    )
                    .await;
                }
                let (ok, why) = match self.b.node.accept(ev).await {
                    Ok(v) => v,
                    Err(e) => (false, format!("error: {e}")),
                };
                send(socket, &RelayMessage::ok(id, ok, why)).await
            }
            _ => send(socket, &RelayMessage::notice("error: unsupported message")).await,
        }
    }

    /// NIP-42: a kind 22242, signed, naming our challenge (and our URL, if
    /// we know it), recent.
    fn check_auth(&self, ev: &Event) -> Result<(), &'static str> {
        if ev.kind != Kind::Authentication {
            return Err("invalid: not an AUTH event");
        }
        if ev.verify().is_err() {
            return Err("invalid: bad signature");
        }
        let tag = |name: &str| {
            ev.tags.iter().find_map(|t| match t.as_slice() {
                [n, v, ..] if n == name => Some(v.clone()),
                _ => None,
            })
        };
        if tag("challenge").as_deref() != Some(self.challenge.as_str()) {
            return Err("invalid: wrong challenge");
        }
        if let Some(url) = &self.b.url {
            let norm = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
            if tag("relay").is_none_or(|r| norm(&r) != norm(url)) {
                return Err("invalid: the AUTH names another relay");
            }
        }
        let now = Timestamp::now().as_secs();
        if ev.created_at.as_secs().abs_diff(now) > AUTH_MAX_AGE {
            return Err("invalid: AUTH too old or from the future");
        }
        Ok(())
    }

    /// The answer to one filter, as `reader`: what the node may show them,
    /// minus the group key's own state, plus its translation.
    async fn answer(&self, reader: &PublicKey, f: &Filter) -> Vec<Event> {
        let mut out: Vec<Event> = match self.b.node.read_as(reader, f).await {
            Ok(evs) => evs.into_iter().filter(|e| !native_state(e)).collect(),
            Err(e) => {
                tracing::warn!("a bridge REQ failed: {e}");
                Vec::new()
            }
        };
        if could_ask_for_state(f) {
            out.extend(
                self.translations(reader)
                    .await
                    .into_iter()
                    .filter(|e| f.match_event(e, MatchEventOptions::new())),
            );
        }
        out.truncate(f.limit.map_or(MAX_EVENTS, |l| l.min(MAX_EVENTS)));
        out
    }

    /// NIP-29 state for every group `reader` is a member of.
    async fn translations(&self, reader: &PublicKey) -> Vec<Event> {
        let mut out = Vec::new();
        for (_, state) in self.b.node.groups() {
            let Some(s) = state.filter(|s| s.is_member(reader)) else {
                continue;
            };
            let members: Vec<PublicKey> = s.members().copied().collect();
            let names = self.b.node.names(&members).await.unwrap_or_default();
            let at = Timestamp::now();
            match nip29::translate(&self.b.relay, &s, &|p| names.get(p).cloned(), at) {
                Ok(evs) => out.extend(evs),
                Err(e) => tracing::warn!("translating a group failed: {e}"),
            }
        }
        out
    }

    /// A live event: to every subscription it matches, if the reader may.
    async fn live(&self, socket: &mut WebSocket, ev: &Event) -> bool {
        let Some(reader) = self.reader else {
            return true;
        };
        if native_state(ev) || !self.b.node.may_read(&reader, ev) {
            return true;
        }
        for (sub, filters) in &self.subs {
            if filters
                .iter()
                .any(|f| f.match_event(ev, MatchEventOptions::new()))
                && !send(socket, &RelayMessage::event(sub.clone(), ev.clone())).await
            {
                return false;
            }
        }
        true
    }
}
