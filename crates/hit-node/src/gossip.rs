//! Live fan-out: one gated `iroh-gossip` instance per group (spike S2,
//! `docs/DESIGN.md` §4). **Gossip is for speed, negentropy is for truth.**
//!
//! - **The gate is the ALPN**, `hit/gossip/1/<group>`: a connection carries
//!   every topic, so per-topic gating on a shared ALPN is impossible.
//! - **Who may connect** is asked of `hit-group` per connection: the
//!   connection's `EndpointId`, through the binding book, to a person, to
//!   "is a member". Read off the connection, never out of a message.
//! - **Both directions are gated**, in an endpoint hook ([`GateHook`]) that
//!   sees every connection after its handshake. Gating only what we accept
//!   is not enough: gossip also *dials* its neighbours, and a removed
//!   member's swarm would happily keep a connection we opened.
//! - **A list is not revocation**: when the group's state changes, live
//!   connections — either direction — whose device no longer speaks for a
//!   member are closed.
//! - **Don't subscribe before you are a member** — a refused peer's gossip
//!   does not recover on its own (S2). A node learns it is a member over
//!   `hit/nostr/1` first.
//! - What goes on gossip is `hit_group::Groups::gossip_group`: never a
//!   private channel's messages, since every member is on this instance.
//! - Every payload is a signed event and takes the same gate as any other
//!   ([`crate::Inner::accept_event`]), via the node's intake queue.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use bytes::Bytes;
use iroh::{
    Endpoint, EndpointId,
    endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, WeakConnectionHandle},
};
use iroh_gossip::{
    api::{Event as GossipEvent, GossipSender},
    net::Gossip,
    proto::TopicId,
};
use n0_future::StreamExt;
use nostr::prelude::{Event, PublicKey};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::Inner;

/// QUIC close code for a refused or revoked gossip connection.
const REFUSED: u32 = 403;

/// The gossip gate, as an endpoint hook: every connection on a gossip ALPN,
/// incoming or outgoing, is admitted only if `hit-group` says its device
/// speaks for a member — and is then tracked, so it can be closed later.
/// Installed when the endpoint is built, before the node exists, so it
/// holds the node weakly and refuses everything until it is set.
#[derive(Debug, Clone, Default)]
pub(crate) struct GateHook {
    node: Arc<OnceLock<Weak<Inner>>>,
}

impl GateHook {
    pub(crate) fn set(&self, inner: &Arc<Inner>) {
        let _ = self.node.set(Arc::downgrade(inner));
    }
}

impl EndpointHooks for GateHook {
    async fn after_handshake<'a>(&'a self, conn: &'a Connection) -> AfterHandshakeOutcome {
        let Some(group) = hit_proto::gossip_group(conn.alpn()) else {
            return AfterHandshakeOutcome::accept();
        };
        let refuse = AfterHandshakeOutcome::Reject {
            error_code: REFUSED.into(),
            reason: b"not a member".to_vec(),
        };
        let Some(inner) = self.node.get().and_then(Weak::upgrade) else {
            return refuse;
        };
        let device = conn.remote_id();
        if inner.device_is_member(device, &group) {
            inner.gossip.track(&group, conn.weak_handle());
            AfterHandshakeOutcome::accept()
        } else {
            tracing::info!("refused gossip with {}", device.fmt_short());
            refuse
        }
    }
}

struct Instance {
    gossip: Gossip,
    sender: GossipSender,
    reader: JoinHandle<()>,
    /// Every gossip connection of this group, both directions.
    live: Vec<WeakConnectionHandle>,
}

/// Every group's gossip instance on this node.
#[derive(Default)]
pub(crate) struct Gossips {
    map: Mutex<HashMap<PublicKey, Instance>>,
}

fn topic(group: &PublicKey) -> TopicId {
    TopicId::from_bytes(group.to_bytes())
}

impl Gossips {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<PublicKey, Instance>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn has(&self, group: &PublicKey) -> bool {
        self.map().contains_key(group)
    }

    /// Every gossip ALPN this node answers.
    pub(crate) fn alpns(&self) -> Vec<Vec<u8>> {
        self.map().keys().map(hit_proto::gossip_alpn).collect()
    }

    /// Starts `group`'s instance and subscribes, bootstrapping from
    /// `peers`. Received payloads go to `intake`.
    pub(crate) async fn start(
        &self,
        endpoint: &Endpoint,
        group: PublicKey,
        peers: Vec<EndpointId>,
        intake: mpsc::Sender<Event>,
    ) -> Result<(), String> {
        if self.has(&group) {
            return Ok(());
        }
        let gossip = Gossip::builder()
            .alpn(hit_proto::gossip_alpn(&group))
            .spawn(endpoint.clone());
        let (sender, mut receiver) = gossip
            .subscribe(topic(&group), peers)
            .await
            .map_err(|e| e.to_string())?
            .split();
        let reader = tokio::spawn(async move {
            while let Some(item) = receiver.next().await {
                let Ok(GossipEvent::Received(msg)) = item else {
                    continue;
                };
                // A payload that is not an event is dropped; one that is goes
                // through the full gate like anything else.
                if let Ok(ev) = serde_json::from_slice::<Event>(&msg.content)
                    && intake.send(ev).await.is_err()
                {
                    break;
                }
            }
        });
        let mut map = self.map();
        if map.contains_key(&group) {
            // Raced with another start; keep the first.
            reader.abort();
            return Ok(());
        }
        map.insert(
            group,
            Instance {
                gossip,
                sender,
                reader,
                live: Vec::new(),
            },
        );
        Ok(())
    }

    /// Stops `group`'s instance: we are no longer a member.
    pub(crate) async fn stop(&self, group: &PublicKey) {
        let Some(inst) = self.map().remove(group) else {
            return;
        };
        inst.reader.abort();
        for c in inst.live.iter().filter_map(WeakConnectionHandle::upgrade) {
            c.close(REFUSED.into(), b"left");
        }
        let _ = inst.gossip.shutdown().await;
    }

    /// Remembers a gated connection of `group`, so it can be revoked.
    fn track(&self, group: &PublicKey, conn: WeakConnectionHandle) {
        if let Some(inst) = self.map().get_mut(group) {
            inst.live.retain(|c| c.upgrade().is_some());
            inst.live.push(conn);
        }
    }

    /// Hands an accepted connection — already admitted by [`GateHook`] — to
    /// `group`'s instance.
    pub(crate) async fn admit(&self, group: &PublicKey, conn: Connection) {
        let Some(gossip) = self.map().get(group).map(|i| i.gossip.clone()) else {
            conn.close(REFUSED.into(), b"not carried");
            return;
        };
        if let Err(e) = gossip.handle_connection(conn).await {
            tracing::debug!("gossip connection ended: {e}");
        }
    }

    /// Closes every live connection of `group`, either direction, whose
    /// device `allowed` refuses now. Revocation is this, not the list.
    pub(crate) fn revoke(&self, group: &PublicKey, allowed: impl Fn(EndpointId) -> bool) {
        let mut map = self.map();
        let Some(inst) = map.get_mut(group) else {
            return;
        };
        inst.live.retain(|weak| {
            let Some(c) = weak.upgrade() else {
                return false;
            };
            if allowed(c.remote_id()) {
                return true;
            }
            c.close(REFUSED.into(), b"revoked");
            false
        });
    }

    pub(crate) async fn broadcast(&self, group: &PublicKey, event: &Event) {
        let sender = self.map().get(group).map(|i| i.sender.clone());
        if let (Some(sender), Ok(json)) = (sender, serde_json::to_vec(event))
            && let Err(e) = sender.broadcast(Bytes::from(json)).await
        {
            tracing::debug!("gossip broadcast failed: {e}");
        }
    }

    /// Dials members we may not be connected to — what makes a refused,
    /// then admitted, peer join the swarm (S2's T2).
    pub(crate) async fn join_peers(&self, group: &PublicKey, peers: Vec<EndpointId>) {
        let sender = self.map().get(group).map(|i| i.sender.clone());
        if let Some(sender) = sender
            && !peers.is_empty()
        {
            let _ = sender.join_peers(peers).await;
        }
    }

    pub(crate) async fn shutdown(&self) {
        let groups: Vec<PublicKey> = self.map().keys().copied().collect();
        for g in groups {
            self.stop(&g).await;
        }
    }
}
