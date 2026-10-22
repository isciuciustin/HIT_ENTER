//! The dialling side: a REQ, an EVENT, and a negentropy sync.

use crate::frame::{read_frame, write_frame};
use crate::node::ALPN;
use crate::store::Store;
use anyhow::{anyhow, bail, Result};
use iroh::{endpoint::{Connection, RecvStream, SendStream}, Endpoint, EndpointId};
use negentropy::{Id, Negentropy, NegentropyStorageVector};
use nostr::prelude::{ClientMessage, Event, EventId, Filter, RelayMessage, SubscriptionId};
use std::borrow::Cow;

pub struct Session {
    conn: Connection,
    tx: SendStream,
    rx: RecvStream,
    pub bytes_out: usize,
    pub bytes_in: usize,
    pub frames_out: usize,
}

#[derive(Debug, Default)]
pub struct SyncStats {
    pub neg_rounds: usize,
    pub local_events: usize,
    pub we_need: usize,
    pub they_need: usize,
    pub fetched: usize,
    pub pushed: usize,
    pub bytes_out: usize,
    pub bytes_in: usize,
}

impl Session {
    pub async fn dial(ep: &Endpoint, peer: EndpointId) -> Result<Self> {
        let conn = ep.connect(peer, ALPN).await?;
        let (tx, rx) = conn.open_bi().await?;
        Ok(Self { conn, tx, rx, bytes_out: 0, bytes_in: 0, frames_out: 0 })
    }

    async fn send(&mut self, m: ClientMessage<'_>) -> Result<()> {
        self.bytes_out += write_frame(&mut self.tx, &serde_json::to_string(&m)?).await?;
        self.frames_out += 1;
        Ok(())
    }

    async fn recv(&mut self) -> Result<RelayMessage<'static>> {
        let (text, n) = read_frame(&mut self.rx).await?.ok_or_else(|| anyhow!("peer closed the stream"))?;
        self.bytes_in += n;
        Ok(serde_json::from_str::<RelayMessage<'static>>(&text)?)
    }

    /// EVENT → OK. Returns the relay's verdict.
    pub async fn publish(&mut self, event: Event) -> Result<(bool, String)> {
        let id = event.id;
        self.send(ClientMessage::event(event)).await?;
        match self.recv().await? {
            RelayMessage::Ok { event_id, status, message } if event_id == id => Ok((status, message.to_string())),
            other => bail!("expected OK, got {other:?}"),
        }
    }

    /// REQ → EVENT* → EOSE. Every event is verified before it is returned.
    pub async fn req(&mut self, filters: Vec<Filter>) -> Result<Vec<Event>> {
        let sub = SubscriptionId::new("req");
        self.send(ClientMessage::req(sub.clone(), filters)).await?;
        let mut out = Vec::new();
        loop {
            match self.recv().await? {
                RelayMessage::Event { event, .. } => {
                    let event = event.into_owned();
                    event.verify()?;
                    out.push(event);
                }
                RelayMessage::EndOfStoredEvents(_) => break,
                other => bail!("unexpected {other:?}"),
            }
        }
        self.send(ClientMessage::Close(Cow::Owned(sub))).await?;
        Ok(out)
    }

    /// NIP-77 reconciliation of `filter`'s events, then fetch what we lack and
    /// push what they lack.
    pub async fn sync(&mut self, store: &Store, filter: Filter) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let (out0, in0) = (self.bytes_out, self.bytes_in);

        let mut storage = NegentropyStorageVector::new();
        let local = store.query(&filter).await?;
        stats.local_events = local.len();
        for e in &local {
            storage.insert(e.created_at.as_secs(), Id::from_byte_array(e.id.to_bytes()))?;
        }
        storage.seal()?;
        let mut neg = Negentropy::owned(storage, 0)?;
        let sub = SubscriptionId::new("neg");

        let init = neg.initiate()?;
        self.send(ClientMessage::NegOpen {
            subscription_id: Cow::Owned(sub.clone()),
            filter: Cow::Owned(filter),
            initial_message: Cow::Owned(hex::encode(init)),
        })
        .await?;

        let (mut have, mut need): (Vec<Id>, Vec<Id>) = (Vec::new(), Vec::new());
        loop {
            match self.recv().await? {
                RelayMessage::NegMsg { message, .. } => {
                    stats.neg_rounds += 1;
                    match neg.reconcile_with_ids(&hex::decode(message.as_ref())?, &mut have, &mut need)? {
                        Some(next) => {
                            self.send(ClientMessage::NegMsg {
                                subscription_id: Cow::Owned(sub.clone()),
                                message: Cow::Owned(hex::encode(next)),
                            })
                            .await?
                        }
                        None => break,
                    }
                }
                RelayMessage::NegErr { message, .. } => bail!("NEG-ERR: {message}"),
                other => bail!("unexpected {other:?}"),
            }
        }
        self.send(ClientMessage::NegClose { subscription_id: Cow::Owned(sub) }).await?;

        let to_event_id = |id: &Id| EventId::from_byte_array(id.to_bytes());
        let need: Vec<EventId> = need.iter().map(to_event_id).collect();
        let have: Vec<EventId> = have.iter().map(to_event_id).collect();
        stats.we_need = need.len();
        stats.they_need = have.len();

        for chunk in need.chunks(200) {
            for event in self.req(vec![Filter::new().ids(chunk.iter().copied())]).await? {
                if store.insert(&event).await? {
                    stats.fetched += 1;
                }
            }
        }
        for event in store.get(&have).await? {
            let (ok, msg) = self.publish(event).await?;
            if ok {
                stats.pushed += 1;
            } else {
                bail!("peer refused an event we hold: {msg}");
            }
        }

        stats.bytes_out = self.bytes_out - out0;
        stats.bytes_in = self.bytes_in - in0;
        Ok(stats)
    }

    pub async fn close(mut self) -> Result<()> {
        let _ = self.tx.finish();
        self.conn.close(0u32.into(), b"bye");
        Ok(())
    }
}
