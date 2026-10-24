//! The client half: dial a node, exchange bindings, reconcile with NIP-77
//! negentropy, fetch what we lack and push what it lacks (spike S1).
//! **Gossip is for speed, negentropy is for truth** (`DESIGN.md` §4).
//!
//! Both sides reconcile over **what the other may see**: the listener over
//! what the dialler may read, the dialler over what the listener may read —
//! the same `hit-group` rule, so a sync never offers a group's events to a
//! peer outside it. Fetched events are applied in `hit_group::apply_order`,
//! so a group's state arrives before the actions and messages it
//! authorises.

use std::{borrow::Cow, fmt, time::Instant};

use hit_group::{CARRIED_KINDS, apply_order};
use hit_proto::{
    ALPN, hello,
    io::{read_frame, write_frame},
    limits::{FETCH_CHUNK, NEG_FRAME_LIMIT},
};
use iroh::{
    EndpointAddr,
    endpoint::{Connection, RecvStream, SendStream},
};
use negentropy::{Id, Negentropy, NegentropyStorageVector};
use nostr::prelude::{
    ClientMessage, Event, EventId, Filter, PublicKey, RelayMessage, SubscriptionId,
};

use crate::{Inner, NodeError, Source, Verdict};

/// What one sync did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncStats {
    /// Negentropy round trips.
    pub rounds: usize,
    /// Ids negentropy said we lack / the peer lacks.
    pub we_needed: usize,
    pub they_needed: usize,
    /// Fetched events stored new; fetched events we already had (should be 0).
    pub fetched: usize,
    pub duplicates: usize,
    /// Events we pushed that the peer accepted.
    pub pushed: usize,
    /// Events refused, in either direction.
    pub refused: usize,
    pub millis: u128,
}

impl fmt::Display for SyncStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fetched {} (dup {}), pushed {}, refused {}, {} rounds, {} ms",
            self.fetched, self.duplicates, self.pushed, self.refused, self.rounds, self.millis
        )
    }
}

/// Every kind this node carries — the filter both sides reconcile over,
/// before each narrows it to what the other may see.
fn carried() -> Filter {
    Filter::new().kinds(CARRIED_KINDS)
}

/// A session: dialled, bindings exchanged, the peer's person known.
struct Session {
    conn: Connection,
    send: SendStream,
    recv: RecvStream,
    person: PublicKey,
    /// The peer's hello binding — offered to the store again at the end,
    /// when this session may have made it a member we share a group with.
    binding: Event,
}

async fn open(
    inner: &Inner,
    peer: EndpointAddr,
    present: Option<Event>,
) -> Result<Session, NodeError> {
    let conn = inner
        .endpoint
        .connect(peer.clone(), ALPN)
        .await
        .map_err(|e| NodeError::Connect(e.to_string()))?;
    // The device we reached is the one we dialled — read off the connection.
    if conn.remote_id() != peer.id {
        return Err(NodeError::WrongPeer {
            dialled: peer.id,
            reached: conn.remote_id(),
        });
    }
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| NodeError::Connect(e.to_string()))?;

    // Binding first, both ways.
    let ours = present.unwrap_or_else(|| inner.own_binding().clone());
    write_frame(&mut send, &hello::encode(&ours)).await?;
    let reply = read_frame(&mut recv).await?;
    let theirs = match hello::decode(&reply) {
        Ok(ev) => ev,
        Err(e) => {
            // A refusal arrives as a NOTICE, then the connection closes.
            if let Ok(RelayMessage::Notice(why)) = std::str::from_utf8(&reply)
                .map_err(|_| ())
                .and_then(|t| RelayMessage::from_json(t).map_err(|_| ()))
            {
                return Err(NodeError::RefusedByPeer(why.into_owned()));
            }
            return Err(e.into());
        }
    };
    let person = inner.authenticate(&theirs, conn.remote_id()).await?;
    inner.heard_from(conn.remote_id());
    Ok(Session {
        conn,
        send,
        recv,
        person,
        binding: theirs,
    })
}

impl Session {
    /// Finish our side; wait for the relay to finish its own. Then keep the
    /// peer's binding if we now share a group: negentropy leaves the two
    /// parties' bindings out, so the hello is the only place it comes from.
    async fn close(mut self, inner: &Inner) {
        let _ = inner
            .accept_event(self.binding.clone(), Source::Fetch)
            .await;
        let _ = self.send.finish();
        let _ = read_frame(&mut self.recv).await;
        self.conn.close(0u32.into(), b"done");
    }
}

/// Hands one event to `peer` and returns its `OK` — how a join reaches a
/// member before the joiner holds anything of the group.
pub(crate) async fn deliver(
    inner: &Inner,
    peer: EndpointAddr,
    event: Event,
) -> Result<Verdict, NodeError> {
    let mut s = open(inner, peer, None).await?;
    write_client(&mut s.send, &ClientMessage::event(event)).await?;
    let verdict = match read_relay(&mut s.recv).await? {
        RelayMessage::Ok {
            status, message, ..
        } => (status, message.into_owned()),
        other => return Err(unexpected("deliver", &other)),
    };
    s.close(inner).await;
    Ok(verdict)
}

/// Opens a session with `peer` and closes it: who it speaks for, and the
/// binding that proved it — for a contact link.
pub(crate) async fn introduce(
    inner: &Inner,
    peer: EndpointAddr,
) -> Result<(PublicKey, Event), NodeError> {
    let s = open(inner, peer, None).await?;
    let found = (s.person, s.binding.clone());
    s.close(inner).await;
    Ok(found)
}

/// One `REQ` to `peer`: whatever it answers, unchecked and unstored — for
/// tests and the CLI's `req`, to see what a relay serves a given person.
pub(crate) async fn request(
    inner: &Inner,
    peer: EndpointAddr,
    filter: Filter,
) -> Result<Vec<Event>, NodeError> {
    let mut s = open(inner, peer, None).await?;
    write_client(
        &mut s.send,
        &ClientMessage::req(SubscriptionId::new("req"), filter),
    )
    .await?;
    let mut out = Vec::new();
    loop {
        match read_relay(&mut s.recv).await? {
            RelayMessage::Event { event, .. } => out.push(event.into_owned()),
            RelayMessage::EndOfStoredEvents(_) => break,
            RelayMessage::Closed { message, .. } => {
                return Err(NodeError::RefusedByPeer(message.into_owned()));
            }
            other => return Err(unexpected("req", &other)),
        }
    }
    s.close(inner).await;
    Ok(out)
}

pub(crate) async fn sync(
    inner: &Inner,
    peer: EndpointAddr,
    present: Option<Event>,
) -> Result<SyncStats, NodeError> {
    let started = Instant::now();
    let mut st = SyncStats::default();
    let mut s = open(inner, peer, present).await?;
    let (send, recv) = (&mut s.send, &mut s.recv);

    // 1. Negentropy: which ids differ.
    let items = inner.reconcilable(&s.person, &carried()).await?;
    let mut storage = NegentropyStorageVector::with_capacity(items.len());
    for ev in &items {
        storage
            .insert(
                ev.created_at.as_secs(),
                Id::from_byte_array(ev.id.to_bytes()),
            )
            .map_err(neg)?;
    }
    storage.seal().map_err(neg)?;
    let mut neg_state = Negentropy::owned(storage, NEG_FRAME_LIMIT).map_err(neg)?;
    let sub = SubscriptionId::new("sync");
    let open = ClientMessage::neg_open(
        sub.clone(),
        carried(),
        hex::encode(neg_state.initiate().map_err(neg)?),
    );
    write_client(send, &open).await?;

    let (mut have, mut need): (Vec<Id>, Vec<Id>) = (Vec::new(), Vec::new());
    loop {
        st.rounds += 1;
        match read_relay(recv).await? {
            RelayMessage::NegMsg { message, .. } => {
                let msg = hex::decode(message.as_ref())
                    .map_err(|e| NodeError::Protocol(e.to_string()))?;
                match neg_state
                    .reconcile_with_ids(&msg, &mut have, &mut need)
                    .map_err(neg)?
                {
                    Some(out) => {
                        let m = ClientMessage::NegMsg {
                            subscription_id: Cow::Borrowed(&sub),
                            message: Cow::Owned(hex::encode(out)),
                        };
                        write_client(send, &m).await?;
                    }
                    None => break,
                }
            }
            RelayMessage::NegErr { message, .. } => {
                return Err(NodeError::RefusedByPeer(message.into_owned()));
            }
            other => return Err(unexpected("negentropy", &other)),
        }
    }
    write_client(
        send,
        &ClientMessage::NegClose {
            subscription_id: Cow::Borrowed(&sub),
        },
    )
    .await?;
    st.we_needed = need.len();
    st.they_needed = have.len();

    // 2. Fetch what we lack, then apply it in dependency order. Every event
    // takes the one way in.
    let need: Vec<EventId> = need
        .iter()
        .map(|i| EventId::from_byte_array(*i.as_bytes()))
        .collect();
    let mut fetched = Vec::with_capacity(need.len());
    for chunk in need.chunks(FETCH_CHUNK) {
        let fetch = SubscriptionId::new("fetch");
        write_client(
            send,
            &ClientMessage::req(fetch, Filter::new().ids(chunk.iter().copied())),
        )
        .await?;
        loop {
            match read_relay(recv).await? {
                RelayMessage::Event { event, .. } => fetched.push(event.into_owned()),
                RelayMessage::EndOfStoredEvents(_) => break,
                RelayMessage::Closed { message, .. } => {
                    return Err(NodeError::RefusedByPeer(message.into_owned()));
                }
                other => return Err(unexpected("fetch", &other)),
            }
        }
    }
    fetched.sort_by_key(apply_order);
    // Within one second the order cannot say which of two actions came
    // first (a join, and its inviter's own join). What is refused waits and
    // is tried again while anything new gets in — the fold's own rule.
    let mut waiting = Vec::new();
    for ev in fetched {
        match inner.accept_event(ev.clone(), Source::Fetch).await? {
            (true, why) if why.is_empty() => st.fetched += 1,
            (true, _) => st.duplicates += 1,
            (false, why) => waiting.push((ev, why)),
        }
    }
    loop {
        let before = waiting.len();
        let mut still = Vec::new();
        for (ev, _) in waiting {
            match inner.accept_event(ev.clone(), Source::Fetch).await? {
                (true, _) => st.fetched += 1,
                (false, why) => still.push((ev, why)),
            }
        }
        waiting = still;
        if waiting.is_empty() || waiting.len() == before {
            break;
        }
    }
    for (ev, why) in &waiting {
        tracing::info!("refused a fetched {}: {why}", describe(ev));
    }
    st.refused += waiting.len();

    // 3. Push what the peer lacks, in the same order.
    let have: Vec<EventId> = have
        .iter()
        .map(|i| EventId::from_byte_array(*i.as_bytes()))
        .collect();
    let mut pushing = Vec::with_capacity(have.len());
    for chunk in have.chunks(FETCH_CHUNK) {
        pushing.extend(inner.store.by_ids(chunk).await?);
    }
    pushing.sort_by_key(apply_order);
    for chunk in pushing.chunks(FETCH_CHUNK) {
        for ev in chunk {
            write_client(send, &ClientMessage::event(ev.clone())).await?;
        }
        for ev in chunk {
            match read_relay(recv).await? {
                RelayMessage::Ok { status: true, .. } => st.pushed += 1,
                RelayMessage::Ok {
                    status: false,
                    message,
                    ..
                } => {
                    tracing::info!("peer refused our {}: {message}", describe(ev));
                    st.refused += 1;
                }
                other => return Err(unexpected("push", &other)),
            }
        }
    }

    s.close(inner).await;
    st.millis = started.elapsed().as_millis();
    Ok(st)
}

/// An event, for a log line: its kind, and enough of its id and author to
/// find it in a store. Never its content.
pub(crate) fn describe(ev: &Event) -> String {
    format!(
        "kind {} event {} by {}",
        ev.kind.as_u16(),
        &ev.id.to_hex()[..8],
        &ev.pubkey.to_hex()[..8]
    )
}

fn neg(e: negentropy::Error) -> NodeError {
    NodeError::Protocol(format!("negentropy: {e}"))
}

fn unexpected(during: &str, msg: &RelayMessage<'_>) -> NodeError {
    let json = msg.as_json();
    NodeError::Protocol(format!(
        "unexpected during {during}: {}",
        json.chars().take(80).collect::<String>()
    ))
}

async fn write_client(send: &mut SendStream, msg: &ClientMessage<'_>) -> Result<(), NodeError> {
    write_frame(send, msg.as_json().as_bytes()).await?;
    Ok(())
}

async fn read_relay(recv: &mut RecvStream) -> Result<RelayMessage<'static>, NodeError> {
    let body = read_frame(recv).await?;
    let text =
        std::str::from_utf8(&body).map_err(|_| NodeError::Protocol("frame is not UTF-8".into()))?;
    RelayMessage::from_json(text)
        .map_err(|e| NodeError::Protocol(format!("not a relay message: {e}")))
}
