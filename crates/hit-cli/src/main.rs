//! Drives a `hit-node` from the terminal (M2–M3, `docs/PLAN.md`). There is
//! no curl for a QUIC protocol, so this is how relays and groups are tested
//! by hand — the recipe is `docs/TESTING.md`.
//!
//! A data directory holds `device.key`, `node.db` and `node.json` (the npub
//! and the peers we were told about). **No secret is in it**: the nsec and
//! every group key live in the OS keychain, under their npubs.
//!
//! Commands that only sign — `group create`, `invite`, `say`, `kick`, `ban`, `mute`,
//! `promote`, `compact` — work on the store alone, so they can run while
//! `run` is serving the same data directory: two endpoints on one device
//! key fail silently (spike S5). `run` pushes what they signed on its next
//! sync. `join`, `sync`, `req`, `lie` and `run` need the network, and must not run
//! alongside `run` on the same directory.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use hit_event::{GroupKey, Keychain, Person, group};
use hit_node::{Config, Node, Relays};
use hit_proto::invite::InviteLink;
use hit_store::Store;
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use nostr::prelude::{
    Event, EventBuilder, Filter, FromBech32, PublicKey, SingleLetterTag, Timestamp, ToBech32,
};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(name = "hit-cli", about = "Drive a HIT_ENTER node from the terminal")]
struct Cli {
    /// The node's data directory.
    #[arg(long, default_value = "hit-data")]
    data: PathBuf,
    /// Keychain service the nsec and group keys are filed under. Change it
    /// to keep test identities apart from a real one.
    #[arg(long, default_value = Keychain::APP_SERVICE)]
    keychain_service: String,
    /// Relays: n0's public ones, or none (direct addresses only).
    #[arg(long, value_enum, default_value = "n0")]
    relays: RelayArg,
    /// UDP port to bind (default: any).
    #[arg(long)]
    port: Option<u16>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum RelayArg {
    N0,
    None,
}

#[derive(Subcommand)]
enum Cmd {
    /// Make a new identity: the nsec goes to the keychain, the npub to node.json.
    Init,
    /// Print this node's npub and device id.
    Id,
    /// Print this node's current device binding (JSON).
    Binding,
    /// Add a device to sync with, and its address if it has no relay.
    Peer {
        id: String,
        /// `ip:port`, for relays off (a LAN, or processes on one machine).
        addr: Option<SocketAddr>,
    },
    /// Groups: create one, list them, show one.
    #[command(subcommand)]
    Group(GroupCmd),
    /// Mint an invite link to a group.
    Invite {
        group: String,
        /// Hours until it expires (default: never).
        #[arg(long)]
        hours: Option<u64>,
        /// Put this `ip:port` in the link, for relays off.
        #[arg(long)]
        addr: Option<SocketAddr>,
    },
    /// Join a group from an invite link.
    Join { link: String },
    /// Post to a group's channel.
    Say {
        group: String,
        text: String,
        #[arg(long, default_value = "general")]
        channel: String,
    },
    /// Remove a member (needs the kick permission and a higher rank).
    Kick { group: String, npub: String },
    /// Add a channel, or rename one (needs MANAGE_CHANNELS). A channel
    /// keeps its kind; `--voice` makes a new one a voice channel.
    Channel {
        group: String,
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        voice: bool,
    },
    /// Post a file (M8): adds it to this node's store, posts a message
    /// carrying it, and stays up serving it — to members who may read it —
    /// for `--serve-for` seconds. Needs the network, so not beside `run`.
    PostFile {
        group: String,
        path: PathBuf,
        #[arg(long, default_value = "")]
        text: String,
        #[arg(long, default_value = "general")]
        channel: String,
        #[arg(long, default_value_t = 60)]
        serve_for: u64,
    },
    /// Send a file in a DM, end to end (NIP-17 kind 15, M8b), and stay up
    /// serving its ciphertext to the two of you for `--serve-for` seconds.
    DmFile {
        npub: String,
        path: PathBuf,
        #[arg(long, default_value_t = 60)]
        serve_for: u64,
    },
    /// Fetch a file by its hash from whoever has it, and write it out.
    GetFile { hash: String, to: PathBuf },
    /// Remove a member and keep them out (needs the ban permission).
    Ban { group: String, npub: String },
    /// Lift a ban. Adds nobody back.
    Unban { group: String, npub: String },
    /// Mute a member for some minutes; `--minutes 0` lifts a mute.
    Mute {
        group: String,
        npub: String,
        #[arg(long, default_value_t = 10)]
        minutes: u64,
    },
    /// **The CLI, lying** (PLAN M5): sign a message and hand it straight to
    /// a device, without this node judging it first — as a patched client
    /// would. For testing that honest peers refuse it; an honest client
    /// never does this.
    Lie {
        /// The device to hand it to.
        id: String,
        group: String,
        text: String,
        #[arg(long, default_value = "general")]
        channel: String,
        /// Date it this many seconds in the past (a backdated message).
        #[arg(long, default_value_t = 0)]
        ago: u64,
    },
    /// Set a member's roles, replacing what they hold (empty: none).
    Promote {
        group: String,
        npub: String,
        roles: Vec<String>,
    },
    /// As a holder of the group key: sign a new snapshot at seq + 1.
    Compact { group: String },
    /// Print a group's messages, oldest first.
    Log {
        group: String,
        #[arg(long)]
        channel: Option<String>,
    },
    /// Sync once with every known device and print what moved.
    Sync {
        /// Present this binding (a JSON file) instead of our own — for the
        /// "copied binding is refused" test only.
        #[arg(long)]
        present_binding: Option<PathBuf>,
    },
    /// Ask one device what it serves us (a raw REQ), without storing it.
    Req {
        id: String,
        /// Only this group's events.
        #[arg(long)]
        group: Option<String>,
    },
    /// Serve, sync with every known device every few seconds, and print new
    /// messages as they arrive.
    Run {
        #[arg(long, default_value_t = 5)]
        every: u64,
    },
}

#[derive(Subcommand)]
enum GroupCmd {
    /// Create a group you own. Its key goes to the keychain.
    Create { name: String },
    /// The groups this node carries.
    List,
    /// One group's members, roles and channels.
    Show { group: String },
    /// Write the group's key to standard output — **only into a pipe**,
    /// never a terminal — to hand it to your anchor, which compacts the
    /// group while you are away (M6):
    /// `hit-cli group export-key G | ssh pi 'systemd-creds encrypt --name=<G npub> - …'`.
    /// Whoever holds it has the group's last word; treat it like an SSH host
    /// key.
    ExportKey { group: String },
}

/// `node.json`. Public facts only — never a secret.
#[derive(Serialize, Deserialize)]
struct NodeFile {
    npub: String,
    #[serde(default)]
    peers: Vec<PeerEntry>,
}

#[derive(Serialize, Deserialize)]
struct PeerEntry {
    id: String,
    addr: Option<SocketAddr>,
}

impl NodeFile {
    fn path(data: &Path) -> PathBuf {
        data.join("node.json")
    }

    fn load(data: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(Self::path(data))
            .with_context(|| format!("no node in {} — run `hit-cli init` first", data.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    fn save(&self, data: &Path) -> Result<()> {
        std::fs::write(Self::path(data), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    fn peers(&self) -> Result<Vec<EndpointAddr>> {
        self.peers
            .iter()
            .map(|p| {
                let id: EndpointId =
                    p.id.parse()
                        .with_context(|| format!("bad peer id {}", p.id))?;
                Ok(match p.addr {
                    Some(addr) => EndpointAddr::from_parts(id, [TransportAddr::Ip(addr)]),
                    None => EndpointAddr::new(id),
                })
            })
            .collect()
    }
}

fn person(cli: &Cli, file: &NodeFile) -> Result<Person> {
    let keychain = Keychain::new(&cli.keychain_service)?;
    Person::load(&keychain, &file.npub)?
        .with_context(|| format!("the keychain has no key for {}", file.npub))
}

fn npub_or_hex(s: &str) -> Result<PublicKey> {
    PublicKey::from_bech32(s)
        .or_else(|_| PublicKey::from_hex(s))
        .with_context(|| format!("not an npub: {s}"))
}

fn npub(pk: &PublicKey) -> String {
    pk.to_bech32().unwrap_or_else(|_| pk.to_hex())
}

fn short(pk: &PublicKey) -> String {
    npub(pk).chars().take(14).collect()
}

async fn open_store(cli: &Cli) -> Result<Store> {
    Ok(Store::open(&cli.data.join("node.db")).await?)
}

/// A group by npub, or by a unique name among the carried groups.
async fn find_group(store: &Store, me: PublicKey, name: &str) -> Result<PublicKey> {
    if let Ok(pk) = npub_or_hex(name) {
        return Ok(pk);
    }
    let groups = hit_node::groups_in(store, me).await?;
    let matches: Vec<PublicKey> = groups
        .carried()
        .filter(|g| groups.state(g).is_some_and(|s| s.name() == name))
        .copied()
        .collect();
    match matches.as_slice() {
        [g] => Ok(*g),
        [] => bail!("no group called {name:?} here"),
        _ => bail!("several groups are called {name:?}; use its npub"),
    }
}

/// Signs `builder` as this node's person and stores it, judged as a running
/// node would.
async fn sign_offline(store: &Store, me: &Person, builder: EventBuilder) -> Result<Event> {
    let ev = me.sign(builder)?;
    hit_node::publish_offline(store, me.public_key(), ev.clone()).await?;
    Ok(ev)
}

async fn start(cli: &Cli) -> Result<(Node, NodeFile)> {
    let file = NodeFile::load(&cli.data)?;
    let person = person(cli, &file)?;
    let node = Node::start(
        person,
        Config {
            data_dir: cli.data.clone(),
            relays: match cli.relays {
                RelayArg::N0 => Relays::N0,
                RelayArg::None => Relays::Disabled,
            },
            port: cli.port,
        },
    )
    .await?;
    for p in file.peers()? {
        node.add_peer(p);
    }
    Ok((node, file))
}

fn print_message(ev: &Event) {
    if ev.kind == group::CHAT_MESSAGE {
        println!(
            "{} #{} {}  {}",
            ev.created_at.as_secs(),
            group::channel_of(ev).unwrap_or("?"),
            short(&ev.pubkey),
            ev.content
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,hit_node=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    match &cli.cmd {
        Cmd::Init => {
            if NodeFile::path(&cli.data).exists() {
                bail!("{} already holds a node", cli.data.display());
            }
            std::fs::create_dir_all(&cli.data)?;
            let person = Person::generate();
            person.save(&Keychain::new(&cli.keychain_service)?)?;
            NodeFile {
                npub: person.npub(),
                peers: Vec::new(),
            }
            .save(&cli.data)?;
            let device = hit_node::device_id(&cli.data)?;
            println!("npub    {}", person.npub());
            println!("device  {device}");
            println!(
                "The secret key is in the OS keychain ({}), not in {}.",
                cli.keychain_service,
                cli.data.display()
            );
        }
        Cmd::Id => {
            let file = NodeFile::load(&cli.data)?;
            println!("npub    {}", file.npub);
            println!("device  {}", hit_node::device_id(&cli.data)?);
        }
        Cmd::Binding => {
            let (node, _) = start(&cli).await?;
            println!("{}", serde_json::to_string(&node.binding())?);
            node.shutdown().await;
        }
        Cmd::Peer { id, addr } => {
            let mut file = NodeFile::load(&cli.data)?;
            let _: EndpointId = id.parse().context("not a device id")?;
            file.peers.retain(|p| &p.id != id);
            file.peers.push(PeerEntry {
                id: id.clone(),
                addr: *addr,
            });
            file.save(&cli.data)?;
            println!(
                "peer {id}{}",
                addr.map(|a| format!(" at {a}")).unwrap_or_default()
            );
        }
        Cmd::Group(GroupCmd::Create { name }) => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let key = GroupKey::generate();
            key.save(&Keychain::new(&cli.keychain_service)?)?;
            let now = Timestamp::now();
            store.carry_group(&key.public_key(), now).await?;
            for ev in hit_node::genesis(&me, &key, name, false, now)? {
                hit_node::publish_offline(&store, me.public_key(), ev).await?;
            }
            println!("group   {}", key.npub());
            println!("The group's key is in the OS keychain, like an SSH host key.");
        }
        Cmd::Group(GroupCmd::List) => {
            let file = NodeFile::load(&cli.data)?;
            let me = npub_or_hex(&file.npub)?;
            let groups = hit_node::groups_in(&open_store(&cli).await?, me).await?;
            for g in groups.carried() {
                match groups.state(g) {
                    Some(s) => println!(
                        "{}  {:?}  seq {}  {} members{}",
                        npub(g),
                        s.name(),
                        s.seq(),
                        s.members().count(),
                        if s.is_member(&me) {
                            ""
                        } else {
                            "  (not a member)"
                        }
                    ),
                    None => println!("{}  (no state yet — joining?)", npub(g)),
                }
            }
        }
        Cmd::Group(GroupCmd::ExportKey { group: name }) => {
            use std::io::{IsTerminal, Write};
            if std::io::stdout().is_terminal() {
                bail!("refusing to print a group key to a terminal; pipe it into systemd-creds");
            }
            let file = NodeFile::load(&cli.data)?;
            let me = npub_or_hex(&file.npub)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me, name).await?;
            let key = GroupKey::load(&Keychain::new(&cli.keychain_service)?, &npub(&g))?
                .context("the keychain holds no key for this group")?;
            // Through a store in memory: the key's only way out is as text.
            let mem = hit_event::MemoryStore::default();
            key.save(&mem)?;
            let text = hit_event::SecretStore::get(&mem, &npub(&g))?
                .context("the key did not round-trip")?;
            let mut out = std::io::stdout().lock();
            out.write_all(text.as_bytes())?;
            out.flush()?;
            eprintln!("name its credential {}", npub(&g));
        }
        Cmd::Group(GroupCmd::Show { group: name }) => {
            let file = NodeFile::load(&cli.data)?;
            let me = npub_or_hex(&file.npub)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me, name).await?;
            let groups = hit_node::groups_in(&store, me).await?;
            let s = groups.state(&g).context("no state for that group yet")?;
            println!("{:?}  {}  seq {}", s.name(), npub(&g), s.seq());
            for c in s.channels() {
                println!("  #{}", c.id);
            }
            for p in s.members() {
                let roles: Vec<&str> = s
                    .roles_of(p)
                    .map(|r| r.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                println!(
                    "  {}{} {}",
                    npub(p),
                    if Some(*p) == s.owner() { " owner" } else { "" },
                    roles.join(",")
                );
            }
        }
        Cmd::Invite {
            group: name,
            hours,
            addr,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            let code = group::new_invite_code()?;
            let expires = hours.map(|h| Timestamp::now() + Duration::from_secs(h * 3_600));
            sign_offline(&store, &me, group::create_invite(&g, &code, expires)).await?;
            let device = hit_node::device_id(&cli.data)?;
            let link = InviteLink {
                group: g,
                code: Some(code),
                peers: hit_node::invite_peers(&store, me.public_key(), &g, device, *addr).await?,
            };
            println!("{link}");
        }
        Cmd::Join { link } => {
            let link: InviteLink = link.parse()?;
            let (node, _) = start(&cli).await?;
            let res = node.join(&link).await;
            match res {
                Ok(stats) => println!("joined {} — {stats}", npub(&link.group)),
                Err(e) => println!("join FAILED — {e}"),
            }
            node.shutdown().await;
        }
        Cmd::Say {
            group: name,
            text,
            channel,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            let ev = sign_offline(&store, &me, group::message(&g, channel, text)).await?;
            print_message(&ev);
        }
        Cmd::Kick {
            group: name,
            npub: who,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            sign_offline(&store, &me, group::remove_user(&g, &npub_or_hex(who)?)).await?;
            println!("removed {who}");
        }
        Cmd::PostFile {
            group: name,
            path,
            text,
            channel,
            serve_for,
        } => {
            let (node, _) = start(&cli).await?;
            let g = find_group(node.store(), node.person(), name).await?;
            let file = node.attach(path).await?;
            node.say_with(&g, channel, text, std::slice::from_ref(&file))
                .await?;
            println!("posted {} ({} bytes) {}", file.name, file.size, file.hash);
            let sync = node.spawn_sync_loop(Duration::from_secs(2));
            tokio::time::sleep(Duration::from_secs(*serve_for)).await;
            sync.abort();
            node.shutdown().await;
        }
        Cmd::DmFile {
            npub: who,
            path,
            serve_for,
        } => {
            let (node, _) = start(&cli).await?;
            let d = node.send_dm_file(&npub_or_hex(who)?, path).await?;
            if let Some(f) = &d.file {
                println!("sent {} ({} bytes), encrypted", f.name, f.size);
            }
            let sync = node.spawn_sync_loop(Duration::from_secs(2));
            tokio::time::sleep(Duration::from_secs(*serve_for)).await;
            sync.abort();
            node.shutdown().await;
        }
        Cmd::GetFile { hash, to } => {
            let (node, _) = start(&cli).await?;
            let sync = node.spawn_sync_loop(Duration::from_secs(2));
            // A first round of sync, so the message carrying it is here.
            tokio::time::sleep(Duration::from_secs(3)).await;
            match node.fetch_file(hash).await {
                Ok(()) => {
                    node.save_file(hash, to).await?;
                    println!("got {} bytes", std::fs::metadata(to)?.len());
                }
                Err(e) => println!("get FAILED — {e}"),
            }
            sync.abort();
            node.shutdown().await;
        }
        Cmd::Ban {
            group: name,
            npub: who,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            sign_offline(&store, &me, group::ban_user(&g, &npub_or_hex(who)?)).await?;
            println!("banned {who}");
        }
        Cmd::Unban {
            group: name,
            npub: who,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            sign_offline(&store, &me, group::unban_user(&g, &npub_or_hex(who)?)).await?;
            println!("unbanned {who}");
        }
        Cmd::Channel {
            group: name,
            id,
            name: title,
            voice,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            let kind = if *voice {
                group::ChannelKind::Voice
            } else {
                group::ChannelKind::Text
            };
            let ch = group::Channel {
                id: id.clone(),
                name: title.clone().unwrap_or_else(|| id.clone()),
                kind,
                slowmode: 0,
            };
            sign_offline(&store, &me, group::edit_metadata(&g, &[group::Edit::Channel(ch)])).await?;
            println!("channel {id} saved");
        }
        Cmd::Mute {
            group: name,
            npub: who,
            minutes,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            let until =
                (*minutes > 0).then(|| Timestamp::now() + Duration::from_secs(minutes * 60));
            sign_offline(&store, &me, group::mute_user(&g, &npub_or_hex(who)?, until)).await?;
            match until {
                Some(_) => println!("muted {who} for {minutes} min"),
                None => println!("unmuted {who}"),
            }
        }
        Cmd::Lie {
            id,
            group: name,
            text,
            channel,
            ago,
        } => {
            let id: EndpointId = id.parse().context("not a device id")?;
            let (node, _) = start(&cli).await?;
            let g = find_group(node.store(), node.person(), name).await?;
            let at = Timestamp::from(Timestamp::now().as_secs().saturating_sub(*ago));
            let ev = node.sign_unjudged(group::message(&g, channel, text).custom_created_at(at))?;
            match node.push_unjudged(EndpointAddr::new(id), ev).await {
                Ok((true, why)) => println!("TAKEN {why}"),
                Ok((false, why)) => println!("refused — {why}"),
                Err(e) => println!("FAILED — {e}"),
            }
            node.shutdown().await;
        }
        Cmd::Promote {
            group: name,
            npub: who,
            roles,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = person(&cli, &file)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me.public_key(), name).await?;
            let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
            sign_offline(&store, &me, group::put_user(&g, &npub_or_hex(who)?, &roles)).await?;
            println!("{who} now holds: {}", roles.join(","));
        }
        Cmd::Compact { group: name } => {
            let file = NodeFile::load(&cli.data)?;
            let me = npub_or_hex(&file.npub)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me, name).await?;
            let key = GroupKey::load(&Keychain::new(&cli.keychain_service)?, &npub(&g))?
                .context("the keychain holds no key for this group")?;
            let (seq, events) = hit_node::compaction(&store, &key, me).await?;
            for ev in events {
                hit_node::publish_offline(&store, me, ev).await?;
            }
            println!("compacted to seq {seq}");
        }
        Cmd::Log {
            group: name,
            channel,
        } => {
            let file = NodeFile::load(&cli.data)?;
            let me = npub_or_hex(&file.npub)?;
            let store = open_store(&cli).await?;
            let g = find_group(&store, me, name).await?;
            let h = SingleLetterTag::LOWERCASE_H;
            let mut msgs = store
                .query(
                    &Filter::new()
                        .kind(group::CHAT_MESSAGE)
                        .custom_tag(h, g.to_hex()),
                )
                .await?;
            msgs.reverse();
            msgs.retain(|e| {
                channel
                    .as_deref()
                    .is_none_or(|c| group::channel_of(e) == Some(c))
            });
            for ev in &msgs {
                print_message(ev);
            }
            eprintln!("{} messages", msgs.len());
        }
        Cmd::Sync { present_binding } => {
            let (node, _) = start(&cli).await?;
            let present: Option<Event> = match present_binding {
                Some(path) => Some(serde_json::from_str(&std::fs::read_to_string(path)?)?),
                None => None,
            };
            for id in node.sync_targets() {
                let peer = EndpointAddr::new(id);
                let res = match &present {
                    Some(b) => node.sync_presenting(peer, b.clone()).await,
                    None => node.sync_with(peer).await,
                };
                match res {
                    Ok(stats) => println!("{}: {stats}", id.fmt_short()),
                    Err(e) => println!("{}: FAILED — {e}", id.fmt_short()),
                }
            }
            node.shutdown().await;
        }
        Cmd::Req { id, group } => {
            let id: EndpointId = id.parse().context("not a device id")?;
            let (node, _) = start(&cli).await?;
            let filter = match group {
                Some(g) => {
                    Filter::new().custom_tag(SingleLetterTag::LOWERCASE_H, npub_or_hex(g)?.to_hex())
                }
                None => Filter::new(),
            };
            match node.fetch(EndpointAddr::new(id), filter).await {
                Ok(evs) => {
                    for ev in &evs {
                        println!(
                            "kind {:>5}  {}  {}",
                            ev.kind.as_u16(),
                            short(&ev.pubkey),
                            ev.content
                        );
                    }
                    println!("{} events served", evs.len());
                }
                Err(e) => println!("FAILED — {e}"),
            }
            node.shutdown().await;
        }
        Cmd::Run { every } => {
            let (node, _) = start(&cli).await?;
            let port = node.endpoint().bound_sockets().first().map(|s| s.port());
            println!("npub    {}", node.npub());
            println!("device  {}", node.id());
            if let Some(port) = port {
                println!("port    {port}");
            }
            let mut events = node.subscribe();
            tokio::spawn(async move {
                while let Ok(ev) = events.recv().await {
                    print_message(&ev);
                }
            });
            let sync = node.spawn_sync_loop(Duration::from_secs(*every));
            tokio::signal::ctrl_c().await?;
            sync.abort();
            node.shutdown().await;
        }
    }
    Ok(())
}
