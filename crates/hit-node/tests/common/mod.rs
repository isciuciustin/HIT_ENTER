//! Shared by the done-when tests: nodes on loopback, no relay, no discovery.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::{future::Future, net::SocketAddr, path::Path, time::Duration};

use hit_event::Person;
use hit_node::{Config, Node, Relays};
use iroh::{EndpointAddr, TransportAddr};
use nostr::prelude::PublicKey;

pub async fn start(dir: &Path, person: &Person) -> Node {
    Node::start(
        same_person(person),
        Config {
            data_dir: dir.to_path_buf(),
            relays: Relays::Disabled,
            port: None,
        },
    )
    .await
    .unwrap()
}

/// A copy of `p`, as a second process holding the same nsec would have.
pub fn same_person(p: &Person) -> Person {
    Person::from_nsec(&p.reveal_nsec_for_backup()).unwrap()
}

pub fn loopback_addr(node: &Node) -> SocketAddr {
    let port = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(SocketAddr::is_ipv4)
        .unwrap()
        .port();
    ([127, 0, 0, 1], port).into()
}

/// A node's address on 127.0.0.1.
pub fn loopback(node: &Node) -> EndpointAddr {
    EndpointAddr::from_parts(node.id(), [TransportAddr::Ip(loopback_addr(node))])
}

/// Tells every node where every other one is.
pub fn mesh(nodes: &[&Node]) {
    for a in nodes {
        for b in nodes {
            if a.id() != b.id() {
                a.add_peer(loopback(b));
            }
        }
    }
}

pub async fn messages(node: &Node, group: &PublicKey) -> Vec<String> {
    node.messages(group, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.content)
        .collect()
}

/// Polls `check` until it holds or `within` passes.
pub async fn eventually<F, Fut>(within: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = tokio::time::Instant::now() + within;
    loop {
        if check().await {
            return true;
        }
        if tokio::time::Instant::now() > end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
