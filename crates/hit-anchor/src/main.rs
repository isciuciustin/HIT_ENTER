//! `hit-anchor` — the always-on peer (M6). See `lib.rs`, and
//! `docs/SELF_HOSTING.md` for a Raspberry Pi and a €4 VPS.
//!
//! ```text
//! hit-anchor --data /var/lib/hit-anchor --credentials init --name "Friends' anchor" \
//!   | systemd-creds encrypt --name=<npub> - /etc/credstore.encrypted/<npub>
//! hit-anchor --data … --credentials pin '<invite link>'
//! hit-anchor --data … --credentials run --ws 127.0.0.1:7777
//! ```

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use hit_anchor::{secrets::Credentials, ws};
use hit_event::{GroupKey, Keychain, Person, SecretStore};
use hit_node::{Config, Node, Relays};
use hit_proto::invite::InviteLink;
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use nostr::prelude::ToBech32;
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(name = "hit-anchor", about = "The always-on HIT_ENTER peer")]
struct Cli {
    /// The anchor's data directory (device key, store, anchor.json).
    #[arg(long, default_value = "hit-anchor-data")]
    data: PathBuf,
    /// Read keys from systemd credentials (`$CREDENTIALS_DIRECTORY`) instead
    /// of the OS keychain — what a server without a desktop session has.
    #[arg(long)]
    credentials: bool,
    /// The keychain service, when not using credentials.
    #[arg(long, default_value = Keychain::APP_SERVICE)]
    keychain_service: String,
    /// `n0` (n0's relays and discovery), `none` (direct only), or the URL of
    /// your own `iroh-relay`.
    #[arg(long, default_value = "n0")]
    relays: String,
    /// UDP port to bind (default: any).
    #[arg(long)]
    port: Option<u16>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Make the anchor's identity. With `--credentials`, the key goes to
    /// standard output — pipe it into `systemd-creds encrypt`; it is never
    /// printed to a terminal.
    Init {
        /// The name members see for it.
        #[arg(long, default_value = "anchor")]
        name: String,
    },
    /// Print the anchor's npub and device id.
    Id,
    /// Tell it where a device is (relays off).
    Peer {
        id: String,
        addr: Option<SocketAddr>,
    },
    /// Pin a group: join it with an invite link, as a member, and say it is
    /// an anchor.
    Pin { link: String },
    /// Run: sync every pinned group, forever; compact groups whose key it
    /// holds; optionally serve NIP-29 clients over `ws://`.
    Run {
        /// Serve the `ws://` bridge here (put a TLS proxy in front for
        /// `wss://`).
        #[arg(long)]
        ws: Option<SocketAddr>,
        /// The public `wss://` URL, so AUTH events naming another relay are
        /// refused.
        #[arg(long)]
        ws_url: Option<String>,
        /// Compact each group whose key it holds this often, if anything
        /// happened (0: never).
        #[arg(long, default_value_t = 24)]
        compact_every_hours: u64,
        /// Seconds between syncs.
        #[arg(long, default_value_t = 3)]
        every: u64,
    },
}

/// `anchor.json`: public facts only.
#[derive(Serialize, Deserialize)]
struct AnchorFile {
    npub: String,
    name: String,
    #[serde(default)]
    peers: Vec<(String, Option<SocketAddr>)>,
}

impl AnchorFile {
    fn load(data: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(data.join("anchor.json")).with_context(|| {
            format!(
                "no anchor in {} — run `hit-anchor init` first",
                data.display()
            )
        })?;
        Ok(serde_json::from_str(&text)?)
    }

    fn save(&self, data: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(data)?;
        std::fs::write(
            data.join("anchor.json"),
            serde_json::to_string_pretty(self)?,
        )?;
        Ok(())
    }
}

fn secrets(cli: &Cli) -> Result<Box<dyn SecretStore + Send + Sync>> {
    Ok(if cli.credentials {
        Box::new(Credentials::from_env()?)
    } else {
        Box::new(Keychain::new(&cli.keychain_service)?)
    })
}

fn relays(cli: &Cli) -> Result<Relays> {
    Ok(match cli.relays.as_str() {
        "n0" => Relays::N0,
        "none" => Relays::Disabled,
        url => Relays::Custom(url.parse().context("--relays: n0, none, or a relay URL")?),
    })
}

async fn start(cli: &Cli) -> Result<(Node, AnchorFile, Box<dyn SecretStore + Send + Sync>)> {
    let file = AnchorFile::load(&cli.data)?;
    let store = secrets(cli)?;
    let person = Person::load(store.as_ref(), &file.npub)?
        .with_context(|| format!("no key for {} in the {}", file.npub, store_name(cli)))?;
    let node = Node::start(
        person,
        Config {
            data_dir: cli.data.clone(),
            relays: relays(cli)?,
            port: cli.port,
        },
    )
    .await?;
    for (id, addr) in &file.peers {
        let id: EndpointId = id.parse().context("bad peer id in anchor.json")?;
        node.add_peer(match addr {
            Some(a) => EndpointAddr::from_parts(id, [TransportAddr::Ip(*a)]),
            None => EndpointAddr::new(id),
        });
    }
    Ok((node, file, store))
}

fn store_name(cli: &Cli) -> &'static str {
    if cli.credentials {
        "systemd credentials"
    } else {
        "keychain"
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hit_anchor=info,hit_node=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Init { name } => {
            if cli.data.join("anchor.json").exists() {
                bail!("{} already holds an anchor", cli.data.display());
            }
            let person = Person::generate();
            if cli.credentials {
                hit_anchor::secrets::emit_to_pipe(&person.reveal_nsec_for_backup())?;
            } else {
                person.save(&Keychain::new(&cli.keychain_service)?)?;
            }
            AnchorFile {
                npub: person.npub(),
                name: name.clone(),
                peers: Vec::new(),
            }
            .save(&cli.data)?;
            eprintln!("npub    {}", person.npub());
            eprintln!("device  {}", hit_node::device_id(&cli.data)?);
            if cli.credentials {
                eprintln!(
                    "the key went to standard output; name its credential {}",
                    person.npub()
                );
            }
        }
        Cmd::Id => {
            let file = AnchorFile::load(&cli.data)?;
            println!("npub    {}", file.npub);
            println!("device  {}", hit_node::device_id(&cli.data)?);
        }
        Cmd::Peer { id, addr } => {
            let mut file = AnchorFile::load(&cli.data)?;
            let _: EndpointId = id.parse().context("not a device id")?;
            file.peers.retain(|(p, _)| p != id);
            file.peers.push((id.clone(), *addr));
            file.save(&cli.data)?;
            println!("peer {id} saved");
        }
        Cmd::Pin { link } => {
            let link: InviteLink = link.parse()?;
            let (node, file, _) = start(&cli).await?;
            let res = node.join(&link).await;
            match res {
                Ok(stats) => {
                    node.set_anchor_profile(&file.name).await?;
                    println!(
                        "pinned {} — {stats}",
                        link.group.to_bech32().unwrap_or_default()
                    );
                }
                Err(e) => println!("pin FAILED — {e}"),
            }
            node.shutdown().await;
        }
        Cmd::Run {
            ws: ws_addr,
            ws_url,
            compact_every_hours,
            every,
        } => {
            let (node, file, store) = start(&cli).await?;
            // Say it is an anchor, under its name, if its profile does not.
            let current = node.own_profile().await?;
            if current.as_ref().is_none_or(|p| {
                !hit_event::profile::is_anchor(p)
                    || hit_event::profile::name_of(p).as_deref() != Some(file.name.as_str())
            }) {
                node.set_anchor_profile(&file.name).await?;
            }
            // Where zap receipts for its groups can be delivered (M10): the
            // public `wss://` URL, when it serves one.
            let relay = ws_url.as_deref().filter(|_| ws_addr.is_some());
            let said = node
                .own_profile()
                .await?
                .as_ref()
                .and_then(hit_event::profile::relay_of);
            if said.as_deref() != relay {
                node.set_anchor_relay(relay).await?;
            }
            println!("npub    {}", node.npub());
            println!("device  {}", node.id());
            let sync = node.spawn_sync_loop(Duration::from_secs(*every));
            // An anchor keeps every file its groups carry (M8): pinned, and
            // fetched as soon as a message naming one arrives.
            node.set_file_policy(hit_node::FilePolicy {
                pin_all: true,
                cache_bytes: u64::MAX,
            });
            let pinner = {
                let n = node.clone();
                tokio::spawn(async move {
                    loop {
                        match n.pin_files().await {
                            Ok(0) => {}
                            Ok(k) => tracing::info!("pinned {k} new files"),
                            Err(e) => tracing::warn!("pinning files failed: {e}"),
                        }
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                })
            };
            let store: Arc<dyn SecretStore + Send + Sync> = Arc::from(store);
            let compaction = (*compact_every_hours > 0).then(|| {
                tokio::spawn(compact_loop(
                    node.clone(),
                    store.clone(),
                    Duration::from_secs(compact_every_hours * 3_600),
                ))
            });
            let bridge = match ws_addr {
                Some(addr) => {
                    let relay = Person::load(store.as_ref(), &file.npub)?
                        .context("the anchor's key vanished")?;
                    let b = ws::Bridge::new(node.clone(), relay, file.name.clone(), ws_url.clone());
                    let addr = *addr;
                    println!("ws      ws://{addr}");
                    Some(tokio::spawn(async move {
                        if let Err(e) = ws::serve(b, addr).await {
                            tracing::error!("the ws bridge stopped: {e}");
                        }
                    }))
                }
                None => None,
            };
            stopped().await?;
            sync.abort();
            pinner.abort();
            for t in [compaction, bridge].into_iter().flatten() {
                t.abort();
            }
            node.shutdown().await;
        }
    }
    Ok(())
}

/// Ctrl-C, or SIGTERM — what systemd sends to stop a service. Either way
/// the node hangs up properly instead of vanishing.
async fn stopped() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r?,
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

/// Every `every`: compact each group this anchor holds the key of and is a
/// member of, if anything happened since the last snapshot (`DESIGN.md` §6
/// step 4) — so the group gets compacted while its owner is away.
async fn compact_loop(node: Node, store: Arc<dyn SecretStore + Send + Sync>, every: Duration) {
    loop {
        tokio::time::sleep(every).await;
        let me = node.person();
        for (g, state) in node.groups() {
            let Some(s) = state.filter(|s| s.is_member(&me) && s.live_actions() > 0) else {
                continue;
            };
            let npub = g.to_bech32().unwrap_or_default();
            if !matches!(GroupKey::load(store.as_ref(), &npub), Ok(Some(_))) {
                continue;
            }
            match node.compact(&g, store.as_ref()).await {
                Ok(seq) => tracing::info!("compacted {} to seq {seq}", s.name()),
                Err(e) => tracing::warn!("compacting {} failed: {e}", s.name()),
            }
        }
    }
}
