//! The relay half: answering `hit/nostr/1` (`docs/PROTOCOL.md`, "Session").
//!
//! 1. The dialler's first frame is its binding; it must verify against the
//!    connection's `EndpointId`. Otherwise a `NOTICE` says why and the
//!    connection closes.
//! 2. We answer with our own binding.
//! 3. NIP-01 `EVENT` / `REQ` / `CLOSE` and NIP-77 `NEG-*`, until the dialler
//!    finishes its side. **Every request is judged as the person the binding
//!    named**, per event, by `hit-group`: authenticating is not authorizing.
//!    A stranger is answered — with nothing it may not see — so that it can
//!    hand over a join.

use std::{borrow::Cow, collections::HashMap, fmt, sync::Arc, time::Duration};

use hit_proto::{
    frame::FrameError,
    hello,
    io::{read_frame, write_frame},
    limits::{
        MAX_EVENTS_PER_FILTER, MAX_FILTERS_PER_REQ, MAX_NEG_SESSIONS, MAX_SUBSCRIPTION_ID_BYTES,
        NEG_FRAME_LIMIT,
    },
};
use iroh::{
    EndpointId,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use negentropy::{Id, Negentropy, NegentropyStorageVector};
use nostr::prelude::{ClientMessage, Filter, PublicKey, RelayMessage};

use crate::{Inner, NodeError, Source};

/// How long to wait for the peer to acknowledge our last frames before the
/// connection is dropped. Returning from `accept` drops it, and a QUIC close
/// discards unacknowledged data (HIT_ENTER_2's `send_final` lesson).
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// QUIC close code for a refused session.
const REFUSED: u32 = 403;

pub(crate) struct Handler(pub(crate) Arc<Inner>);

impl fmt::Debug for Handler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("hit/nostr/1 handler")
    }
}

impl ProtocolHandler for Handler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // Rule 1: the peer's device is what the connection says, nothing else.
        let device = conn.remote_id();
        let (mut send, mut recv) = conn.accept_bi().await?;
        match serve(&self.0, device, &mut send, &mut recv).await {
            Ok(()) => {}
            Err(Refuse(why)) => {
                tracing::info!("refused {}: {why}", device.fmt_short());
                let _ =
                    write_relay(&mut send, &RelayMessage::notice(format!("refused: {why}"))).await;
                finish(&mut send).await;
                conn.close(REFUSED.into(), b"refused");
                return Ok(());
            }
        }
        finish(&mut send).await;
        Ok(())
    }
}

/// Finishes our side and waits (bounded) for the peer to have read it.
async fn finish(send: &mut SendStream) {
    let _ = send.finish();
    let _ = tokio::time::timeout(FLUSH_TIMEOUT, send.stopped()).await;
}

/// Why a session ended early, in words safe to send to the peer.
struct Refuse(String);

impl From<NodeError> for Refuse {
    fn from(e: NodeError) -> Self {
        Self(e.to_string())
    }
}

impl From<FrameError> for Refuse {
    fn from(e: FrameError) -> Self {
        Self(e.to_string())
    }
}

type Neg = Negentropy<'static, NegentropyStorageVector>;

async fn serve(
    inner: &Inner,
    device: EndpointId,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<(), Refuse> {
    // 1–2. Binding first, both ways.
    let first = read_frame(recv).await?;
    let presented = hello::decode(&first).map_err(NodeError::from)?;
    let person = inner.authenticate(&presented, device).await?;
    inner.heard_from(device);
    let ours = inner.own_binding().clone();
    write_frame(send, &hello::encode(&ours)).await?;

    // 3. The relay loop.
    let mut negs: HashMap<String, Neg> = HashMap::new();
    loop {
        let body = match read_frame(recv).await {
            Ok(b) => b,
            Err(FrameError::Closed) => {
                // The session may have made the dialler a member (a join);
                // its binding was refused at the hello if so. Offer it again.
                let _ = inner.accept_event(presented, Source::Fetch).await;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let Ok(text) = std::str::from_utf8(&body) else {
            write_relay(send, &RelayMessage::notice("error: frame is not UTF-8")).await?;
            continue;
        };
        let Ok(msg) = ClientMessage::from_json(text) else {
            write_relay(
                send,
                &RelayMessage::notice("error: not a NIP-01 client message"),
            )
            .await?;
            continue;
        };
        handle(inner, &person, msg, &mut negs, send).await?;
    }
}

async fn handle(
    inner: &Inner,
    person: &PublicKey,
    msg: ClientMessage<'_>,
    negs: &mut HashMap<String, Neg>,
    send: &mut SendStream,
) -> Result<(), Refuse> {
    match msg {
        ClientMessage::Event(ev) => {
            let ev = ev.into_owned();
            let id = ev.id;
            let (ok, why) = inner.accept_event(ev, Source::Push).await?;
            write_relay(send, &RelayMessage::ok(id, ok, why)).await?;
        }
        ClientMessage::Req {
            subscription_id,
            filters,
        } => {
            let sub = subscription_id.into_owned();
            if sub.as_str().len() > MAX_SUBSCRIPTION_ID_BYTES {
                write_relay(
                    send,
                    &RelayMessage::closed(sub, "invalid: subscription id too long"),
                )
                .await?;
                return Ok(());
            }
            if filters.len() > MAX_FILTERS_PER_REQ {
                write_relay(
                    send,
                    &RelayMessage::closed(sub, "invalid: too many filters"),
                )
                .await?;
                return Ok(());
            }
            // Only what this person may see; a stranger gets EOSE and
            // nothing else, which says no more than "nothing here".
            for filter in filters {
                for ev in inner.visible(person, &capped(filter.into_owned())).await? {
                    write_relay(send, &RelayMessage::event(sub.clone(), ev)).await?;
                }
            }
            // One-shot: stored events, then EOSE. Live delivery is gossip (M3).
            write_relay(send, &RelayMessage::eose(sub)).await?;
        }
        ClientMessage::NegOpen {
            subscription_id,
            filter,
            initial_message,
        } => {
            let key = subscription_id.to_string();
            if negs.len() >= MAX_NEG_SESSIONS && !negs.contains_key(&key) {
                return neg_err(
                    send,
                    subscription_id.into_owned(),
                    "blocked: too many negentropy sessions",
                )
                .await;
            }
            // The set is what this person may see — the same rule as a REQ,
            // or the difference would name events it may not fetch — minus
            // the two bindings the hello already carried.
            let mut filter = filter.into_owned();
            filter.limit = None;
            let items: Vec<(u64, [u8; 32])> = inner
                .reconcilable(person, &filter)
                .await?
                .iter()
                .map(|e| (e.created_at.as_secs(), e.id.to_bytes()))
                .collect();
            let reply = (|| {
                let mut storage = NegentropyStorageVector::with_capacity(items.len());
                for (t, id) in &items {
                    storage.insert(*t, Id::from_byte_array(*id)).ok()?;
                }
                storage.seal().ok()?;
                let mut neg = Negentropy::owned(storage, NEG_FRAME_LIMIT).ok()?;
                let out = neg
                    .reconcile(&hex::decode(initial_message.as_ref()).ok()?)
                    .ok()?;
                Some((neg, out))
            })();
            match reply {
                Some((neg, out)) => {
                    negs.insert(key, neg);
                    write_relay(
                        send,
                        &RelayMessage::NegMsg {
                            subscription_id,
                            message: Cow::Owned(hex::encode(out)),
                        },
                    )
                    .await?;
                }
                None => {
                    return neg_err(
                        send,
                        subscription_id.into_owned(),
                        "invalid: bad negentropy message",
                    )
                    .await;
                }
            }
        }
        ClientMessage::NegMsg {
            subscription_id,
            message,
        } => {
            let out = negs
                .get_mut(subscription_id.as_str())
                .and_then(|neg| neg.reconcile(&hex::decode(message.as_ref()).ok()?).ok());
            match out {
                Some(out) => {
                    write_relay(
                        send,
                        &RelayMessage::NegMsg {
                            subscription_id,
                            message: Cow::Owned(hex::encode(out)),
                        },
                    )
                    .await?;
                }
                None => {
                    negs.remove(subscription_id.as_str());
                    return neg_err(
                        send,
                        subscription_id.into_owned(),
                        "closed: unknown session or bad message",
                    )
                    .await;
                }
            }
        }
        ClientMessage::NegClose { subscription_id } => {
            negs.remove(subscription_id.as_str());
        }
        ClientMessage::Close(_) => {}
        _ => {
            write_relay(send, &RelayMessage::notice("error: unsupported message")).await?;
        }
    }
    Ok(())
}

/// A peer's filter with its limit capped: absent or too large becomes the cap.
fn capped(mut f: Filter) -> Filter {
    f.limit = Some(
        f.limit
            .map_or(MAX_EVENTS_PER_FILTER, |l| l.min(MAX_EVENTS_PER_FILTER)),
    );
    f
}

async fn neg_err(
    send: &mut SendStream,
    subscription_id: nostr::prelude::SubscriptionId,
    why: &str,
) -> Result<(), Refuse> {
    write_relay(
        send,
        &RelayMessage::NegErr {
            subscription_id: Cow::Owned(subscription_id),
            message: Cow::Owned(why.to_owned()),
        },
    )
    .await
}

async fn write_relay(send: &mut SendStream, msg: &RelayMessage<'_>) -> Result<(), Refuse> {
    write_frame(send, msg.as_json().as_bytes()).await?;
    Ok(())
}
