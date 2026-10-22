//! The accept loop: incoming connections, dispatched by ALPN.
//!
//! iroh's `Router` fixes its ALPNs when it is spawned, and a node's gossip
//! ALPNs come and go with its groups (`hit/gossip/1/<group>`). So this is
//! the router's loop, minus what we do not use, plus a dispatch that asks
//! the node each time — and the endpoint's ALPN list is replaced with
//! `set_alpns` whenever a group's instance starts or stops.

use std::sync::Arc;

use hit_proto::{ALPN, gossip_group};
use iroh::{endpoint::Incoming, protocol::ProtocolHandler};
use tokio::{sync::watch, task::JoinSet};

use crate::{Inner, relay};

/// Runs until `stop` changes or the endpoint closes.
pub(crate) async fn run(inner: Arc<Inner>, mut stop: watch::Receiver<bool>) {
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            incoming = inner.endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                tasks.spawn(handle(inner.clone(), incoming));
            }
        }
    }
    tasks.abort_all();
}

async fn handle(inner: Arc<Inner>, incoming: Incoming) {
    let mut accepting = match incoming.accept() {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!("accepting failed: {e}");
            return;
        }
    };
    let alpn = match accepting.alpn().await {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!("bad handshake: {e}");
            return;
        }
    };
    let conn = match accepting.await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("connection failed: {e}");
            return;
        }
    };
    if alpn == ALPN {
        if let Err(e) = relay::Handler(inner).accept(conn).await {
            tracing::debug!("session ended: {e}");
        }
    } else if let Some(handler) = registered(&inner, &alpn) {
        handler(conn).await;
    } else if alpn == iroh_blobs::ALPN {
        let blobs = inner.blobs.clone();
        blobs.serve(inner, conn).await;
    } else if let Some(group) = gossip_group(&alpn) {
        inner.gossip_accept(group, conn).await;
    } else {
        conn.close(0u32.into(), b"unsupported");
    }
}

/// A protocol another crate registered for `alpn` (M9), the lock released
/// before the connection is handed over.
fn registered(inner: &Inner, alpn: &[u8]) -> Option<crate::ProtocolFn> {
    inner
        .protocols
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(alpn)
        .cloned()
}
