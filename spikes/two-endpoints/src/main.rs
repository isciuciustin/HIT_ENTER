//! **M0 spike S5 — `DESIGN.md` §7 and §15 q5.**
//!
//! *Two iroh endpoints in one process, one of them relay-only: does it work,
//! what does it cost, and does anything in iroh assume one per process?*
//!
//! `DESIGN.md` §7 proposes a direct endpoint (DMs, calls, small private
//! groups) and a relay-only one (`clear_ip_transports()`, public groups),
//! **each with its own key and binding**. HIT_ENTER_2's `spikes/relay-path`
//! read — but never ran — that two endpoints sharing one key take turns at
//! the relay. This runs both.
//!
//! ```text
//! ./target/release/two-endpoints-spike                 # split same-key independent
//! ./target/release/two-endpoints-spike split           # one section
//! ./target/release/two-endpoints-spike cost direct 8   # cost: direct | relay-only | pair
//! ./target/release/two-endpoints-spike real-net        # n0's relays; prints counts, never addresses
//! ```
//!
//! Everything except `real-net` runs on loopback against a relay server in a
//! **child process** (`serve-relay`), so its memory and threads never land in
//! the numbers `cost` reports.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey,
    endpoint::{Connection, presets},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{sleep, timeout},
};

/// Not `hit/nostr/1`. A spike must not be able to talk to a real node.
const ALPN: &[u8] = b"hit-spike/two-endpoints/1";

/// How long to let holepunching run before reading a path list. On loopback a
/// direct path appears in well under a second.
const SETTLE: Duration = Duration::from_secs(5);

const DIAL_TIMEOUT: Duration = Duration::from_secs(8);

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let section = args.first().map(String::as_str).unwrap_or("all");
    match section {
        "serve-relay" => serve_relay().await,
        "split" => split().await.map(|_| ()),
        "same-key" => same_key().await,
        "independent" => independent().await,
        "cost" => {
            let kind = args.get(1).map(String::as_str).unwrap_or("direct");
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
            cost(kind, n).await
        }
        "real-net" => real_net().await,
        "all" => {
            split().await?;
            same_key().await?;
            independent().await
        }
        other => bail!("unknown section {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The relay, in its own process
// ---------------------------------------------------------------------------

/// Child mode: run a relay with QUIC address discovery, print `url port`,
/// and live until stdin closes (the parent's `kill_on_drop` also ends it).
async fn serve_relay() -> Result<()> {
    let (map, url, _server) = iroh::test_utils::run_relay_server().await?;
    let quic_port = map
        .get(&url)
        .and_then(|c| c.quic.as_ref().map(|q| q.port))
        .context("relay has no QUIC config")?;
    println!("{url} {quic_port}");
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    while stdin.read_line(&mut line).await? > 0 {
        line.clear();
    }
    Ok(())
}

struct Relay {
    map: RelayMap,
    url: RelayUrl,
    _child: Child,
}

async fn spawn_relay() -> Result<Relay> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("serve-relay")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawning the relay child")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).await?;
    let mut parts = line.split_whitespace();
    let url: RelayUrl = parts.next().context("no url")?.parse()?;
    let port: u16 = parts.next().context("no port")?.parse()?;
    let config = RelayConfig::new(url.clone(), Some(iroh_relay::RelayQuicConfig::new(port)));
    Ok(Relay {
        map: config.into(),
        url,
        _child: child,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Direct,
    RelayOnly,
}

async fn bind(relay: &Relay, key: SecretKey, kind: Kind) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        // The child relay's certificate is self-signed. A property of this
        // harness, not of the product.
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .relay_mode(RelayMode::Custom(relay.map.clone()))
        .alpns(vec![ALPN.to_vec()]);
    if kind == Kind::RelayOnly {
        builder = builder.clear_ip_transports();
    }
    let ep = builder.bind().await.context("binding")?;
    timeout(Duration::from_secs(10), ep.online())
        .await
        .context("endpoint never came online")?;
    Ok(ep)
}

type Accepted = mpsc::UnboundedSender<(&'static str, Connection)>;

/// Accept loop: reports each accepted connection, and echoes every bi stream
/// back prefixed with `name:` so a dialler can tell which endpoint answered.
fn serve(ep: Endpoint, name: &'static str, tx: Accepted) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(accepting) = incoming.accept() else { return };
                let Ok(conn) = accepting.await else { return };
                let _ = tx.send((name, conn.clone()));
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    let Ok(buf) = recv.read_to_end(1024).await else { break };
                    let mut out = format!("{name}:").into_bytes();
                    out.extend(buf);
                    let _ = send.write_all(&out).await;
                    let _ = send.finish();
                }
            });
        }
    })
}

/// Dial by `EndpointId` + relay URL only — what an invite ticket carries.
async fn dial(from: &Endpoint, to: iroh::EndpointId, relay: &Relay) -> Result<Connection> {
    let addr = EndpointAddr::new(to).with_relay_url(relay.url.clone());
    timeout(DIAL_TIMEOUT, from.connect(addr, ALPN))
        .await
        .context("dial timed out")?
        .context("dial failed")
}

async fn ping(conn: &Connection, msg: &str) -> Result<String> {
    let fut = async {
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(msg.as_bytes()).await?;
        send.finish()?;
        let out = recv.read_to_end(1024).await?;
        anyhow::Ok(String::from_utf8_lossy(&out).into_owned())
    };
    timeout(Duration::from_secs(5), fut).await.context("ping timed out")?
}

/// (relay paths, direct IP paths) on a connection.
fn paths(conn: &Connection) -> (usize, usize) {
    let list = conn.paths();
    let relay = list.iter().filter(|p| p.is_relay()).count();
    let ip = list.iter().filter(|p| p.is_ip()).count();
    (relay, ip)
}

fn own_ips(ep: &Endpoint) -> usize {
    ep.addr().addrs.iter().filter(|a| a.is_ip()).count()
}

struct Checks(Vec<(String, bool)>);

impl Checks {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn check(&mut self, what: impl Into<String>, ok: bool) {
        let what = what.into();
        println!("    [{}] {what}", if ok { "ok" } else { "FAIL" });
        self.0.push((what, ok));
    }
    fn finish(self, section: &str) -> Result<()> {
        let failed = self.0.iter().filter(|(_, ok)| !ok).count();
        println!("  {section}: {} checks, {failed} failed\n", self.0.len());
        if failed > 0 {
            bail!("{section}: {failed} check(s) failed");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// T1 — split: one process, a direct endpoint and a relay-only one
// ---------------------------------------------------------------------------

/// The proposal as written: D (direct) and R (relay-only), separate keys, in
/// one process and one tokio runtime. P is a stranger, a default endpoint.
/// What does P learn about our IP through each?
async fn split() -> Result<()> {
    println!("T1 split — D direct + R relay-only in one process; P is a stranger");
    let relay = spawn_relay().await?;
    let mut c = Checks::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let d = bind(&relay, SecretKey::generate(), Kind::Direct).await?;
    let r = bind(&relay, SecretKey::generate(), Kind::RelayOnly).await?;
    let p = bind(&relay, SecretKey::generate(), Kind::Direct).await?;
    let _sd = serve(d.clone(), "D", tx.clone());
    let _sr = serve(r.clone(), "R", tx.clone());
    let _sp = serve(p.clone(), "P", tx.clone());

    println!("    own EndpointAddr IPs: D {}, R {}", own_ips(&d), own_ips(&r));
    c.check("D advertises IP addresses (baseline is real)", own_ips(&d) > 0);
    c.check("R advertises zero IP addresses", own_ips(&r) == 0);
    c.check("D and R have different EndpointIds", d.id() != r.id());

    // P dials both, inbound to us.
    let p_to_d = dial(&p, d.id(), &relay).await?;
    let p_to_r = dial(&p, r.id(), &relay).await?;
    c.check("P → D answered by D", ping(&p_to_d, "x").await? == "D:x");
    c.check("P → R answered by R", ping(&p_to_r, "x").await? == "R:x");

    // R dials P, outbound from us.
    let r_to_p = dial(&r, p.id(), &relay).await?;
    c.check("R → P answered by P", ping(&r_to_p, "x").await? == "P:x");

    sleep(SETTLE).await;
    // P's side of the connection R opened.
    let mut p_side_of_r_dial = None;
    while let Ok((name, conn)) = rx.try_recv() {
        if name == "P" && conn.remote_id() == r.id() {
            p_side_of_r_dial = Some(conn);
        }
    }
    let p_side_of_r_dial = p_side_of_r_dial.context("P never saw R's connection")?;

    let (dr, di) = paths(&p_to_d);
    let (rr, ri) = paths(&p_to_r);
    let (or, oi) = paths(&p_side_of_r_dial);
    println!("    P's view, P → D: {dr} relay, {di} IP");
    println!("    P's view, P → R: {rr} relay, {ri} IP");
    println!("    P's view, R → P: {or} relay, {oi} IP");
    c.check("P holds a direct IP path to D (it learns D's IP)", di > 0);
    c.check("P holds no IP path to R when P dials", ri == 0);
    c.check("P holds no IP path to R when R dials", oi == 0);

    d.close().await;
    r.close().await;
    p.close().await;
    c.finish("T1 split")
}

// ---------------------------------------------------------------------------
// T2 — same key on two endpoints
// ---------------------------------------------------------------------------

/// HIT_ENTER_2 read, not ran: a relay keeps one *active* client per
/// `EndpointId`. Two endpoints sharing a key — the "one device key, two
/// endpoints" shape — should take turns being reachable. E1 is direct, E2
/// relay-only, same key; each dial is from a fresh stranger, so no stranger
/// remembers a direct path from an earlier dial.
async fn same_key() -> Result<()> {
    println!("T2 same-key — E1 (direct) and E2 (relay-only) share one SecretKey");
    let relay = spawn_relay().await?;
    let mut c = Checks::new();
    let (tx, _rx) = mpsc::unbounded_channel();
    let key = SecretKey::generate();

    let e1 = bind(&relay, key.clone(), Kind::Direct).await?;
    let _s1 = serve(e1.clone(), "E1", tx.clone());
    sleep(Duration::from_secs(1)).await;
    let e2 = bind(&relay, key.clone(), Kind::RelayOnly).await?;
    let _s2 = serve(e2.clone(), "E2", tx.clone());
    c.check("both endpoints bound with the same id", e1.id() == e2.id());

    // Dial every 2 s for 20 s and record who answered.
    let mut answers: BTreeMap<String, usize> = BTreeMap::new();
    let mut seq = String::new();
    for _ in 0..10 {
        let p = bind(&relay, SecretKey::generate(), Kind::Direct).await?;
        let who = match dial(&p, e1.id(), &relay).await {
            Ok(conn) => ping(&conn, "?")
                .await
                .map(|s| s.trim_end_matches(":?").to_string())
                .unwrap_or_else(|_| "no-reply".into()),
            Err(_) => "unreachable".into(),
        };
        seq.push_str(&format!("{who} "));
        *answers.entry(who).or_default() += 1;
        p.close().await;
        sleep(Duration::from_secs(2)).await;
    }
    println!("    both up, 10 dials: {seq}");
    println!("    tally: {answers:?}");
    // Recorded, not asserted: the sequence is the finding. The design only
    // needs to know whether one key can serve two endpoints reliably.
    let reliable = answers.get("E1").copied().unwrap_or(0) == 10
        || answers.get("E2").copied().unwrap_or(0) == 10;
    println!("    one endpoint answered every dial: {reliable}");

    // Close the newer one; does the older come back?
    e2.close().await;
    sleep(Duration::from_secs(2)).await;
    let p = bind(&relay, SecretKey::generate(), Kind::Direct).await?;
    let after = match dial(&p, e1.id(), &relay).await {
        Ok(conn) => ping(&conn, "?").await.unwrap_or_else(|e| format!("no-reply ({e})")),
        Err(e) => format!("unreachable ({e:#})"),
    };
    println!("    after E2 closes: {after}");
    c.check("after E2 closes, E1 is reachable again", after == "E1:?");

    p.close().await;
    e1.close().await;
    c.finish("T2 same-key")
}

// ---------------------------------------------------------------------------
// T4 — independence: closing one endpoint does not disturb the other
// ---------------------------------------------------------------------------

async fn independent() -> Result<()> {
    println!("T4 independent — close and rebind one endpoint while the other carries traffic");
    let relay = spawn_relay().await?;
    let mut c = Checks::new();
    let (tx, _rx) = mpsc::unbounded_channel();

    let d_key = SecretKey::generate();
    let d = bind(&relay, d_key.clone(), Kind::Direct).await?;
    let r = bind(&relay, SecretKey::generate(), Kind::RelayOnly).await?;
    let p = bind(&relay, SecretKey::generate(), Kind::Direct).await?;
    let _sd = serve(d.clone(), "D", tx.clone());
    let _sr = serve(r.clone(), "R", tx.clone());
    let _sp = serve(p.clone(), "P", tx.clone());

    let p_to_d = dial(&p, d.id(), &relay).await?;
    let p_to_r = dial(&p, r.id(), &relay).await?;
    c.check("both answer before", ping(&p_to_d, "1").await? == "D:1" && ping(&p_to_r, "1").await? == "R:1");

    d.close().await;
    c.check("D closed: P → R still answers", ping(&p_to_r, "2").await.ok().as_deref() == Some("R:2"));
    let r_to_p = dial(&r, p.id(), &relay).await;
    c.check(
        "D closed: R can still dial out",
        match &r_to_p {
            Ok(conn) => ping(conn, "2").await.ok().as_deref() == Some("P:2"),
            Err(_) => false,
        },
    );

    // Rebind D with the same key — what a "private mode" toggle or a network
    // change would do.
    let d2 = bind(&relay, d_key, Kind::Direct).await?;
    let _sd2 = serve(d2.clone(), "D", tx.clone());
    let p_to_d2 = dial(&p, d2.id(), &relay).await?;
    c.check("D rebound with the same key answers", ping(&p_to_d2, "3").await? == "D:3");
    c.check("R unaffected by D's rebind", ping(&p_to_r, "3").await.ok().as_deref() == Some("R:3"));

    r.close().await;
    c.check("R closed: P → D still answers", ping(&p_to_d2, "4").await.ok().as_deref() == Some("D:4"));

    d2.close().await;
    p.close().await;
    c.finish("T4 independent")
}

// ---------------------------------------------------------------------------
// T3 — cost
// ---------------------------------------------------------------------------

struct Sample {
    rss_kib: u64,
    threads: u64,
    fds: usize,
    sockets: usize,
    cpu_ticks: u64,
}

fn sample() -> Result<Sample> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let mut fds = 0;
    let mut sockets = 0;
    for entry in std::fs::read_dir("/proc/self/fd")? {
        fds += 1;
        if let Ok(target) = std::fs::read_link(entry?.path())
            && target.to_string_lossy().starts_with("socket:")
        {
            sockets += 1;
        }
    }
    // utime + stime, fields 14 and 15, after the `(comm)` which may hold spaces.
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    let cpu_ticks = f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
        + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Ok(Sample {
        rss_kib: field("VmRSS:"),
        threads: field("Threads:"),
        fds,
        sockets,
        cpu_ticks,
    })
}

/// One process per kind, so freed-but-retained allocator memory from one
/// kind never shows up in another's numbers. `pair` binds a direct and a
/// relay-only endpoint per step — the product's shape.
async fn cost(kind: &str, n: usize) -> Result<()> {
    let kinds: &[Kind] = match kind {
        "direct" => &[Kind::Direct],
        "relay-only" => &[Kind::RelayOnly],
        "pair" => &[Kind::Direct, Kind::RelayOnly],
        other => bail!("cost kind must be direct | relay-only | pair, not {other:?}"),
    };
    println!("T3 cost — {kind}, up to {n} step(s); relay in a child process");
    let relay = spawn_relay().await?;
    sleep(Duration::from_secs(2)).await;
    let base = sample()?;
    println!(
        "    {:>4} {:>9} {:>8} {:>7} {:>4} {:>8} {:>9}",
        "n", "RSS KiB", "Δ/step", "threads", "fds", "sockets", "bind ms"
    );
    println!(
        "    {:>4} {:>9} {:>8} {:>7} {:>4} {:>8} {:>9}",
        0, base.rss_kib, "-", base.threads, base.fds, base.sockets, "-"
    );
    let mut eps = Vec::new();
    let mut prev = base.rss_kib;
    for i in 1..=n {
        let t = std::time::Instant::now();
        for &k in kinds {
            eps.push(bind(&relay, SecretKey::generate(), k).await?);
        }
        let ms = t.elapsed().as_millis();
        sleep(Duration::from_secs(2)).await;
        let s = sample()?;
        println!(
            "    {:>4} {:>9} {:>8} {:>7} {:>4} {:>8} {:>9}",
            i,
            s.rss_kib,
            s.rss_kib as i64 - prev as i64,
            s.threads,
            s.fds,
            s.sockets,
            ms
        );
        prev = s.rss_kib;
    }
    let before = sample()?;
    sleep(Duration::from_secs(20)).await;
    let after = sample()?;
    // Linux USER_HZ is 100 on every mainstream build.
    let cpu_ms = (after.cpu_ticks - before.cpu_ticks) * 10;
    println!(
        "    idle 20 s with {} endpoint(s): {cpu_ms} ms CPU ({:.2}% of one core)",
        eps.len(),
        cpu_ms as f64 / 200.0
    );
    let per = (after.rss_kib - base.rss_kib) as f64 / n as f64;
    println!("    ≈ {per:.0} KiB RSS per step after idling\n");
    for ep in eps {
        ep.close().await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// T5 — real network: does a relay-only endpoint advertise an IP out there?
// ---------------------------------------------------------------------------

/// Loopback has no NAT and no UPnP gateway, so it cannot show whether QUIC
/// address discovery or the port mapper put a public address into a
/// relay-only endpoint's `EndpointAddr`. This uses n0's public relays and the
/// real gateway, waits, and prints **counts and kinds, never addresses** — so
/// its output is safe to paste into a README.
async fn real_net() -> Result<()> {
    println!("T5 real-net — n0 relays, default port mapper; counts only");
    let mut c = Checks::new();
    for (label, kind) in [("direct", Kind::Direct), ("relay-only", Kind::RelayOnly)] {
        let mut builder = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]);
        if kind == Kind::RelayOnly {
            builder = builder.clear_ip_transports();
        }
        let ep = builder.bind().await?;
        let online = timeout(Duration::from_secs(20), ep.online()).await.is_ok();
        sleep(Duration::from_secs(15)).await;
        let addr = ep.addr();
        let ips: Vec<_> = addr.ip_addrs().collect();
        let public = ips.iter().filter(|a| is_public(a.ip())).count();
        println!(
            "    {label:>10}: online {online}, relay {}, {} IP address(es), {public} of them public",
            addr.relay_urls().count(),
            ips.len()
        );
        if kind == Kind::RelayOnly {
            c.check("relay-only endpoint advertises zero IP addresses on a real network", ips.is_empty());
        }
        ep.close().await;
    }
    c.finish("T5 real-net")
}

fn is_public(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80)
        }
    }
}
