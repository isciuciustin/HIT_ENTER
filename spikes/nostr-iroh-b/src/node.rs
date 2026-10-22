//! One node, two roles: it *serves* `hit/nostr/1` (a relay answering
//! NIP-01 + NIP-77 over a QUIC stream) and it *syncs* from a peer
//! (negentropy, then fetch what it lacks, then push what the peer lacks).
//! The same code is both — there is no "client" and no "server" binary.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use negentropy::{Id, Negentropy, NegentropyStorageVector};
use nostr::prelude::{ClientMessage, Event, EventId, Filter, RelayMessage, SubscriptionId};

use crate::frame::{read_frame, write_frame};
use crate::store::Store;

pub const ALPN: &[u8] = b"hit/nostr/1";

/// Negentropy's own frame budget. Hex doubles it on the wire, and our frame
/// cap is 1 MiB, so 128 KiB leaves a wide margin.
const NEG_FRAME_LIMIT: u64 = 128 * 1024;
const CHUNK: usize = 200;

/// Every peer id this relay saw on a connection — read off the connection,
/// never out of a message body (CLAUDE.md rule 1).
pub type SeenPeers = Arc<Mutex<Vec<EndpointId>>>;

pub async fn serve(ep: Endpoint, store: Store, seen: SeenPeers, verbose: bool) {
    while let Some(incoming) = ep.accept().await {
        let store = store.clone();
        let seen = seen.clone();
        tokio::spawn(async move {
            let res: Result<()> = async {
                let conn = incoming.accept()?.await?;
                let peer = conn.remote_id();
                seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.push(peer);
                if verbose {
                    eprintln!("[relay] connection from {peer}");
                }
                loop {
                    let (send, recv) = match conn.accept_bi().await {
                        Ok(s) => s,
                        Err(_) => return Ok(()), // peer closed
                    };
                    let store = store.clone();
                    tokio::spawn(async move {
                        if let Err(e) = session(send, recv, store, verbose).await {
                            eprintln!("[relay] session ended: {e:#}");
                        }
                    });
                }
            }
            .await;
            if let Err(e) = res {
                eprintln!("[relay] connection error: {e:#}");
            }
        });
    }
}

type Neg = Negentropy<'static, NegentropyStorageVector>;

async fn send_msg(send: &mut SendStream, msg: &RelayMessage<'_>) -> Result<()> {
    write_frame(send, &msg.as_json()).await?;
    Ok(())
}

async fn session(mut send: SendStream, mut recv: RecvStream, store: Store, verbose: bool) -> Result<()> {
    let mut negs: HashMap<String, Neg> = HashMap::new();
    while let Some((text, _)) = read_frame(&mut recv).await? {
        let msg = match ClientMessage::from_json(&text) {
            Ok(m) => m,
            Err(e) => {
                send_msg(&mut send, &RelayMessage::notice(format!("bad message: {e}"))).await?;
                continue;
            }
        };
        match msg {
            ClientMessage::Event(ev) => {
                let ev = ev.into_owned();
                // Verify before storing, forwarding or showing (rule 2).
                // Signature first; `hit-group` permissions come in M2.
                if let Err(e) = ev.verify() {
                    send_msg(&mut send, &RelayMessage::ok(ev.id, false, format!("invalid: {e}"))).await?;
                    continue;
                }
                let new = store.insert(&ev).await?;
                let note = if new { "" } else { "duplicate: already have it" };
                send_msg(&mut send, &RelayMessage::ok(ev.id, true, note)).await?;
            }
            ClientMessage::Req { subscription_id, filters } => {
                for f in &filters {
                    for ev in store.query(f).await? {
                        send_msg(&mut send, &RelayMessage::event(subscription_id.as_ref().clone(), ev)).await?;
                    }
                }
                send_msg(&mut send, &RelayMessage::eose(subscription_id.into_owned())).await?;
            }
            ClientMessage::NegOpen { subscription_id, filter, initial_message } => {
                let items = store.id_times(&filter).await?;
                let mut storage = NegentropyStorageVector::with_capacity(items.len());
                for (t, id) in items {
                    storage.insert(t, Id::from_byte_array(id))?;
                }
                storage.seal()?;
                let mut neg = Negentropy::owned(storage, NEG_FRAME_LIMIT)?;
                let out = neg.reconcile(&hex::decode(initial_message.as_ref())?)?;
                negs.insert(subscription_id.to_string(), neg);
                send_msg(&mut send, &RelayMessage::NegMsg {
                    subscription_id,
                    message: Cow::Owned(hex::encode(out)),
                }).await?;
            }
            ClientMessage::NegMsg { subscription_id, message } => {
                let Some(neg) = negs.get_mut(subscription_id.as_str()) else {
                    send_msg(&mut send, &RelayMessage::NegErr {
                        subscription_id,
                        message: Cow::Borrowed("closed: unknown subscription"),
                    }).await?;
                    continue;
                };
                let out = neg.reconcile(&hex::decode(message.as_ref())?)?;
                send_msg(&mut send, &RelayMessage::NegMsg {
                    subscription_id,
                    message: Cow::Owned(hex::encode(out)),
                }).await?;
            }
            ClientMessage::NegClose { subscription_id } => {
                negs.remove(subscription_id.as_str());
            }
            ClientMessage::Close(_) => {}
            other => {
                if verbose {
                    eprintln!("[relay] ignored {}", other.as_json().chars().take(40).collect::<String>());
                }
            }
        }
    }
    send.finish()?;
    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    pub neg_rounds: usize,
    pub neg_bytes_out: usize,
    pub neg_bytes_in: usize,
    pub we_have: usize,
    pub we_need: usize,
    pub fetched: usize,
    pub pushed: usize,
    pub rejected: usize,
    pub elapsed: Duration,
}

/// Reconcile `filter` with `peer`, then converge both ways.
pub async fn sync(ep: &Endpoint, peer: EndpointAddr, store: &Store, filter: &Filter) -> Result<SyncStats> {
    let started = Instant::now();
    let mut st = SyncStats::default();
    let conn = ep.connect(peer.clone(), ALPN).await.context("connect")?;
    // The thing we dialled is the thing we got (rule 1, from the other side).
    if conn.remote_id() != peer.id {
        bail!("dialled {} but reached {}", peer.id, conn.remote_id());
    }
    let (mut send, mut recv) = conn.open_bi().await?;

    // --- 1. negentropy: find out which ids differ -------------------------
    let items = store.id_times(filter).await?;
    let mut storage = NegentropyStorageVector::with_capacity(items.len());
    for (t, id) in items {
        storage.insert(t, Id::from_byte_array(id))?;
    }
    storage.seal()?;
    let mut neg = Negentropy::owned(storage, NEG_FRAME_LIMIT)?;
    let sub = SubscriptionId::new("neg1");
    let open = ClientMessage::neg_open(sub.clone(), filter.clone(), hex::encode(neg.initiate()?));
    st.neg_bytes_out += write_frame(&mut send, &open.as_json()).await?;

    let (mut have, mut need): (Vec<Id>, Vec<Id>) = (vec![], vec![]);
    loop {
        let (text, n) = read_frame(&mut recv).await?.context("peer closed mid-negentropy")?;
        st.neg_bytes_in += n;
        st.neg_rounds += 1;
        match RelayMessage::from_json(&text)? {
            RelayMessage::NegMsg { message, .. } => {
                match neg.reconcile_with_ids(&hex::decode(message.as_ref())?, &mut have, &mut need)? {
                    Some(out) => {
                        let m = ClientMessage::NegMsg {
                            subscription_id: Cow::Borrowed(&sub),
                            message: Cow::Owned(hex::encode(out)),
                        };
                        st.neg_bytes_out += write_frame(&mut send, &m.as_json()).await?;
                    }
                    None => break,
                }
            }
            RelayMessage::NegErr { message, .. } => bail!("NEG-ERR: {message}"),
            other => bail!("unexpected during negentropy: {}", other.as_json()),
        }
    }
    let close = ClientMessage::NegClose { subscription_id: Cow::Borrowed(&sub) };
    st.neg_bytes_out += write_frame(&mut send, &close.as_json()).await?;
    st.we_have = have.len();
    st.we_need = need.len();

    // --- 2. fetch what we lack, verifying each event ----------------------
    let need: Vec<EventId> = need.iter().map(|i| EventId::from_byte_array(*i.as_bytes())).collect();
    for chunk in need.chunks(CHUNK) {
        let sub = SubscriptionId::new("fetch");
        let req = ClientMessage::req(sub, Filter::new().ids(chunk.iter().copied()));
        write_frame(&mut send, &req.as_json()).await?;
        loop {
            let (text, _) = read_frame(&mut recv).await?.context("peer closed mid-fetch")?;
            match RelayMessage::from_json(&text)? {
                RelayMessage::Event { event, .. } => {
                    let ev = event.into_owned();
                    if ev.verify().is_err() {
                        st.rejected += 1;
                    } else if store.insert(&ev).await? {
                        st.fetched += 1;
                    }
                }
                RelayMessage::EndOfStoredEvents(_) => break,
                other => bail!("unexpected during fetch: {}", other.as_json()),
            }
        }
    }

    // --- 3. push what the peer lacks --------------------------------------
    let have: Vec<EventId> = have.iter().map(|i| EventId::from_byte_array(*i.as_bytes())).collect();
    for chunk in have.chunks(CHUNK) {
        let events = store.by_ids(chunk).await?;
        for ev in &events {
            write_frame(&mut send, &ClientMessage::event(ev.clone()).as_json()).await?;
        }
        for _ in &events {
            let (text, _) = read_frame(&mut recv).await?.context("peer closed mid-push")?;
            match RelayMessage::from_json(&text)? {
                RelayMessage::Ok { status: true, .. } => st.pushed += 1,
                RelayMessage::Ok { status: false, .. } => st.rejected += 1,
                other => bail!("unexpected during push: {}", other.as_json()),
            }
        }
    }

    // Graceful end: finish our side, wait for the relay to finish its own.
    send.finish()?;
    let _ = read_frame(&mut recv).await?;
    conn.close(0u32.into(), b"done");
    st.elapsed = started.elapsed();
    Ok(st)
}

/// Push a single event and return the relay's verdict — used to test that a
/// forged event is refused.
pub async fn push_one(ep: &Endpoint, peer: EndpointAddr, ev: &Event) -> Result<(bool, String)> {
    let conn = ep.connect(peer, ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &ClientMessage::event(ev.clone()).as_json()).await?;
    let (text, _) = read_frame(&mut recv).await?.context("no reply")?;
    send.finish()?;
    let _ = read_frame(&mut recv).await?;
    conn.close(0u32.into(), b"done");
    match RelayMessage::from_json(&text)? {
        RelayMessage::Ok { status, message, .. } => Ok((status, message.into_owned())),
        other => bail!("unexpected: {}", other.as_json()),
    }
}
