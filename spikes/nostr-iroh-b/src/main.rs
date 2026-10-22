//! S1 — Nostr over iroh. Can two apps be two relays?
//!
//! ```bash
//! cargo run --release -- demo          # the whole scenario, in one process
//! ./run-two-processes.sh               # the same, as two real OS processes
//! ```

mod frame;
mod node;
mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
use nostr::prelude::{Event, EventBuilder, FinalizeEvent, Filter, Keys, Kind, SingleLetterTag, Tag, Timestamp};

use node::{ALPN, SeenPeers, SyncStats};
use store::Store;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// The whole S1 scenario in one process, with assertions.
    Demo,
    /// Run a relay over iroh on this database. Prints a ticket.
    Serve {
        #[arg(long)]
        db: PathBuf,
        /// Write the ticket here too, so a script can read it.
        #[arg(long)]
        ticket_file: Option<PathBuf>,
    },
    /// Sign a note into the `--group` and store it locally.
    Post {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "g1")]
        group: String,
        #[arg(long)]
        content: String,
        /// How many copies (distinct events) to make.
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Negentropy-sync the `--group` with a peer's ticket.
    Sync {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        peer: String,
        #[arg(long, default_value = "g1")]
        group: String,
    },
    /// Print how many events a database holds in the group.
    Count {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "g1")]
        group: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Demo => demo().await,
        Cmd::Serve { db, ticket_file } => {
            let store = Store::open(&db).await?;
            let ep = bind().await?;
            let ticket = ticket_of(&ep);
            println!("{ticket}");
            if let Some(p) = ticket_file {
                std::fs::write(p, &ticket)?;
            }
            eprintln!("[relay] serving {} events on {}", store.count().await?, ALPN_STR);
            let seen = SeenPeers::default();
            tokio::select! {
                () = node::serve(ep, store, seen, true) => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            Ok(())
        }
        Cmd::Post { db, group, content, count } => {
            let store = Store::open(&db).await?;
            let keys = Keys::generate();
            let now = Timestamp::now().as_secs();
            for i in 0..count {
                let ev = make_event(&keys, &format!("{content} #{i}"), &group, now + u64::from(i))?;
                store.insert(&ev).await?;
            }
            println!("posted {count}; store now holds {}", store.count().await?);
            Ok(())
        }
        Cmd::Sync { db, peer, group } => {
            let store = Store::open(&db).await?;
            let ep = bind().await?;
            let st = node::sync(&ep, parse_ticket(&peer)?, &store, &group_filter(&group)).await?;
            print_stats("sync", &st);
            println!("store now holds {} events", store.count().await?);
            ep.close().await;
            Ok(())
        }
        Cmd::Count { db, group } => {
            let store = Store::open(&db).await?;
            println!("{}", store.query(&group_filter(&group)).await?.len());
            Ok(())
        }
    }
}

const ALPN_STR: &str = "hit/nostr/1";

async fn bind() -> Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind: {e}"))
}

/// `<endpoint id>@<ip:port>,<ip:port>…` — enough to dial on a LAN with no
/// discovery service. (A real ticket also carries a relay URL.)
fn ticket_of(ep: &Endpoint) -> String {
    let addrs: Vec<String> = ep.bound_sockets().iter().map(|s| s.to_string()).collect();
    format!("{}@{}", ep.id(), addrs.join(","))
}

fn parse_ticket(s: &str) -> Result<EndpointAddr> {
    let (id, addrs) = s.split_once('@').context("ticket is <id>@<ip:port>[,…]")?;
    let id: EndpointId = id.parse().map_err(|e| anyhow::anyhow!("endpoint id: {e}"))?;
    let addrs = addrs
        .split(',')
        .map(|a| a.parse::<SocketAddr>().map(TransportAddr::Ip))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EndpointAddr::from_parts(id, addrs))
}

/// Dial address for an endpoint in the same process: loopback only.
fn loopback_addr(ep: &Endpoint) -> Result<EndpointAddr> {
    let sock = ep
        .bound_sockets()
        .into_iter()
        .find(|s| s.ip().is_loopback())
        .or_else(|| ep.bound_sockets().into_iter().next())
        .context("no bound socket")?;
    Ok(EndpointAddr::from_parts(ep.id(), [TransportAddr::Ip(sock)]))
}

fn group_filter(group: &str) -> Filter {
    Filter::new().custom_tag(SingleLetterTag::LOWERCASE_H, group)
}

fn make_event(keys: &Keys, content: &str, group: &str, at: u64) -> Result<Event> {
    Ok(EventBuilder::new(Kind::TextNote, content)
        .tag(Tag::parse(["h", group])?)
        .custom_created_at(Timestamp::from_secs(at))
        .finalize(keys)?)
}

fn print_stats(label: &str, s: &SyncStats) {
    println!(
        "{label}: {} NEG round(s), {} B out / {} B in | peer lacked {}, we lacked {} | \
         fetched {}, pushed {}, rejected {} | {:?}",
        s.neg_rounds, s.neg_bytes_out, s.neg_bytes_in, s.we_have, s.we_need,
        s.fetched, s.pushed, s.rejected, s.elapsed,
    );
}

// ---------------------------------------------------------------------------

const SHARED: u32 = 2000;

async fn demo() -> Result<()> {
    println!("S1 — Nostr over iroh (iroh 1.1.0, nostr 0.45, negentropy 0.5, sqlx/SQLite)\n");
    let dir = tempfile::tempdir()?;
    let a_store = Store::open(&dir.path().join("a.db")).await?;
    let b_store = Store::open(&dir.path().join("b.db")).await?;
    let (a_ep, b_ep) = (bind().await?, bind().await?);
    let a_seen: SeenPeers = Arc::new(Mutex::new(vec![]));
    let b_seen: SeenPeers = Arc::new(Mutex::new(vec![]));
    // Both nodes are relays. "Every running app is a relay."
    tokio::spawn(node::serve(a_ep.clone(), a_store.clone(), a_seen.clone(), false));
    tokio::spawn(node::serve(b_ep.clone(), b_store.clone(), b_seen.clone(), false));
    let a_addr = loopback_addr(&a_ep)?;
    let b_addr = loopback_addr(&b_ep)?;
    println!("A = {}\nB = {}\n", a_ep.id(), b_ep.id());

    let alice = Keys::generate();
    let bob = Keys::generate();
    let t0 = 1_700_000_000u64;
    let g1 = group_filter("g1");

    // -- history both already hold: {SHARED} events ------------------------
    let shared: Vec<Event> = (0..SHARED)
        .map(|i| make_event(&alice, &format!("history {i}"), "g1", t0 + u64::from(i)))
        .collect::<Result<_>>()?;
    a_store.insert_many(&shared).await?;
    b_store.insert_many(&shared).await?;
    let history_bytes: usize = shared.iter().map(|e| e.as_json().len()).sum();
    println!("Seeded {SHARED} shared events ({history_bytes} B of JSON) in both stores.");

    // -- each side writes while apart ---------------------------------------
    let t1 = t0 + 10_000;
    for i in 0..7 {
        a_store.insert(&make_event(&alice, &format!("A offline {i}"), "g1", t1 + i)?).await?;
    }
    for i in 0..4 {
        b_store.insert(&make_event(&bob, &format!("B offline {i}"), "g1", t1 + 100 + i)?).await?;
    }
    // A different group that B must NOT receive through a g1 sync.
    a_store.insert(&make_event(&alice, "secret g2", "g2", t1)?).await?;
    println!("A wrote 7 + 1 (other group); B wrote 4. Not connected.\n");

    // 1. first reconnect ---------------------------------------------------
    let s1 = node::sync(&b_ep, a_addr.clone(), &b_store, &g1).await?;
    print_stats("1. B→A after being apart", &s1);
    ensure!(s1.we_need == 7 && s1.we_have == 4, "expected 7/4, got {}/{}", s1.we_need, s1.we_have);
    ensure!(s1.fetched == 7 && s1.pushed == 4);
    let (a_n, b_n) = (a_store.query(&g1).await?.len(), b_store.query(&g1).await?.len());
    ensure!(a_n == b_n && a_n == SHARED as usize + 11, "g1 sets differ: A {a_n}, B {b_n}");
    let b_g2 = b_store.query(&group_filter("g2")).await?.len();
    ensure!(b_g2 == 0, "a g1 sync leaked {b_g2} g2 event(s)");
    println!("   converged: both hold {a_n} g1 events; 0 g2 events leaked to B\n");
    println!(
        "   wire: {} B of negentropy to reconcile {} events, vs {} B to just send them all\n",
        s1.neg_bytes_out + s1.neg_bytes_in, a_n, history_bytes,
    );

    // 2. nothing changed ---------------------------------------------------
    let s2 = node::sync(&b_ep, a_addr.clone(), &b_store, &g1).await?;
    print_stats("2. B→A, nothing new", &s2);
    ensure!(s2.we_need == 0 && s2.we_have == 0);
    println!();

    // 3. B was offline again; A has news, B has news, and A syncs *from* B
    //    (the roles are symmetric — either app can dial either).
    for i in 0..3 {
        a_store.insert(&make_event(&alice, &format!("A again {i}"), "g1", t1 + 500 + i)?).await?;
    }
    for i in 0..2 {
        b_store.insert(&make_event(&bob, &format!("B again {i}"), "g1", t1 + 600 + i)?).await?;
    }
    let s3 = node::sync(&a_ep, b_addr.clone(), &a_store, &g1).await?;
    print_stats("3. A→B (roles reversed)", &s3);
    ensure!(s3.we_need == 2 && s3.we_have == 3);
    ensure!(a_store.query(&g1).await?.len() == b_store.query(&g1).await?.len());
    println!();

    // 4. a forged event is refused and not stored --------------------------
    let before = a_store.count().await?;
    let real = make_event(&alice, "pay me 1 BTC", "g1", t1 + 900)?;
    let forged_json = real.as_json().replace("pay me 1 BTC", "pay me 2 BTC");
    let forged = Event::from_json(forged_json)?; // parses; does not verify
    let (ok, why) = node::push_one(&b_ep, a_addr.clone(), &forged).await?;
    println!("4. forged event pushed to A: accepted={ok} ({why})");
    ensure!(!ok && a_store.count().await? == before, "forged event was stored");
    let (ok, _) = node::push_one(&b_ep, a_addr.clone(), &real).await?;
    ensure!(ok, "the genuine event was refused");
    println!("   the genuine event: accepted={ok}\n");

    // 5. who connected, as the relay saw it ---------------------------------
    let seen = a_seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.clone();
    ensure!(!seen.is_empty() && seen.iter().all(|id| *id == b_ep.id()));
    println!("5. A saw {} connection(s), all from B's EndpointId — read off the", seen.len());
    println!("   connection, not from any message.\n");

    // 6. a hostile frame length is refused before allocating -----------------
    let (mut send, mut recv) = b_ep.connect(a_addr, ALPN).await?.open_bi().await?;
    send.write_all(&u32::MAX.to_le_bytes()).await?;
    let r = frame::read_frame(&mut recv).await;
    println!("6. 4 GiB length prefix → relay dropped the stream: {}", match r {
        Ok(None) => "clean EOF".to_string(),
        Ok(Some(_)) => bail!("relay answered a hostile frame"),
        Err(e) => format!("{e}"),
    });

    println!("\nPASS — all assertions held.");
    a_ep.close().await;
    b_ep.close().await;
    Ok(())
}
