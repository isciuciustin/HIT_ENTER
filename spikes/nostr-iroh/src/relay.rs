//! The relay side: NIP-01 EVENT / REQ / CLOSE and NIP-77 NEG-* over one
//! bidirectional stream per session.
//!
//! No AUTH and no binding check — any peer that dials may read and write.
//! That is deliberate for S1 and is NOT the design (DESIGN.md §4).

use crate::frame::{read_frame, write_frame};
use crate::store::Store;
use anyhow::Result;
use iroh::endpoint::Connection;
use negentropy::{Id, Negentropy, NegentropyStorageVector};
use nostr::prelude::{ClientMessage, Filter, RelayMessage};
use std::borrow::Cow;
use std::collections::HashMap;
use tracing::{info, warn};

async fn send(s: &mut iroh::endpoint::SendStream, m: RelayMessage<'_>) -> Result<usize> {
    write_frame(s, &serde_json::to_string(&m)?).await
}

pub async fn serve_connection(conn: Connection, store: Store) -> Result<()> {
    // Read the peer's identity off the connection, never off a message.
    let peer = conn.remote_id();
    info!(%peer, "session open");
    let (mut tx, mut rx) = conn.accept_bi().await?;
    let mut neg: HashMap<String, Negentropy<'static, NegentropyStorageVector>> = HashMap::new();

    while let Some((text, _)) = read_frame(&mut rx).await? {
        let msg: ClientMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!(%peer, "unparseable frame: {e}");
                continue;
            }
        };
        match msg {
            ClientMessage::Event(event) => {
                let event = event.into_owned();
                let id = event.id;
                match store.insert(&event).await {
                    Ok(true) => send(&mut tx, RelayMessage::ok(id, true, "")).await?,
                    Ok(false) => send(&mut tx, RelayMessage::ok(id, true, "duplicate: already have it")).await?,
                    Err(e) => {
                        warn!(%peer, %id, "rejected: {e}");
                        send(&mut tx, RelayMessage::ok(id, false, format!("invalid: {e}"))).await?
                    }
                };
            }
            ClientMessage::Req { subscription_id, filters } => {
                let sub = subscription_id.into_owned();
                for filter in filters {
                    for event in store.query(&filter).await? {
                        send(&mut tx, RelayMessage::event(sub.clone(), event)).await?;
                    }
                }
                send(&mut tx, RelayMessage::eose(sub)).await?;
            }
            ClientMessage::Close(_) => {}
            ClientMessage::NegOpen { subscription_id, filter, initial_message } => {
                let sub = subscription_id.into_owned();
                let reply = match open_neg(&store, &filter, &initial_message).await {
                    Ok((state, reply)) => {
                        neg.insert(sub.to_string(), state);
                        RelayMessage::NegMsg { subscription_id: Cow::Owned(sub), message: Cow::Owned(hex::encode(reply)) }
                    }
                    Err(e) => RelayMessage::NegErr { subscription_id: Cow::Owned(sub), message: Cow::Owned(format!("error: {e}")) },
                };
                send(&mut tx, reply).await?;
            }
            ClientMessage::NegMsg { subscription_id, message } => {
                let sub = subscription_id.into_owned();
                let reply = match neg.get_mut(&sub.to_string()) {
                    None => RelayMessage::NegErr { subscription_id: Cow::Owned(sub), message: Cow::Owned("closed: unknown subscription".into()) },
                    Some(state) => match hex::decode(message.as_ref()).map_err(anyhow::Error::from).and_then(|q| Ok(state.reconcile(&q)?)) {
                        Ok(out) => RelayMessage::NegMsg { subscription_id: Cow::Owned(sub), message: Cow::Owned(hex::encode(out)) },
                        Err(e) => RelayMessage::NegErr { subscription_id: Cow::Owned(sub), message: Cow::Owned(format!("error: {e}")) },
                    },
                };
                send(&mut tx, reply).await?;
            }
            ClientMessage::NegClose { subscription_id } => {
                neg.remove(&subscription_id.to_string());
            }
            other => warn!(%peer, "ignored {:?}", other),
        }
    }
    tx.finish()?;
    conn.closed().await;
    info!(%peer, "session closed");
    Ok(())
}

async fn open_neg(
    store: &Store,
    filter: &Filter,
    initial_hex: &str,
) -> Result<(Negentropy<'static, NegentropyStorageVector>, Vec<u8>)> {
    let mut storage = NegentropyStorageVector::new();
    for e in store.query(filter).await? {
        storage.insert(e.created_at.as_secs(), Id::from_byte_array(e.id.to_bytes()))?;
    }
    storage.seal()?;
    let mut state = Negentropy::owned(storage, 0)?;
    let reply = state.reconcile(&hex::decode(initial_hex)?)?;
    Ok((state, reply))
}
