//! S2 — can an `iroh-gossip` topic refuse a peer that is not a member?
//! (`DESIGN.md` §15 q2.) Loopback only, no relay, no discovery service.
//!
//! The mechanism under test: wrap `Gossip` in our own `ProtocolHandler` that
//! reads `conn.remote_id()` and either hands the connection to
//! `Gossip::handle_connection` or closes it. Gossip itself has no such hook.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use iroh::address_lookup::memory::MemoryLookup;
use iroh::endpoint::{Connection, presets};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, TransportAddr};
use iroh_gossip::api::{Event, GossipReceiver, GossipSender};
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::TopicId;
use n0_future::StreamExt;

/// An allow-list in front of one `Gossip` instance (one ALPN).
#[derive(Clone, Debug)]
struct Gate {
    gossip: Gossip,
    /// `None` = ungated, the baseline.
    members: Option<Arc<RwLock<HashSet<EndpointId>>>>,
    live: Arc<Mutex<HashMap<EndpointId, Vec<Connection>>>>,
    refused: Arc<AtomicUsize>,
    accepted: Arc<AtomicUsize>,
}

impl ProtocolHandler for Gate {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // Read off the connection, never out of a message body.
        let peer = conn.remote_id();
        let allowed = match &self.members {
            None => true,
            Some(m) => m.read().map(|m| m.contains(&peer)).unwrap_or(false),
        };
        if !allowed {
            self.refused.fetch_add(1, Ordering::SeqCst);
            conn.close(403u32.into(), b"not a member");
            return Ok(());
        }
        self.accepted.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut live) = self.live.lock() {
            live.entry(peer).or_default().push(conn.clone());
        }
        self.gossip.handle_connection(conn).await.map_err(AcceptError::from_err)
    }

    async fn shutdown(&self) {
        let _ = self.gossip.shutdown().await;
    }
}

impl Gate {
    fn admit(&self, peer: EndpointId) {
        if let Some(m) = &self.members {
            if let Ok(mut m) = m.write() {
                m.insert(peer);
            }
        }
    }

    /// Revoke: drop from the allow-list and, if `close`, also close every
    /// connection already accepted from that peer.
    fn revoke(&self, peer: EndpointId, close: bool) {
        if let Some(m) = &self.members {
            if let Ok(mut m) = m.write() {
                m.remove(&peer);
            }
        }
        if close {
            if let Ok(mut live) = self.live.lock() {
                for c in live.remove(&peer).unwrap_or_default() {
                    c.close(403u32.into(), b"revoked");
                }
            }
        }
    }

    fn live_connections(&self) -> usize {
        self.live.lock().map(|l| l.values().map(Vec::len).sum()).unwrap_or(0)
    }
}

/// One app: an endpoint, and one gated gossip per ALPN it speaks.
struct Node {
    ep: Endpoint,
    _router: Router,
    gates: HashMap<&'static str, Gate>,
}

impl Node {
    fn id(&self) -> EndpointId {
        self.ep.id()
    }
    fn gate(&self, alpn: &str) -> &Gate {
        &self.gates[alpn]
    }
}

/// `alpns`: ALPN → allow-list (`None` = ungated).
async fn node(
    lookup: &MemoryLookup,
    alpns: &[(&'static str, Option<Vec<EndpointId>>)],
) -> Result<Node> {
    node_with_key(lookup, alpns, iroh::SecretKey::generate()).await
}

async fn node_with_key(
    lookup: &MemoryLookup,
    alpns: &[(&'static str, Option<Vec<EndpointId>>)],
    key: iroh::SecretKey,
) -> Result<Node> {
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(key)
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind: {e}"))?;
    ep.address_lookup()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .add(lookup.clone());
    let sock = ep
        .bound_sockets()
        .into_iter()
        .find(|s| s.ip().is_loopback())
        .or_else(|| ep.bound_sockets().into_iter().next())
        .ok_or_else(|| anyhow::anyhow!("no socket"))?;
    lookup.set_endpoint_info(EndpointAddr::from_parts(ep.id(), [TransportAddr::Ip(sock)]));

    let mut router = Router::builder(ep.clone());
    let mut gates = HashMap::new();
    for (alpn, members) in alpns {
        let gate = Gate {
            gossip: Gossip::builder().alpn(alpn.as_bytes()).spawn(ep.clone()),
            members: members
                .as_ref()
                .map(|m| Arc::new(RwLock::new(m.iter().copied().collect()))),
            live: Arc::default(),
            refused: Arc::default(),
            accepted: Arc::default(),
        };
        router = router.accept(alpn.as_bytes(), gate.clone());
        gates.insert(*alpn, gate);
    }
    Ok(Node { ep, _router: router.spawn(), gates })
}

fn topic(name: &str) -> TopicId {
    let mut b = [0u8; 32];
    b[..name.len()].copy_from_slice(name.as_bytes());
    TopicId::from_bytes(b)
}

type Sub = (GossipSender, GossipReceiver);

async fn join(n: &Node, alpn: &str, t: TopicId, boot: &[EndpointId]) -> Result<Sub> {
    Ok(n.gate(alpn).gossip.subscribe(t, boot.to_vec()).await?.split())
}

async fn joined(rx: &mut GossipReceiver, within: Duration) -> bool {
    tokio::time::timeout(within, rx.joined()).await.map(|r| r.is_ok()).unwrap_or(false)
}

/// Every message body received within `window`.
async fn drain(rx: &mut GossipReceiver, window: Duration) -> Vec<String> {
    let mut got = vec![];
    let end = Instant::now() + window;
    while let Ok(Some(ev)) = tokio::time::timeout_at(end.into(), rx.next()).await {
        if let Ok(Event::Received(m)) = ev {
            got.push(String::from_utf8_lossy(&m.content).into_owned());
        }
    }
    got
}

struct Report(Vec<(String, bool)>);

impl Report {
    fn check(&mut self, label: &str, ok: bool) {
        println!("   [{}] {label}", if ok { "ok" } else { "FAIL" });
        self.0.push((label.to_string(), ok));
    }
}

const SHORT: Duration = Duration::from_secs(3);
const LONG: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<()> {
    println!("S2 — gated gossip (iroh 1.3.0, iroh-gossip 0.101.0), loopback, no relay\n");
    let mut r = Report(vec![]);
    let only = std::env::args().nth(1);
    let run = |n: &str| only.as_deref().is_none_or(|o| o == n);

    if run("t0") {
    // ---- T0 baseline: no gate -------------------------------------------
    println!("T0  ungated baseline: a stranger who knows one member and the topic");
    {
        let lk = MemoryLookup::new();
        let a = node(&lk, &[("g", None)]).await?;
        let n = node(&lk, &[("g", None)]).await?;
        let t = topic("secret-channel");
        let (a_tx, mut a_rx) = join(&a, "g", t, &[]).await?;
        let (_, mut n_rx) = join(&n, "g", t, &[a.id()]).await?;
        let _ = joined(&mut n_rx, LONG).await;
        let _ = joined(&mut a_rx, SHORT).await;
        a_tx.broadcast("private plans".into()).await?;
        let got = drain(&mut n_rx, SHORT).await;
        r.check("stranger joins and READS (this is the problem)", got.contains(&"private plans".into()));
    }

    }

    if run("t1") {
    // ---- T1 gate on remote_id --------------------------------------------
    println!("\nT1  accept-side gate on conn.remote_id()");
    let lk = MemoryLookup::new();
    // Ids are needed before the allow-lists are filled, so make the allow-lists
    // empty, build everyone, then admit.
    let alpn = [("g", Some(vec![]))];
    let a = node(&lk, &alpn).await?;
    let b = node(&lk, &alpn).await?;
    let c = node(&lk, &alpn).await?;
    let n = node(&lk, &alpn).await?;
    for node in [&a, &b, &c] {
        for peer in [a.id(), b.id(), c.id()] {
            node.gate("g").admit(peer);
        }
    }
    let t = topic("secret-channel");
    let (a_tx, _a_rx) = join(&a, "g", t, &[]).await?;
    let (_b_tx, mut b_rx) = join(&b, "g", t, &[a.id()]).await?;
    let (_c_tx, mut c_rx) = join(&c, "g", t, &[a.id()]).await?;
    let ok = joined(&mut b_rx, LONG).await && joined(&mut c_rx, LONG).await;
    r.check("members A, B, C form a swarm", ok);
    let (_, mut n_rx) = join(&n, "g", t, &[a.id(), b.id()]).await?;
    a_tx.broadcast("hello members".into()).await?;
    let (gb, gc) = (drain(&mut b_rx, SHORT).await, drain(&mut c_rx, SHORT).await);
    r.check("members receive the broadcast", gb.contains(&"hello members".into()) && gc.contains(&"hello members".into()));
    let got_n = drain(&mut n_rx, SHORT).await;
    r.check("stranger N (told A and B's ids and the topic) receives nothing", got_n.is_empty());
    r.check("stranger N never reports joined", !n_rx.is_joined());
    let refused = a.gate("g").refused.load(Ordering::SeqCst) + b.gate("g").refused.load(Ordering::SeqCst);
    r.check(&format!("members refused N's connection ({refused} time(s))"), refused >= 1);

    }

    if run("t2") {
    // ---- T2 admit at runtime: which retry actually works? -----------------
    println!("\nT2  N was refused; now it is admitted. What makes it join?");
    let only_variant = std::env::args().nth(2);
    for variant in ["wait", "wait-60", "subscribe-again", "drop-then-subscribe", "join_peers", "a-dials-n", "restart-n"] {
        // `wait-60` is slow; run it on request: `... t2 wait-60`.
        let skip = match only_variant.as_deref() {
            Some(v) => v != variant,
            None => variant == "wait-60",
        };
        if skip {
            continue;
        }
        let lk = MemoryLookup::new();
        let a = node(&lk, &[("g", Some(vec![]))]).await?;
        let n_key = iroh::SecretKey::generate();
        let mut n = node_with_key(&lk, &[("g", Some(vec![]))], n_key.clone()).await?;
        a.gate("g").admit(a.id());
        // N's own gate admits A: gossip links are bidirectional, so A may dial N.
        n.gate("g").admit(a.id());
        let t = topic("late-joiner");
        let (a_tx, mut a_rx) = join(&a, "g", t, &[]).await?;
        let _ = joined(&mut a_rx, Duration::from_millis(100)).await;
        let (n_tx, n_rx) = join(&n, "g", t, &[a.id()]).await?;
        tokio::time::sleep(Duration::from_secs(2)).await; // refused meanwhile
        a.gate("g").admit(n.id());
        let t0 = Instant::now();
        let (mut rx, tx) = match variant {
            "wait" | "wait-60" => (n_rx, n_tx),
            "subscribe-again" => {
                let (tx2, rx2) = join(&n, "g", t, &[a.id()]).await?;
                drop((n_tx, n_rx));
                (rx2, tx2)
            }
            "drop-then-subscribe" => {
                drop((n_tx, n_rx));
                tokio::time::sleep(Duration::from_millis(300)).await;
                let (tx2, rx2) = join(&n, "g", t, &[a.id()]).await?;
                (rx2, tx2)
            }
            "join_peers" => {
                n_tx.join_peers(vec![a.id()]).await?;
                (n_rx, n_tx)
            }
            "a-dials-n" => {
                a_tx.join_peers(vec![n.id()]).await?;
                (n_rx, n_tx)
            }
            _ => {
                // "restart-n": same key, fresh endpoint and gossip instance.
                drop((n_tx, n_rx));
                n.ep.close().await;
                n = node_with_key(&lk, &[("g", Some(vec![a.id()]))], n_key.clone()).await?;
                let (tx2, rx2) = join(&n, "g", t, &[a.id()]).await?;
                (rx2, tx2)
            }
        };
        let wait = if variant == "wait-60" { 60 } else { 20 };
        let j = joined(&mut rx, Duration::from_secs(wait)).await;
        let took = t0.elapsed();
        let mut got = false;
        if j {
            a_tx.broadcast("welcome N".into()).await?;
            got = drain(&mut rx, SHORT).await.contains(&"welcome N".into());
        }
        drop(tx);
        println!("   {variant:<20} joined={j} ({took:.1?}) received={got}");
        r.0.push((format!("T2 {variant}"), true)); // observational
    }

    }

    if run("t3") {
    // ---- T3 revoke (clean setup: N is a member from the start) -------------
    println!("\nT3  revoke a member who is connected");
    for close in [false, true] {
        let lk = MemoryLookup::new();
        let a = node(&lk, &[("g", Some(vec![]))]).await?;
        let n = node(&lk, &[("g", Some(vec![]))]).await?;
        for m in [&a, &n] {
            m.gate("g").admit(a.id());
            m.gate("g").admit(n.id());
        }
        let t = topic("revocation");
        let (a_tx, mut a_rx) = join(&a, "g", t, &[]).await?;
        let _ = joined(&mut a_rx, Duration::from_millis(100)).await;
        let (_, mut n_rx) = join(&n, "g", t, &[a.id()]).await?;
        r.check("N joins as a member", joined(&mut n_rx, LONG).await);
        a_tx.broadcast("before".into()).await?;
        r.check("N receives while a member", drain(&mut n_rx, SHORT).await.contains(&"before".into()));
        a.gate("g").revoke(n.id(), close);
        a_tx.broadcast("after revoke".into()).await?;
        let got = drain(&mut n_rx, SHORT).await.contains(&"after revoke".into());
        if close {
            r.check("list + close: N receives nothing after revocation", !got);
        } else {
            println!("   list only: N still receives = {got}");
            r.0.push(("T3 list-only observed".into(), true));
        }
    }

    }

    if run("t4") {
    // ---- T4 per-group ----------------------------------------------------
    println!("\nT4  one connection carries every topic: is the gate per peer or per group?");
    println!("   (a) one ALPN, one allow-list {{A, B, X}}, two groups");
    {
        let lk = MemoryLookup::new();
        let a = node(&lk, &[("g", Some(vec![]))]).await?;
        let b = node(&lk, &[("g", Some(vec![]))]).await?;
        let x = node(&lk, &[("g", Some(vec![]))]).await?;
        for m in [&a, &b, &x] {
            for p in [a.id(), b.id(), x.id()] {
                m.gate("g").admit(p);
            }
        }
        let (g1, g2) = (topic("group-1"), topic("group-2"));
        // X is in group 1 only. A and B are in both.
        let (_, _a1) = join(&a, "g", g1, &[]).await?;
        let (a2_tx, mut a2_rx) = join(&a, "g", g2, &[]).await?;
        let (_, mut b2_rx) = join(&b, "g", g2, &[a.id()]).await?;
        let _ = joined(&mut b2_rx, LONG).await;
        let _ = joined(&mut a2_rx, SHORT).await;
        // X joins group 2's topic, which it was never meant to be in.
        let (_, mut x2_rx) = join(&x, "g", g2, &[a.id()]).await?;
        let _ = joined(&mut x2_rx, LONG).await;
        a2_tx.broadcast("group-2 only".into()).await?;
        let leaked = drain(&mut x2_rx, SHORT).await.contains(&"group-2 only".into());
        r.check("X reads group 2 though only allowed for group 1 (hole)", leaked);
    }
    println!("   (b) one ALPN per group, each with its own allow-list");
    {
        let lk = MemoryLookup::new();
        let spec = |_: ()| [("hit/g/1", Some(vec![])), ("hit/g/2", Some(vec![]))];
        let a = node(&lk, &spec(())).await?;
        let b = node(&lk, &spec(())).await?;
        let x = node(&lk, &spec(())).await?;
        for m in [&a, &b, &x] {
            for p in [a.id(), b.id(), x.id()] {
                m.gate("hit/g/1").admit(p);
            }
        }
        for m in [&a, &b] {
            for p in [a.id(), b.id()] {
                m.gate("hit/g/2").admit(p);
            }
        }
        let (g1, g2) = (topic("group-1"), topic("group-2"));
        let (a1_tx, mut a1_rx) = join(&a, "hit/g/1", g1, &[]).await?;
        let (_, mut b1_rx) = join(&b, "hit/g/1", g1, &[a.id()]).await?;
        let (a2_tx, mut a2_rx) = join(&a, "hit/g/2", g2, &[]).await?;
        let (_, mut b2_rx) = join(&b, "hit/g/2", g2, &[a.id()]).await?;
        let (_, mut x1_rx) = join(&x, "hit/g/1", g1, &[a.id()]).await?;
        let (_, mut x2_rx) = join(&x, "hit/g/2", g2, &[a.id()]).await?;
        let _ = joined(&mut a1_rx, SHORT).await;
        let _ = joined(&mut a2_rx, SHORT).await;
        let ok = joined(&mut b1_rx, LONG).await && joined(&mut b2_rx, LONG).await;
        r.check("A and B are swarmed on both groups", ok);
        r.check("X joins group 1", joined(&mut x1_rx, LONG).await);
        a1_tx.broadcast("for group 1".into()).await?;
        a2_tx.broadcast("for group 2".into()).await?;
        let g1_x = drain(&mut x1_rx, SHORT).await;
        let g2_x = drain(&mut x2_rx, SHORT).await;
        r.check("X receives group 1", g1_x.contains(&"for group 1".into()));
        r.check("X receives nothing from group 2", g2_x.is_empty());
        r.check("B receives group 2", drain(&mut b2_rx, SHORT).await.contains(&"for group 2".into()));
        println!(
            "   cost: A holds {} accepted connection(s) for 2 groups with 2 peers",
            a.gate("hit/g/1").live_connections() + a.gate("hit/g/2").live_connections()
        );
    }

    }

    let failed: Vec<_> = r.0.iter().filter(|(_, ok)| !ok).collect();
    println!("\n{} checks, {} failed", r.0.len(), failed.len());
    if !failed.is_empty() {
        for (l, _) in failed {
            println!("  FAIL: {l}");
        }
        bail!("some checks failed");
    }
    println!("PASS");
    std::process::exit(0);
}
