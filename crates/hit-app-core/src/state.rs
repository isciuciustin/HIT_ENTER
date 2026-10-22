//! The app without the window: onboarding, the node's life, and every
//! question the window asks. No Tauri types, so all of it is tested without
//! a GUI (`tests` below); `commands.rs` is a thin layer over this.
//!
//! **Onboarding** (`DESIGN.md` §3, "honestly"): a new identity is generated
//! and filed in the keychain at once — a crash must not lose it — but the
//! node does not start, and no group can be created or joined, until the
//! person has saved a backup: an `ncryptsec` file, or the key shown once and
//! proved copied by typing its last characters back. `app.json` records
//! which step they are on, so quitting half way resumes there. An imported
//! key is already backed up — the person just typed it.
//!
//! **The nsec is shown only on the backup step**, never after it, and never
//! leaves this process except as the `ncryptsec` file the person asked for.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use hit_event::{KeyError, Person, SecretStore, group};
use hit_group::Permissions;
use hit_node::{Config, Node, NodeError, Relays};
use hit_proto::invite::InviteLink;
use hit_voice::{
    call::{CallState, Voice, VoiceError},
    device::{self, Devices},
};
use hit_zap::{LightningAddress, Quote, Wallet, Zapper};
use nostr::prelude::{Event, EventId, FinalizeEvent, FromBech32, Keys, PublicKey, ToBech32};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

/// How many characters of the key the person types back on the backup step.
pub const CONFIRM_CHARS: usize = 8;
/// The shortest password an `ncryptsec` backup may have.
pub const MIN_PASSWORD: usize = 8;
/// The longest message the composer sends. A courtesy, not a wire rule: the
/// frame cap (1 MiB) is the hard one.
pub const MAX_MESSAGE_CHARS: usize = 4000;

/// A microphone or speaker, as the audio settings list it.
pub use hit_voice::device::Device as DeviceView;

/// The longest DM, in characters (`hit_event::dm`).
pub const MAX_DM_CHARS: usize = hit_event::dm::MAX_DM_CHARS;
/// How often the node syncs with every device it knows.
const SYNC_EVERY: Duration = Duration::from_secs(3);
/// How often this app says "I am here" on its groups' gossip.
const PRESENCE_EVERY: Duration = Duration::from_secs(30);
/// Within this, someone whose presence reached us, or whose device completed
/// a session with us, counts as here: two heartbeats and a margin.
pub const PRESENT_WINDOW: Duration = Duration::from_secs(75);

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Refused(String),
}

impl From<hit_zap::Error> for AppError {
    fn from(e: hit_zap::Error) -> Self {
        Self::Refused(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

type Result<T> = std::result::Result<T, AppError>;

/// Where onboarding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// No identity on this device: make one or import one.
    New,
    /// An identity exists and is not backed up yet.
    Backup,
    /// Backed up; the node runs.
    Ready,
}

/// How this process reaches the network. The app uses n0's relays; tests
/// and the one-machine recipe turn them off and pin a port.
#[derive(Debug, Clone)]
pub struct Net {
    pub relays: Relays,
    pub port: Option<u16>,
    /// Put in invite links as the bootstrap device's address — only when
    /// relays are off, where an id alone finds nobody.
    pub invite_addr: Option<SocketAddr>,
    /// Devices and where they are, told to the node at start — what the
    /// CLI's `peer` does. With relays off nothing else finds a device after
    /// a restart; with relays on, discovery does and this stays empty.
    pub peers: Vec<(iroh::EndpointId, SocketAddr)>,
}

/// `app.json`: public facts only, never a secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AppFile {
    npub: Option<String>,
    backed_up: bool,
    /// The microphone and speakers for calls, by device id; `None` is the
    /// system's default.
    #[serde(default)]
    audio_input: Option<String>,
    #[serde(default)]
    audio_output: Option<String>,
    /// Echo cancellation off — for a headset that cancels its own, or a
    /// canceller that misbehaves.
    #[serde(default)]
    no_echo_cancel: bool,
}

/// Who is in each voice channel, and this person's call if any.
#[derive(Debug, Clone, Serialize)]
pub struct VoiceView {
    /// Group npub → channel id → the npubs in it.
    pub rooms: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    pub call: Option<CallState>,
    /// Someone calling this person (1:1): their npub, while it rings.
    pub incoming: Option<String>,
    /// Why the last call ended, for a few seconds after.
    pub ended: Option<String>,
    /// In a call: the devices' drops and underruns so far — each one a skip
    /// or a click somebody heard.
    pub audio: Option<device::Stats>,
}

/// The audio settings and what there is to choose from.
#[derive(Debug, Clone, Serialize)]
pub struct AudioSettings {
    pub devices: Devices,
    pub input: Option<String>,
    pub output: Option<String>,
    pub echo_cancel: bool,
}

/// Whether the Member role — which every member holds — is denied
/// `VIEW_CHANNEL` in `channel`.
fn is_private(s: &hit_group::GroupState, channel: &str) -> bool {
    s.overrides().iter().any(|o| {
        o.channel == channel
            && o.role == group::DEFAULT_ROLE
            && Permissions(o.deny).contains(Permissions::VIEW_CHANNEL)
    })
}

pub(crate) fn kind_name(k: group::ChannelKind) -> &'static str {
    match k {
        group::ChannelKind::Text => "text",
        group::ChannelKind::Voice => "voice",
    }
}

impl From<VoiceError> for AppError {
    fn from(e: VoiceError) -> Self {
        match e {
            VoiceError::Node(n) => Self::Node(n),
            other => Self::Refused(other.to_string()),
        }
    }
}

struct Running {
    node: Node,
    voice: Arc<Voice>,
    tasks: Vec<JoinHandle<()>>,
}

pub struct App {
    data_dir: PathBuf,
    secrets: Arc<dyn SecretStore + Send + Sync>,
    net: Net,
    file: Mutex<AppFile>,
    running: tokio::sync::RwLock<Option<Running>>,
    /// One `read.json` write at a time.
    read_lock: tokio::sync::Mutex<()>,
    /// The open audio devices' glitch counts, for the call bar.
    audio_watch: Mutex<Option<device::Watch>>,
    /// Zaps (M10): the one HTTP client, made on first use.
    zapper: std::sync::OnceLock<Zapper>,
    /// A zap being paid by QR code: its request, the provider whose
    /// receipt counts, and the fee owed after it.
    pending_zap: Mutex<Option<PendingZap>>,
}

struct PendingZap {
    request: Event,
    provider: PublicKey,
    fee_sats: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub stage: Stage,
    pub npub: Option<String>,
    /// Set when `app.json` names an identity the keychain does not hold:
    /// the person must import it again.
    pub lost: Option<String>,
    pub device: Option<String>,
    pub name: Option<String>,
    pub confirm_chars: usize,
    pub min_password: usize,
    pub max_message_chars: usize,
    /// Whether this node uses relays — the app says what that means for
    /// the person's IP address.
    pub relays: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelView {
    pub id: String,
    pub name: String,
    /// `text` or `voice`.
    pub kind: &'static str,
    /// A plain member may not view it (the Member role is denied): its
    /// events — and, for a voice channel, who is in it — go only to those
    /// who may.
    pub private: bool,
    pub can_send: bool,
    pub slowmode: u32,
    /// May remove anyone's messages here.
    pub can_moderate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub id: String,
    pub name: String,
    /// Channels this person may see, in the snapshot's order.
    pub channels: Vec<ChannelView>,
    pub member_count: usize,
    pub owner: Option<String>,
    pub can_invite: bool,
    /// False while joined but before the group's state arrived.
    pub ready: bool,
    /// What this person may do here — a courtesy for the window; every peer
    /// judges the action again.
    pub me_owner: bool,
    pub can_rename: bool,
    pub can_manage_channels: bool,
    pub can_ban: bool,
    /// Holds the group key, so may compact (the owner, on a device that
    /// made or was handed the key).
    pub can_compact: bool,
    /// Until when this person is muted here (seconds), if they are.
    pub muted_until: Option<u64>,
    /// The snapshot's `seq`: how many compactions the group has had.
    pub seq: u64,
    /// Was a member, is not now: kicked, banned, or left. The history on
    /// this disk stays readable; nothing can be sent.
    pub removed: bool,
    pub banned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleView {
    pub id: String,
    pub name: String,
    pub position: i64,
    /// This person may give or take it.
    pub assignable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BanView {
    pub npub: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberView {
    pub npub: String,
    pub name: Option<String>,
    /// Role names, highest first; empty for a plain member.
    pub roles: Vec<String>,
    /// Role ids, for the role picker.
    pub role_ids: Vec<String>,
    pub owner: bool,
    pub me: bool,
    pub can_kick: bool,
    pub can_ban: bool,
    pub can_mute: bool,
    pub can_set_roles: bool,
    pub muted_until: Option<u64>,
    /// An anchor (M6): the group's always-on peer, a member like any other.
    pub anchor: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub id: String,
    pub group: String,
    pub channel: String,
    pub author: String,
    pub text: String,
    pub at: u64,
    pub mine: bool,
    /// Every reaction held for it, oldest first.
    pub reactions: Vec<ReactionView>,
    /// Its files (M8).
    pub files: Vec<FileView>,
    /// What it was zapped (M10), counting only receipts its author's
    /// provider signed.
    pub zap_sats: u64,
    pub zap_count: usize,
}

/// A file, as the window shows and sends it (M8). The same fields as
/// `hit_event::group::Attachment`; the window hands back what `attach`
/// gave it, and the node checks every limit again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileView {
    pub hash: String,
    pub sha256: String,
    pub size: u64,
    pub mime: String,
    pub name: String,
}

impl From<group::Attachment> for FileView {
    fn from(a: group::Attachment) -> Self {
        Self {
            hash: a.hash,
            sha256: a.sha256,
            size: a.size,
            mime: a.mime,
            name: a.name,
        }
    }
}

impl From<FileView> for group::Attachment {
    fn from(f: FileView) -> Self {
        Self {
            hash: f.hash,
            sha256: f.sha256,
            size: f.size,
            mime: f.mime,
            name: f.name,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReactionView {
    pub id: String,
    /// The message it reacts to.
    pub message: String,
    pub group: String,
    pub channel: String,
    pub emoji: String,
    pub author: String,
    pub mine: bool,
}

/// A DM as the window shows it (M7).
#[derive(Debug, Clone, Serialize)]
pub struct DmView {
    pub id: String,
    /// The other person in the conversation.
    pub with: String,
    pub mine: bool,
    pub text: String,
    pub at: u64,
    /// A file (M8b): what the window may know of it — never its key, which
    /// stays on the Rust side; the window asks for the bytes by DM id.
    pub file: Option<DmFileView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DmFileView {
    pub hash: String,
    pub size: u64,
    pub mime: String,
    pub name: String,
}

/// Where this person's DMs wait while they are away.
#[derive(Debug, Clone, Serialize)]
pub struct InboxView {
    /// Their inbox list names at least one anchor.
    pub enabled: bool,
    /// The anchors it names (npubs).
    pub named: Vec<String>,
    /// The anchors of their groups, which it could name.
    pub available: Vec<String>,
}

pub fn dm_view(d: &hit_event::dm::Dm, me: &PublicKey) -> DmView {
    let mine = d.from == *me;
    DmView {
        id: d.id.clone(),
        with: npub(if mine { &d.to } else { &d.from }),
        mine,
        text: d.text.clone(),
        at: d.at.as_secs(),
        file: d.file.as_ref().map(|f| DmFileView {
            hash: f.hash.clone(),
            size: f.size,
            mime: f.mime.clone(),
            name: f.name.clone(),
        }),
    }
}

/// How many messages arrived after the last one read, per channel.
#[derive(Debug, Clone, Serialize)]
pub struct Unread {
    pub group: String,
    pub channel: String,
    pub count: usize,
}

pub fn npub(p: &PublicKey) -> String {
    p.to_bech32().unwrap_or_else(|_| p.to_hex())
}

fn parse_key(s: &str) -> Result<PublicKey> {
    PublicKey::from_bech32(s.trim())
        .or_else(|_| PublicKey::parse(s.trim()))
        .map_err(|_| AppError::Refused("not a valid key".into()))
}

/// A chat message as the window shows it; `None` for anything else.
pub fn message_view(ev: &Event, me: &PublicKey) -> Option<MessageView> {
    if ev.kind != group::CHAT_MESSAGE {
        return None;
    }
    Some(MessageView {
        id: ev.id.to_hex(),
        group: npub(&group::group_of(ev)?),
        channel: group::channel_of(ev)?.to_owned(),
        author: npub(&ev.pubkey),
        text: ev.content.clone(),
        at: ev.created_at.as_secs(),
        mine: ev.pubkey == *me,
        reactions: Vec::new(),
        files: group::attachments_of(ev)
            .unwrap_or_default()
            .into_iter()
            .map(FileView::from)
            .collect(),
        zap_sats: 0,
        zap_count: 0,
    })
}

/// A reaction as the window shows it; `None` for anything else.
pub fn reaction_view(ev: &Event, me: &PublicKey) -> Option<ReactionView> {
    if ev.kind != group::REACTION {
        return None;
    }
    Some(ReactionView {
        id: ev.id.to_hex(),
        message: group::targets_of(ev).first()?.to_hex(),
        group: npub(&group::group_of(ev)?),
        channel: group::channel_of(ev)?.to_owned(),
        emoji: ev.content.clone(),
        author: npub(&ev.pubkey),
        mine: ev.pubkey == *me,
    })
}

/// A channel id from its name: lowercase letters, digits and dashes, and a
/// number on the end if `taken` says so.
fn channel_id(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug = slug.chars().take(32).collect();
    if slug.is_empty() {
        slug = "channel".into();
    }
    if !taken(&slug) {
        return slug;
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|s| !taken(s))
        .unwrap_or(slug)
}

fn parse_id(s: &str) -> Result<EventId> {
    EventId::from_hex(s.trim()).map_err(|_| AppError::Refused("not a valid event id".into()))
}

impl App {
    /// Opens the app's data directory and, if this device's identity is
    /// backed up, starts its node.
    pub async fn open(
        data_dir: &Path,
        secrets: Arc<dyn SecretStore + Send + Sync>,
        net: Net,
    ) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let file = match std::fs::read(data_dir.join("app.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| AppError::Io(e.to_string()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AppFile::default(),
            Err(e) => return Err(e.into()),
        };
        let app = Self {
            data_dir: data_dir.to_path_buf(),
            secrets,
            net,
            file: Mutex::new(file),
            running: tokio::sync::RwLock::new(None),
            read_lock: tokio::sync::Mutex::new(()),
            audio_watch: Mutex::new(None),
            zapper: std::sync::OnceLock::new(),
            pending_zap: Mutex::new(None),
        };
        if app.stage() == Stage::Ready
            && let Some(person) = app.person()?
        {
            app.start(person).await?;
        }
        Ok(app)
    }

    fn file(&self) -> std::sync::MutexGuard<'_, AppFile> {
        self.file.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save_file(&self, next: AppFile) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| AppError::Io(e.to_string()))?;
        let tmp = self.data_dir.join("app.json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, self.data_dir.join("app.json"))?;
        *self.file() = next;
        Ok(())
    }

    /// This device's person, from the keychain. `None` with no identity,
    /// or with one the keychain has lost.
    fn person(&self) -> Result<Option<Person>> {
        let Some(npub) = self.file().npub.clone() else {
            return Ok(None);
        };
        Ok(Person::load(self.secrets.as_ref(), &npub)?)
    }

    pub fn stage(&self) -> Stage {
        let f = self.file();
        match (&f.npub, f.backed_up) {
            (None, _) => Stage::New,
            (Some(_), false) => Stage::Backup,
            (Some(_), true) => Stage::Ready,
        }
    }

    pub async fn status(&self) -> Result<Status> {
        let npub = self.file().npub.clone();
        let lost = match &npub {
            Some(n) if self.person()?.is_none() => Some(n.clone()),
            _ => None,
        };
        let running = self.running.read().await;
        let (device, name) = match running.as_ref() {
            Some(r) => {
                let me = r.node.person();
                let name = r.node.names(&[me]).await?.remove(&me);
                (Some(r.node.id().to_string()), name)
            }
            None => (None, None),
        };
        Ok(Status {
            stage: if lost.is_some() {
                Stage::New
            } else {
                self.stage()
            },
            npub: if lost.is_some() { None } else { npub },
            lost,
            device,
            name,
            confirm_chars: CONFIRM_CHARS,
            min_password: MIN_PASSWORD,
            max_message_chars: MAX_MESSAGE_CHARS,
            relays: !matches!(self.net.relays, Relays::Disabled),
        })
    }

    // --- onboarding ------------------------------------------------------

    /// Makes a new identity: the key into the keychain, the npub into
    /// `app.json`, and the backup step next.
    pub fn create_identity(&self) -> Result<String> {
        self.require_new()?;
        let person = Person::generate();
        person.save(self.secrets.as_ref())?;
        let npub = person.npub();
        // A new identity keeps the device's audio settings.
        let base = self.file().clone();
        self.save_file(AppFile {
            npub: Some(npub.clone()),
            backed_up: false,
            ..base
        })?;
        Ok(npub)
    }

    /// Imports an existing `nsec`, or an `ncryptsec` with its password.
    /// Already backed up — the person is holding it — so the node starts.
    pub async fn import_identity(&self, secret: &str, password: Option<&str>) -> Result<String> {
        self.require_new()?;
        let secret = Zeroizing::new(secret.trim().to_owned());
        let person = if secret.starts_with("ncryptsec") {
            let password = password.ok_or_else(|| {
                AppError::Refused("an ncryptsec needs the password it was made with".into())
            })?;
            Person::from_ncryptsec(&secret, password)?
        } else {
            Person::from_nsec(&secret)?
        };
        person.save(self.secrets.as_ref())?;
        let npub = person.npub();
        // A new identity keeps the device's audio settings.
        let base = self.file().clone();
        self.save_file(AppFile {
            npub: Some(npub.clone()),
            backed_up: true,
            ..base
        })?;
        self.start(person).await?;
        Ok(npub)
    }

    /// The key, for the backup step — and only on it.
    pub fn reveal_key(&self) -> Result<Zeroizing<String>> {
        if self.stage() != Stage::Backup {
            return Err(AppError::Refused(
                "the key is shown only while it is being backed up".into(),
            ));
        }
        let person = self.person()?.ok_or_else(Self::lost)?;
        Ok(person.reveal_nsec_for_backup())
    }

    /// The backup step, by the key shown once: `tail` must be its last
    /// [`CONFIRM_CHARS`] characters — proof it was copied somewhere.
    pub async fn confirm_backup(&self, tail: &str) -> Result<()> {
        let key = self.reveal_key()?;
        let want = &key[key.len().saturating_sub(CONFIRM_CHARS)..];
        if tail.trim() != want {
            return Err(AppError::Refused(
                "those are not the key's last characters".into(),
            ));
        }
        self.backed_up().await
    }

    /// The backup step, by file: the key encrypted with `password` (NIP-49)
    /// and written to `path`, which the person chose. Also offered after
    /// onboarding: it never shows the key.
    pub async fn backup_to_file(&self, path: &Path, password: &str) -> Result<()> {
        if password.chars().count() < MIN_PASSWORD {
            return Err(AppError::Refused(format!(
                "the password needs at least {MIN_PASSWORD} characters"
            )));
        }
        let person = self.person()?.ok_or_else(Self::lost)?;
        let ncryptsec = person.to_ncryptsec(password)?;
        std::fs::write(path, format!("{ncryptsec}\n"))?;
        if self.stage() == Stage::Backup {
            self.backed_up().await?;
        }
        Ok(())
    }

    async fn backed_up(&self) -> Result<()> {
        let mut next = self.file().clone();
        next.backed_up = true;
        self.save_file(next)?;
        let person = self.person()?.ok_or_else(Self::lost)?;
        self.start(person).await
    }

    fn require_new(&self) -> Result<()> {
        if self.file().npub.is_some() && self.person()?.is_some() {
            return Err(AppError::Refused(
                "this device already has an identity".into(),
            ));
        }
        Ok(())
    }

    fn lost() -> AppError {
        AppError::Refused("the keychain no longer holds this identity's key".into())
    }

    async fn start(&self, person: Person) -> Result<()> {
        let mut running = self.running.write().await;
        if running.is_some() {
            return Ok(());
        }
        let node = Node::start(
            person,
            Config {
                data_dir: self.data_dir.clone(),
                relays: self.net.relays.clone(),
                port: self.net.port,
            },
        )
        .await?;
        for (id, addr) in &self.net.peers {
            node.add_peer(iroh::EndpointAddr::from_parts(
                *id,
                [iroh::TransportAddr::Ip(*addr)],
            ));
        }
        tracing::info!("this device: {}", node.id());
        let sync = node.spawn_sync_loop(SYNC_EVERY);
        let n = node.clone();
        let presence = tokio::spawn(async move {
            loop {
                if let Err(e) = n.announce_presence().await {
                    tracing::debug!("announcing presence failed: {e}");
                }
                tokio::time::sleep(PRESENCE_EVERY).await;
            }
        });
        let voice = Voice::new(node.clone(), hit_voice::call::Config::default());
        *running = Some(Running {
            node,
            voice,
            tasks: vec![sync, presence],
        });
        Ok(())
    }

    /// The running node, once onboarding is done.
    pub async fn node(&self) -> Result<Node> {
        self.running
            .read()
            .await
            .as_ref()
            .map(|r| r.node.clone())
            .ok_or_else(|| AppError::Refused("finish setting up first".into()))
    }

    /// Runs `task` until shutdown, beside the node.
    pub async fn attach(&self, task: JoinHandle<()>) {
        if let Some(r) = self.running.write().await.as_mut() {
            r.tasks.push(task);
        } else {
            task.abort();
        }
    }

    pub async fn shutdown(&self) {
        if let Some(r) = self.running.write().await.take() {
            // Out of the call first, so the others drop us now rather than
            // when our presence goes stale.
            if r.voice.state().is_some() {
                let _ = r.voice.leave().await;
            }
            for t in r.tasks {
                t.abort();
            }
            r.node.shutdown().await;
        }
    }

    // --- the chat --------------------------------------------------------

    pub async fn set_name(&self, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(AppError::Refused("a name cannot be empty".into()));
        }
        self.node().await?.set_name(name).await?;
        Ok(())
    }

    pub async fn groups(&self) -> Result<Vec<GroupView>> {
        let node = self.node().await?;
        let me = node.person();
        let mut out: Vec<GroupView> = node
            .groups()
            .into_iter()
            .map(|(g, state)| match state {
                Some(s) if s.is_member(&me) => {
                    let channels = s
                        .channels()
                        .iter()
                        .filter(|c| s.effective(&me, &c.id).contains(Permissions::VIEW_CHANNEL))
                        .map(|c| {
                            let here = s.effective(&me, &c.id);
                            ChannelView {
                                id: c.id.clone(),
                                name: c.name.clone(),
                                kind: kind_name(c.kind),
                                private: is_private(&s, &c.id),
                                can_send: here.contains(Permissions::SEND_MESSAGES),
                                slowmode: c.slowmode,
                                can_moderate: here.contains(Permissions::MANAGE_MESSAGES),
                            }
                        })
                        .collect();
                    let base = s.base(&me);
                    let me_owner = s.owner() == Some(me);
                    GroupView {
                        id: npub(&g),
                        name: s.name().to_owned(),
                        channels,
                        member_count: s.members().count(),
                        owner: s.owner().map(|o| npub(&o)),
                        can_invite: base.contains(Permissions::CREATE_INVITE),
                        ready: true,
                        me_owner,
                        can_rename: base.contains(Permissions::MANAGE_GROUP),
                        can_manage_channels: base.contains(Permissions::MANAGE_CHANNELS),
                        can_ban: base.contains(Permissions::BAN_MEMBERS),
                        can_compact: me_owner
                            && hit_event::GroupKey::load(self.secrets.as_ref(), &npub(&g))
                                .is_ok_and(|k| k.is_some()),
                        muted_until: s
                            .muted_until(&me, nostr::prelude::Timestamp::now())
                            .map(|t| t.as_secs()),
                        seq: s.seq(),
                        removed: false,
                        banned: false,
                    }
                }
                // No longer a member: the channels, read-only, over the
                // history this disk holds.
                Some(s) => GroupView {
                    id: npub(&g),
                    name: s.name().to_owned(),
                    channels: s
                        .channels()
                        .iter()
                        .map(|c| ChannelView {
                            id: c.id.clone(),
                            name: c.name.clone(),
                            kind: kind_name(c.kind),
                            private: is_private(&s, &c.id),
                            can_send: false,
                            slowmode: c.slowmode,
                            can_moderate: false,
                        })
                        .collect(),
                    member_count: s.members().count(),
                    owner: s.owner().map(|o| npub(&o)),
                    can_invite: false,
                    ready: true,
                    me_owner: false,
                    can_rename: false,
                    can_manage_channels: false,
                    can_ban: false,
                    can_compact: false,
                    muted_until: None,
                    seq: s.seq(),
                    removed: true,
                    banned: s.is_banned(&me),
                },
                // Carried without state yet: a join on its way.
                None => GroupView {
                    id: npub(&g),
                    name: String::new(),
                    channels: Vec::new(),
                    member_count: 0,
                    owner: None,
                    can_invite: false,
                    ready: false,
                    me_owner: false,
                    can_rename: false,
                    can_manage_channels: false,
                    can_ban: false,
                    can_compact: false,
                    muted_until: None,
                    seq: 0,
                    removed: false,
                    banned: false,
                },
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// The members of `group`, owner first, then by rank, then by name.
    pub async fn members(&self, group: &str) -> Result<Vec<MemberView>> {
        let node = self.node().await?;
        let g = parse_key(group)?;
        let me = node.person();
        let state = node.group(&g).ok_or(NodeError::NoSuchGroup)?;
        let snap = state.to_snapshot();
        let role_names: BTreeMap<&str, (&str, i64)> = snap
            .roles
            .iter()
            .map(|r| (r.id.as_str(), (r.name.as_str(), r.position)))
            .collect();
        let people: Vec<PublicKey> = state.members().copied().collect();
        let names = node.names(&people).await?;
        let anchors = node.anchors(&g).await?;
        let mut out: Vec<(i64, MemberView)> = people
            .iter()
            .map(|p| {
                let role_ids: Vec<String> =
                    state.roles_of(p).into_iter().flatten().cloned().collect();
                let mut roles: Vec<(&str, i64)> = role_ids
                    .iter()
                    .filter_map(|r| role_names.get(r.as_str()).copied())
                    .collect();
                roles.sort_by_key(|(_, pos)| std::cmp::Reverse(*pos));
                let over = state.outranks(&me, p);
                let base = state.base(&me);
                let may = |perm| over && base.contains(perm);
                let owner = state.owner() == Some(*p);
                let rank = if owner {
                    i64::MAX
                } else {
                    roles.first().map_or(0, |(_, pos)| *pos)
                };
                let view = MemberView {
                    npub: npub(p),
                    name: names.get(p).cloned(),
                    roles: roles.into_iter().map(|(n, _)| n.to_owned()).collect(),
                    role_ids,
                    owner,
                    me: *p == me,
                    can_kick: may(Permissions::KICK_MEMBERS),
                    can_ban: may(Permissions::BAN_MEMBERS),
                    can_mute: may(Permissions::MUTE_MEMBERS),
                    can_set_roles: may(Permissions::MANAGE_ROLES),
                    muted_until: state
                        .muted_until(p, nostr::prelude::Timestamp::now())
                        .map(|t| t.as_secs()),
                    anchor: anchors.contains(p),
                };
                (rank, view)
            })
            .collect();
        out.sort_by(|(ra, a), (rb, b)| {
            rb.cmp(ra)
                .then_with(|| a.name.is_none().cmp(&b.name.is_none()))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.npub.cmp(&b.npub))
        });
        Ok(out.into_iter().map(|(_, v)| v).collect())
    }

    /// The roles of `group` above the default one, highest first, and which
    /// this person may hand out.
    pub async fn roles(&self, group: &str) -> Result<Vec<RoleView>> {
        let node = self.node().await?;
        let me = node.person();
        let state = node
            .group(&parse_key(group)?)
            .ok_or(NodeError::NoSuchGroup)?;
        let mut out: Vec<RoleView> = state
            .roles()
            .filter(|r| r.id != group::DEFAULT_ROLE)
            .map(|r| RoleView {
                id: r.id.clone(),
                name: r.name.clone(),
                position: r.position,
                assignable: state.may_assign(&me, &r.id),
            })
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.position));
        Ok(out)
    }

    /// Who is banned from `group`.
    pub async fn bans(&self, group: &str) -> Result<Vec<BanView>> {
        let node = self.node().await?;
        let state = node
            .group(&parse_key(group)?)
            .ok_or(NodeError::NoSuchGroup)?;
        let people: Vec<PublicKey> = state.bans().copied().collect();
        let names = node.names(&people).await?;
        Ok(people
            .iter()
            .map(|p| BanView {
                npub: npub(p),
                name: names.get(p).cloned(),
            })
            .collect())
    }

    // --- moderation (M5) -------------------------------------------------
    //
    // Each is one signed action; every peer judges it again. A refusal here
    // is the local node's judgement, said before anyone else is bothered.

    pub async fn kick(&self, group: &str, who: &str) -> Result<()> {
        self.node()
            .await?
            .remove(&parse_key(group)?, &parse_key(who)?)
            .await?;
        Ok(())
    }

    pub async fn ban(&self, group: &str, who: &str) -> Result<()> {
        self.node()
            .await?
            .ban(&parse_key(group)?, &parse_key(who)?)
            .await?;
        Ok(())
    }

    pub async fn unban(&self, group: &str, who: &str) -> Result<()> {
        self.node()
            .await?
            .unban(&parse_key(group)?, &parse_key(who)?)
            .await?;
        Ok(())
    }

    /// Mutes `who` for `minutes`, or lifts their mute (`None`).
    pub async fn mute(&self, group: &str, who: &str, minutes: Option<u32>) -> Result<()> {
        let until = minutes.map(|m| nostr::prelude::Timestamp::now() + u64::from(m) * 60);
        self.node()
            .await?
            .mute(&parse_key(group)?, &parse_key(who)?, until)
            .await?;
        Ok(())
    }

    /// Gives `who` exactly `roles` (ids) beyond the default one.
    pub async fn set_roles(&self, group: &str, who: &str, roles: &[String]) -> Result<()> {
        let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
        self.node()
            .await?
            .put(&parse_key(group)?, &parse_key(who)?, &roles)
            .await?;
        Ok(())
    }

    /// A moderator removing someone's message (`9005`).
    pub async fn remove_message(&self, group: &str, channel: &str, id: &str) -> Result<()> {
        self.node()
            .await?
            .remove_events(&parse_key(group)?, channel, &[parse_id(id)?])
            .await?;
        Ok(())
    }

    pub async fn rename_group(&self, group: &str, name: &str) -> Result<()> {
        self.node()
            .await?
            .edit(
                &parse_key(group)?,
                &[group::Edit::Name(name.trim().to_owned())],
            )
            .await?;
        Ok(())
    }

    /// Creates a channel (`id` `None`, a voice channel if `voice`) or
    /// changes one: its name and slowmode. A channel keeps its kind.
    pub async fn save_channel(
        &self,
        group: &str,
        id: Option<&str>,
        name: &str,
        slowmode: u32,
        voice: bool,
    ) -> Result<String> {
        let g = parse_key(group)?;
        let node = self.node().await?;
        let state = node.group(&g).ok_or(NodeError::NoSuchGroup)?;
        let name = name.trim().trim_start_matches('#').trim();
        let id = match id {
            Some(id) => id.to_owned(),
            None => channel_id(name, |c| state.channels().iter().any(|x| x.id == c)),
        };
        let kind = state.channels().iter().find(|c| c.id == id).map_or(
            if voice {
                group::ChannelKind::Voice
            } else {
                group::ChannelKind::Text
            },
            |c| c.kind,
        );
        node.edit(
            &g,
            &[group::Edit::Channel(group::Channel {
                id: id.clone(),
                name: name.to_owned(),
                kind,
                slowmode,
            })],
        )
        .await?;
        Ok(id)
    }

    /// Makes a channel private — members may not view it, moderators and
    /// up may — or public again. The app's one shape of override; finer
    /// ones are the CLI's. Needs someone who outranks every role: in
    /// practice the owner.
    pub async fn set_channel_private(&self, group: &str, id: &str, private: bool) -> Result<()> {
        let g = parse_key(group)?;
        let node = self.node().await?;
        let state = node.group(&g).ok_or(NodeError::NoSuchGroup)?;
        let view = Permissions::VIEW_CHANNEL.bits();
        let edits: Vec<group::Edit> = state
            .roles()
            .map(|r| {
                let member = r.id == group::DEFAULT_ROLE;
                group::Edit::Override(group::Override {
                    channel: id.to_owned(),
                    role: r.id.clone(),
                    allow: if private && !member { view } else { 0 },
                    deny: if private && member { view } else { 0 },
                })
            })
            .collect();
        node.edit(&g, &edits).await?;
        Ok(())
    }

    pub async fn delete_channel(&self, group: &str, id: &str) -> Result<()> {
        self.node()
            .await?
            .edit(
                &parse_key(group)?,
                &[group::Edit::ChannelDelete(id.to_owned())],
            )
            .await?;
        Ok(())
    }

    /// Compacts `group` (the owner, holding its key): a new snapshot at
    /// `seq + 1`. Returns the new `seq`.
    pub async fn compact(&self, group: &str) -> Result<u64> {
        Ok(self
            .node()
            .await?
            .compact(&parse_key(group)?, self.secrets.as_ref())
            .await?)
    }

    // --- DMs (M7) ----------------------------------------------------------

    /// Every DM, sent and received, oldest first.
    pub async fn dms(&self) -> Result<Vec<DmView>> {
        let node = self.node().await?;
        let me = node.person();
        Ok(node.dms().await?.iter().map(|d| dm_view(d, &me)).collect())
    }

    /// Sends a DM to `to`. Delivered now if they are online; else it waits
    /// with their inbox anchors, if they named any, or here until they are.
    pub async fn send_dm(&self, to: &str, text: &str) -> Result<DmView> {
        let text = text.trim_end();
        if text.trim().is_empty() {
            return Err(AppError::Refused("nothing to send".into()));
        }
        if text.chars().count() > hit_event::dm::MAX_DM_CHARS {
            return Err(AppError::Refused(format!(
                "longer than {} characters",
                hit_event::dm::MAX_DM_CHARS
            )));
        }
        let node = self.node().await?;
        let to = parse_key(to)?;
        if to == node.person() {
            return Err(AppError::Refused("that is you".into()));
        }
        let d = node.send_dm(&to, text).await?;
        Ok(dm_view(&d, &node.person()))
    }

    /// Sends a file to `to`, end to end (NIP-17 kind 15).
    pub async fn send_dm_file(&self, to: &str, path: &Path) -> Result<DmView> {
        let node = self.node().await?;
        let to = parse_key(to)?;
        if to == node.person() {
            return Err(AppError::Refused("that is you".into()));
        }
        let d = node.send_dm_file(&to, path).await?;
        Ok(dm_view(&d, &node.person()))
    }

    /// A DM file's bytes, decrypted on this side.
    pub async fn dm_file_bytes(&self, dm_id: &str) -> Result<Vec<u8>> {
        Ok(self.node().await?.dm_file_bytes(dm_id).await?)
    }

    /// Writes a DM file, decrypted, to `to` — where the person chose.
    pub async fn export_dm_file(&self, dm_id: &str, to: &Path) -> Result<()> {
        let bytes = self.dm_file_bytes(dm_id).await?;
        std::fs::write(to, bytes)?;
        Ok(())
    }

    /// The anchors this person's DMs may wait at, and which ones they named.
    pub async fn inbox(&self) -> Result<InboxView> {
        let node = self.node().await?;
        let me = node.person();
        let named: Vec<String> = node
            .inbox_of(&me)
            .await?
            .into_iter()
            .map(|(p, _)| npub(&p))
            .collect();
        let mut available = BTreeSet::new();
        for (g, state) in node.groups() {
            if state.is_some_and(|s| s.is_member(&me)) {
                available.extend(node.anchors(&g).await?.iter().map(npub));
            }
        }
        Ok(InboxView {
            enabled: !named.is_empty(),
            named,
            available: available.into_iter().collect(),
        })
    }

    /// Names every anchor of this person's groups as their inbox, or none.
    pub async fn set_inbox(&self, enabled: bool) -> Result<InboxView> {
        let node = self.node().await?;
        let anchors: Vec<PublicKey> = if enabled {
            self.inbox()
                .await?
                .available
                .iter()
                .filter_map(|a| parse_key(a).ok())
                .collect()
        } else {
            Vec::new()
        };
        if enabled && anchors.is_empty() {
            return Err(AppError::Refused(
                "none of your groups has an anchor to hold them".into(),
            ));
        }
        node.set_inbox(&anchors).await?;
        self.inbox().await
    }

    /// Display names for `people` (npubs), where this node holds one.
    pub async fn names(&self, people: &[String]) -> Result<BTreeMap<String, String>> {
        let node = self.node().await?;
        let keys: Vec<PublicKey> = people.iter().filter_map(|p| parse_key(p).ok()).collect();
        Ok(node
            .names(&keys)
            .await?
            .into_iter()
            .map(|(k, v)| (npub(&k), v))
            .collect())
    }

    /// The messages of `channel`, oldest first, each with its reactions.
    pub async fn messages(&self, group: &str, channel: &str) -> Result<Vec<MessageView>> {
        let node = self.node().await?;
        let me = node.person();
        let evs = node
            .channel_events(
                &parse_key(group)?,
                channel,
                &[group::CHAT_MESSAGE, group::REACTION],
            )
            .await?;
        let mut out: Vec<MessageView> = evs.iter().filter_map(|e| message_view(e, &me)).collect();
        let at: BTreeMap<String, usize> = out
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.clone(), i))
            .collect();
        for r in evs.iter().filter_map(|e| reaction_view(e, &me)) {
            // A reaction to a message not held here — withdrawn, or not
            // arrived yet — has nothing to hang on.
            if let Some(i) = at.get(&r.message) {
                out[*i].reactions.push(r);
            }
        }
        let ids: Vec<EventId> = evs
            .iter()
            .filter(|e| e.kind == group::CHAT_MESSAGE)
            .map(|e| e.id)
            .collect();
        for (id, t) in node.zap_totals(&ids).await? {
            if let Some(i) = at.get(&id.to_hex()) {
                out[*i].zap_sats = t.msat / 1000;
                out[*i].zap_count = t.count;
            }
        }
        Ok(out)
    }

    // --- files (M8) --------------------------------------------------------

    /// Adds a file from disk to this device's store, ready to send.
    pub async fn attach_file(&self, path: &Path) -> Result<FileView> {
        Ok(self.node().await?.attach(path).await?.into())
    }

    /// A file's bytes — fetched from whoever has it, if this device does
    /// not, and verified by hash either way.
    pub async fn file_bytes(&self, hash: &str) -> Result<Vec<u8>> {
        let node = self.node().await?;
        node.fetch_file(hash).await?;
        Ok(node.file_bytes(hash).await?)
    }

    /// Writes a file to `to` (fetching it first, if need be).
    pub async fn export_file(&self, hash: &str, to: &Path) -> Result<()> {
        let node = self.node().await?;
        node.fetch_file(hash).await?;
        Ok(node.save_file(hash, to).await?)
    }

    /// Reacts to `message` with `emoji`.
    pub async fn react(
        &self,
        group: &str,
        channel: &str,
        message: &str,
        emoji: &str,
    ) -> Result<ReactionView> {
        let emoji = emoji.trim();
        if emoji.is_empty() || emoji.chars().count() > group::MAX_REACTION {
            return Err(AppError::Refused("not a reaction".into()));
        }
        let node = self.node().await?;
        let ev = node
            .react(&parse_key(group)?, channel, &parse_id(message)?, emoji)
            .await?;
        reaction_view(&ev, &node.person()).ok_or_else(|| AppError::Io("not a reaction".into()))
    }

    /// Withdraws our own message or reaction `id`.
    pub async fn withdraw(&self, group: &str, channel: &str, id: &str) -> Result<()> {
        self.node()
            .await?
            .delete(&parse_key(group)?, channel, &[parse_id(id)?])
            .await?;
        Ok(())
    }

    /// Records that `channel` has been read up to `at` (seconds). Kept in
    /// `read.json` beside `app.json`, so unread counts survive a restart.
    pub async fn mark_read(&self, group: &str, channel: &str, at: u64) -> Result<()> {
        let _lock = self.read_lock.lock().await;
        let mut marks = self.read_marks()?;
        let key = format!("{group}/{channel}");
        if marks.get(&key).is_some_and(|m| *m >= at) {
            return Ok(());
        }
        marks.insert(key, at);
        let bytes = serde_json::to_vec(&marks).map_err(|e| AppError::Io(e.to_string()))?;
        let tmp = self.data_dir.join("read.json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, self.data_dir.join("read.json"))?;
        Ok(())
    }

    fn read_marks(&self) -> Result<BTreeMap<String, u64>> {
        match std::fs::read(self.data_dir.join("read.json")) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| AppError::Io(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// DMs from others newer than each conversation's read mark (`dm/<npub>`
    /// in `read.json`), per conversation.
    pub async fn dm_unread(&self) -> Result<BTreeMap<String, usize>> {
        let marks = self.read_marks()?;
        let mut out = BTreeMap::new();
        for d in self.dms().await? {
            let since = marks.get(&format!("dm/{}", d.with)).copied().unwrap_or(0);
            if !d.mine && d.at > since {
                *out.entry(d.with).or_insert(0) += 1;
            }
        }
        Ok(out)
    }

    /// Every channel this person can see with messages from others newer
    /// than its read mark. A channel never opened counts all of them.
    pub async fn unread(&self) -> Result<Vec<Unread>> {
        let node = self.node().await?;
        let me = node.person();
        let marks = self.read_marks()?;
        let mut out = Vec::new();
        for g in self.groups().await?.into_iter().filter(|g| g.ready) {
            let key = parse_key(&g.id)?;
            let all = node.messages(&key, None).await?;
            for c in &g.channels {
                let since = marks
                    .get(&format!("{}/{}", g.id, c.id))
                    .copied()
                    .unwrap_or(0);
                let count = all
                    .iter()
                    .filter(|e| {
                        e.pubkey != me
                            && e.created_at.as_secs() > since
                            && group::channel_of(e) == Some(c.id.as_str())
                    })
                    .count();
                if count > 0 {
                    out.push(Unread {
                        group: g.id.clone(),
                        channel: c.id.clone(),
                        count,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Posts `text`. Stored here first; delivered now if anyone is
    /// reachable, else by the next sync that reaches someone — the queue is
    /// the store.
    pub async fn send(
        &self,
        group: &str,
        channel: &str,
        text: &str,
        files: Vec<FileView>,
    ) -> Result<MessageView> {
        let text = text.trim_end();
        if text.trim().is_empty() && files.is_empty() {
            return Err(AppError::Refused("nothing to send".into()));
        }
        if files.len() > group::MAX_ATTACHMENTS {
            return Err(AppError::Refused(format!(
                "at most {} files in one message",
                group::MAX_ATTACHMENTS
            )));
        }
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(AppError::Refused(format!(
                "longer than {MAX_MESSAGE_CHARS} characters"
            )));
        }
        let node = self.node().await?;
        let files: Vec<group::Attachment> = files.into_iter().map(Into::into).collect();
        let ev = node
            .say_with(&parse_key(group)?, channel, text, &files)
            .await?;
        message_view(&ev, &node.person()).ok_or_else(|| AppError::Io("not a message".into()))
    }

    pub async fn create_group(&self, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Refused("a group needs a name".into()));
        }
        let g = self
            .node()
            .await?
            .create_group(name, self.secrets.as_ref())
            .await?;
        Ok(npub(&g))
    }

    /// A new invite link to `group`, valid for `hours` (or until revoked).
    pub async fn invite(&self, group: &str, hours: Option<u32>) -> Result<String> {
        let node = self.node().await?;
        let valid = hours.map(|h| Duration::from_secs(u64::from(h) * 3_600));
        let addr = match self.net.relays {
            Relays::Disabled => self.net.invite_addr,
            Relays::N0 | Relays::Custom(_) => None,
        };
        let link = node.invite(&parse_key(group)?, valid, addr).await?;
        Ok(link.to_string())
    }

    /// This person's contact link: someone with no group in common can
    /// write to them with it (and, once written to, call).
    pub async fn contact_link(&self) -> Result<String> {
        let node = self.node().await?;
        let addr = match self.net.relays {
            Relays::Disabled => self.net.invite_addr,
            Relays::N0 | Relays::Custom(_) => None,
        };
        Ok(node.contact_link(addr).to_string())
    }

    /// Opens someone's contact link — one of their devices must answer —
    /// and returns their npub, for the window to open the conversation.
    pub async fn add_contact(&self, link: &str) -> Result<String> {
        let link: hit_proto::contact::ContactLink = link
            .trim()
            .parse()
            .map_err(|e: hit_proto::contact::ContactError| AppError::Refused(e.to_string()))?;
        let person = self.node().await?.add_contact(&link).await?;
        Ok(npub(&person))
    }

    /// Joins from a link; returns the group's id.
    pub async fn join(&self, link: &str) -> Result<String> {
        let link: InviteLink = link
            .trim()
            .parse()
            .map_err(|e: hit_proto::invite::InviteError| AppError::Refused(e.to_string()))?;
        let node = self.node().await?;
        if node
            .group(&link.group)
            .is_some_and(|s| s.is_member(&node.person()))
        {
            return Err(AppError::Refused("you are already in this group".into()));
        }
        node.join(&link).await?;
        Ok(npub(&link.group))
    }

    /// The people with a device reached lately (npubs), this one excluded.
    /// The people here lately (npubs), this one excluded: their presence
    /// reached us over gossip, or a device of theirs completed a session.
    pub async fn present(&self) -> Result<BTreeSet<String>> {
        let node = self.node().await?;
        let me = node.person();
        Ok(node
            .present(PRESENT_WINDOW)
            .iter()
            .filter(|p| **p != me)
            .map(npub)
            .collect())
    }

    // --- Voice (M9) --------------------------------------------------------

    pub async fn voice(&self) -> Result<Arc<Voice>> {
        self.running
            .read()
            .await
            .as_ref()
            .map(|r| r.voice.clone())
            .ok_or_else(|| AppError::Refused("finish setting up first".into()))
    }

    /// The microphone and speakers chosen in the settings (or
    /// `HIT_AUDIO_IN` / `HIT_AUDIO_OUT`, which win — the by-hand recipe's
    /// null sink).
    async fn open_audio(&self) -> Result<(device::Mic, device::Speaker)> {
        let f = self.file().clone();
        let input = std::env::var("HIT_AUDIO_IN").ok().or(f.audio_input);
        let output = std::env::var("HIT_AUDIO_OUT").ok().or(f.audio_output);
        let echo = !f.no_echo_cancel;
        let (mic, speaker) = tokio::task::spawn_blocking(move || device::open(input, output, echo))
            .await
            .map_err(|e| AppError::Io(e.to_string()))?
            .map_err(|e| AppError::Refused(e.to_string()))?;
        *self.audio_watch.lock().unwrap_or_else(|e| e.into_inner()) = Some(mic.watch());
        Ok((mic, speaker))
    }

    /// Joins a voice channel — leaving the call this person is in, if any.
    pub async fn voice_join(&self, group: &str, channel: &str) -> Result<()> {
        let g = parse_key(group)?;
        let voice = self.voice().await?;
        if voice.state().is_some() {
            voice.leave().await?;
        }
        let (mic, speaker) = self.open_audio().await?;
        voice
            .join(g, channel, Box::new(mic), Box::new(speaker))
            .await?;
        Ok(())
    }

    /// Calls `who` (a 1:1 call from a DM) — leaving any call first.
    pub async fn voice_call(&self, who: &str) -> Result<()> {
        let p = parse_key(who)?;
        let voice = self.voice().await?;
        if voice.state().is_some() {
            voice.leave().await?;
        }
        let (mic, speaker) = self.open_audio().await?;
        voice.call(p, Box::new(mic), Box::new(speaker)).await?;
        Ok(())
    }

    /// Picks up the call ringing here — leaving any call first.
    pub async fn voice_answer(&self) -> Result<()> {
        let voice = self.voice().await?;
        if voice.incoming().is_none() {
            return Err(VoiceError::NotRinging.into());
        }
        if voice.state().is_some() {
            voice.leave().await?;
        }
        let (mic, speaker) = self.open_audio().await?;
        voice.answer(Box::new(mic), Box::new(speaker)).await?;
        Ok(())
    }

    pub async fn voice_decline(&self) -> Result<()> {
        self.voice().await?.decline()?;
        Ok(())
    }

    pub async fn voice_leave(&self) -> Result<()> {
        let voice = self.voice().await?;
        if voice.state().is_some() {
            voice.leave().await?;
        }
        Ok(())
    }

    /// Mute, deafen, push-to-talk: whichever is given.
    pub async fn voice_set(
        &self,
        muted: Option<bool>,
        deafened: Option<bool>,
        push_to_talk: Option<bool>,
    ) -> Result<()> {
        let voice = self.voice().await?;
        if let Some(m) = muted {
            voice.set_muted(m);
        }
        if let Some(d) = deafened {
            voice.set_deafened(d);
        }
        if let Some(p) = push_to_talk {
            voice.set_push_to_talk(p);
        }
        Ok(())
    }

    pub async fn voice_talk(&self, down: bool) -> Result<()> {
        self.voice().await?.talk(down);
        Ok(())
    }

    /// Every voice channel this person can see, with who is in it.
    pub async fn voice_view(&self) -> Result<VoiceView> {
        let node = self.node().await?;
        let voice = self.voice().await?;
        let me = node.person();
        let mut rooms = BTreeMap::new();
        for (g, state) in node.groups() {
            let Some(s) = state else { continue };
            if !s.is_member(&me) {
                continue;
            }
            let mut chans = BTreeMap::new();
            for c in s.channels() {
                if c.kind != group::ChannelKind::Voice
                    || !s.effective(&me, &c.id).contains(Permissions::VIEW_CHANNEL)
                {
                    continue;
                }
                let people: Vec<String> = voice.participants(&g, &c.id).iter().map(npub).collect();
                chans.insert(c.id.clone(), people);
            }
            if !chans.is_empty() {
                rooms.insert(npub(&g), chans);
            }
        }
        Ok(VoiceView {
            rooms,
            call: voice.state(),
            incoming: voice.incoming().map(|p| npub(&p)),
            ended: voice.ended(),
            audio: voice.state().and_then(|_| {
                self.audio_watch
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .map(device::Watch::stats)
            }),
        })
    }

    pub async fn audio_settings(&self) -> Result<AudioSettings> {
        let devices = tokio::task::spawn_blocking(device::devices)
            .await
            .map_err(|e| AppError::Io(e.to_string()))?;
        let f = self.file().clone();
        Ok(AudioSettings {
            devices,
            input: f.audio_input,
            output: f.audio_output,
            echo_cancel: !f.no_echo_cancel,
        })
    }

    /// Takes effect at the next join.
    pub fn set_audio(
        &self,
        input: Option<String>,
        output: Option<String>,
        echo_cancel: bool,
    ) -> Result<()> {
        let mut next = self.file().clone();
        next.audio_input = input;
        next.audio_output = output;
        next.no_echo_cancel = !echo_cancel;
        self.save_file(next)
    }
}

/// The wallet and Lightning address, as the settings show them (M10).
#[derive(Debug, Clone, Serialize)]
pub struct WalletView {
    /// A wallet connection (NWC) is in the keychain.
    pub connected: bool,
    /// Where it talks to the wallet — its relays; never the secret.
    pub relays: Vec<String>,
    /// This person's Lightning address, if their profile has one.
    pub address: Option<String>,
    /// Their profile also names the key its server signs receipts with:
    /// zaps to them can be counted.
    pub zappable: bool,
    pub fee_percent: u64,
    pub fee_min_sats: u64,
    /// Whether this build charges the fee at all (an address is set).
    pub fee_on: bool,
}

/// What a zap will cost, shown before sending (`DESIGN.md` §11).
#[derive(Debug, Clone, Serialize)]
pub struct ZapQuoteView {
    pub recipient: String,
    pub address: String,
    pub amount: u64,
    pub fee: u64,
    pub total: u64,
    /// Where the receipt goes: the group's anchors' relays.
    pub relays: Vec<String>,
    /// A wallet is connected; otherwise only the QR code.
    pub wallet: bool,
}

/// A zap paid from the connected wallet.
#[derive(Debug, Clone, Serialize)]
pub struct ZapSent {
    /// Its receipt reached this app and was carried into the group. When
    /// false the anchor carries it, or nobody does — the zap still went.
    pub counted: bool,
    pub fee: u64,
}

/// An invoice to pay by QR code.
#[derive(Debug, Clone, Serialize)]
pub struct InvoiceView {
    pub bolt11: String,
    /// An SVG of the `lightning:` URI.
    pub qr: String,
    pub sats: u64,
}

/// HIT_ENTER's fee address: the constant, or — in a debug build only — the
/// `HIT_ZAP_FEE_ADDRESS` a test against a fake network sets.
fn fee_address() -> Option<LightningAddress> {
    #[cfg(debug_assertions)]
    if let Ok(a) = std::env::var("HIT_ZAP_FEE_ADDRESS") {
        return a.parse().ok();
    }
    hit_zap::FEE_ADDRESS.and_then(|a| a.parse().ok())
}

fn qr_svg(bolt11: &str) -> Result<String> {
    let code = qrcode::QrCode::new(format!("lightning:{}", bolt11.to_uppercase()))
        .map_err(|e| AppError::Io(e.to_string()))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build())
}

/// The longest zap comment the dialog sends.
pub const MAX_ZAP_COMMENT: usize = hit_event::zap::MAX_COMMENT;

impl App {
    fn zapper(&self) -> Result<&Zapper> {
        if let Some(z) = self.zapper.get() {
            return Ok(z);
        }
        let z = Zapper::new(fee_address())?;
        Ok(self.zapper.get_or_init(|| z))
    }

    fn wallet_account(&self) -> Result<String> {
        let npub = self
            .file()
            .npub
            .clone()
            .ok_or_else(|| AppError::Refused("no identity yet".into()))?;
        Ok(format!("nwc:{npub}"))
    }

    /// The connected wallet, from the keychain.
    fn wallet(&self) -> Result<Option<Wallet>> {
        let Some(uri) = self
            .secrets
            .get(&self.wallet_account()?)
            .map_err(KeyError::from)?
        else {
            return Ok(None);
        };
        Ok(Wallet::parse(&uri).ok())
    }

    pub async fn wallet_view(&self) -> Result<WalletView> {
        let wallet = self.wallet()?;
        let node = self.node().await?;
        let lud = node.lud16_of(&node.person()).await?;
        Ok(WalletView {
            connected: wallet.is_some(),
            relays: wallet.map(|w| w.relays()).unwrap_or_default(),
            zappable: lud.as_ref().is_some_and(|(_, k)| k.is_some()),
            address: lud.map(|(a, _)| a),
            fee_percent: hit_zap::FEE_PERCENT,
            fee_min_sats: hit_zap::FEE_MIN_SATS,
            fee_on: fee_address().is_some(),
        })
    }

    /// Keeps a wallet connection string (NIP-47) in the keychain — it can
    /// spend up to its budget, so it is a secret like the nsec: never in
    /// `app.json`, never shown again.
    pub async fn connect_wallet(&self, connection: &str) -> Result<WalletView> {
        Wallet::parse(connection)?;
        self.secrets
            .put(&self.wallet_account()?, connection.trim())
            .map_err(KeyError::from)?;
        self.wallet_view().await
    }

    pub async fn disconnect_wallet(&self) -> Result<WalletView> {
        self.secrets
            .delete(&self.wallet_account()?)
            .map_err(KeyError::from)?;
        self.wallet_view().await
    }

    /// Sets this person's Lightning address — asking its server for the
    /// key it signs receipts with, which goes in the profile beside it —
    /// or removes it (empty).
    pub async fn set_lightning_address(&self, address: &str) -> Result<WalletView> {
        let node = self.node().await?;
        let address = address.trim();
        if address.is_empty() {
            node.set_lud16(None).await?;
        } else {
            let a: LightningAddress = address.parse()?;
            let key = self.zapper()?.provider_of(&a).await?;
            node.set_lud16(Some((&a.to_string(), &key))).await?;
        }
        self.wallet_view().await
    }

    /// Everything a zap of `message` needs, checked: its author's address
    /// and provider key, the group's receipt relays, and the quote.
    async fn prepare_zap(
        &self,
        group: &str,
        message: &str,
        amount: u64,
    ) -> Result<(Node, PublicKey, Quote, Vec<String>)> {
        let node = self.node().await?;
        let g = parse_key(group)?;
        let id = parse_id(message)?;
        let target = node
            .store()
            .by_ids(&[id])
            .await
            .map_err(NodeError::from)?
            .into_iter()
            .next()
            .ok_or(NodeError::NoSuchEvent)?;
        if target.pubkey == node.person() {
            return Err(AppError::Refused("that is your own message".into()));
        }
        let (address, key) = node
            .lud16_of(&target.pubkey)
            .await?
            .ok_or_else(|| AppError::Refused("they have not set a Lightning address".into()))?;
        let key = key.ok_or_else(|| {
            AppError::Refused(
                "their profile does not name their wallet's zap key, so no zap to them could be counted — they can set their address again in HIT_ENTER".into(),
            )
        })?;
        let relays = node.receipt_relays(&g).await?;
        if relays.is_empty() {
            return Err(AppError::Refused(
                "nobody would see this zap: the group has no anchor with a public wss:// relay to receive its receipt".into(),
            ));
        }
        let quote = self.zapper()?.quote(&address.parse()?, amount).await?;
        if quote.provider != key {
            return Err(AppError::Refused(
                "their Lightning address's server signs with a different key than their profile says, so the zap would show nowhere — they can set their address again".into(),
            ));
        }
        Ok((node, target.pubkey, quote, relays))
    }

    pub async fn zap_quote(&self, group: &str, message: &str, amount: u64) -> Result<ZapQuoteView> {
        let (_, recipient, quote, relays) = self.prepare_zap(group, message, amount).await?;
        Ok(ZapQuoteView {
            recipient: npub(&recipient),
            address: quote.params.address.to_string(),
            amount: quote.amount_sats,
            fee: quote.fee_sats,
            total: quote.total_sats(),
            relays,
            wallet: self.wallet()?.is_some(),
        })
    }

    /// The signed zap request — by a throwaway key when anonymous.
    fn zap_request(
        node: &Node,
        recipient: &PublicKey,
        message: &str,
        quote: &Quote,
        relays: &[String],
        comment: &str,
        anon: bool,
    ) -> Result<Event> {
        let b = hit_event::zap::request(
            recipient,
            &parse_id(message)?,
            quote.amount_sats * 1000,
            relays,
            &quote.params.lnurl,
            comment.trim(),
            anon,
        );
        Ok(if anon {
            b.finalize(&Keys::generate())
                .map_err(|e| AppError::Io(e.to_string()))?
        } else {
            node.sign_unjudged(b)?
        })
    }

    /// Zaps `message` from the connected wallet: the zap, then the fee
    /// (best-effort, after), then its receipt carried in if it reached us.
    pub async fn zap(
        &self,
        group: &str,
        message: &str,
        amount: u64,
        comment: &str,
        anon: bool,
    ) -> Result<ZapSent> {
        let wallet = self
            .wallet()?
            .ok_or_else(|| AppError::Refused("connect a wallet first, or pay by QR code".into()))?;
        let (node, recipient, quote, relays) = self.prepare_zap(group, message, amount).await?;
        let request =
            Self::zap_request(&node, &recipient, message, &quote, &relays, comment, anon)?;
        let zapped = self
            .zapper()?
            .send(&wallet, &quote, &request)
            .await
            .inspect_err(|e| tracing::warn!("zap not paid: {e}"))?;
        let counted = match &zapped.receipt {
            Some((receipt, _)) => match node.carry_zap(receipt).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("zap paid, but its receipt was refused: {e}");
                    false
                }
            },
            None => {
                tracing::warn!("zap paid, but no receipt reached this app on {relays:?}");
                false
            }
        };
        let fee = quote.fee_sats;
        tokio::spawn(async move {
            match zapped.fee.await {
                Ok(hit_zap::Fee::Failed(why)) => tracing::info!("zap fee not paid: {why}"),
                Ok(_) => {}
                Err(e) => tracing::warn!("zap fee task: {e}"),
            }
        });
        Ok(ZapSent { counted, fee })
    }

    /// The zap's invoice, for any wallet by QR code. [`Self::zap_wait`]
    /// then waits for its receipt; [`Self::zap_fee_invoice`] is the fee's.
    pub async fn zap_invoice(
        &self,
        group: &str,
        message: &str,
        amount: u64,
        comment: &str,
        anon: bool,
    ) -> Result<InvoiceView> {
        let (node, recipient, quote, relays) = self.prepare_zap(group, message, amount).await?;
        let request =
            Self::zap_request(&node, &recipient, message, &quote, &relays, comment, anon)?;
        let bolt11 = self.zapper()?.invoice(&quote, &request).await?;
        *self.pending_zap.lock().unwrap_or_else(|e| e.into_inner()) = Some(PendingZap {
            request,
            provider: quote.provider,
            fee_sats: quote.fee_sats,
        });
        Ok(InvoiceView {
            qr: qr_svg(&bolt11)?,
            bolt11,
            sats: quote.amount_sats,
        })
    }

    /// Waits up to two minutes for the QR-paid zap to be counted: its
    /// receipt on a relay this app can read (then carried in), or carried
    /// in by the group's anchor — whose relay serves nothing to an app that
    /// has not authenticated, so that road shows up only as the carrier.
    pub async fn zap_wait(&self) -> Result<bool> {
        const WAIT: Duration = Duration::from_secs(120);
        let (request, provider) = {
            let p = self.pending_zap.lock().unwrap_or_else(|e| e.into_inner());
            let p = p
                .as_ref()
                .ok_or_else(|| AppError::Refused("no zap waiting".into()))?;
            (p.request.clone(), p.provider)
        };
        let node = self.node().await?;
        let target = group::targets_of(&request)
            .first()
            .copied()
            .ok_or_else(|| AppError::Io("a zap request without a target".into()))?;
        let mut watch = {
            let request = request.clone();
            tokio::spawn(async move { hit_zap::zap::await_receipt(&request, provider, WAIT).await })
        };
        let deadline = tokio::time::Instant::now() + WAIT;
        let mut watching = true;
        loop {
            if node
                .zaps_of(&[target])
                .await?
                .iter()
                .any(|z| z.request == request.id)
            {
                watch.abort();
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                watch.abort();
                return Ok(false);
            }
            tokio::select! {
                got = &mut watch, if watching => {
                    watching = false;
                    if let Ok(Some((receipt, _))) = got {
                        return Ok(node.carry_zap(&receipt).await.is_ok());
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
        }
    }

    /// The fee's invoice after a QR-paid zap — the second QR, marked as the
    /// fee (`DESIGN.md` §11 point 6). `None` when no fee is owed.
    pub async fn zap_fee_invoice(&self) -> Result<Option<InvoiceView>> {
        let fee = self
            .pending_zap
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map_or(0, |p| p.fee_sats);
        if fee == 0 {
            return Ok(None);
        }
        let bolt11 = self.zapper()?.fee_invoice(fee).await?;
        Ok(Some(InvoiceView {
            qr: qr_svg(&bolt11)?,
            bolt11,
            sats: fee,
        }))
    }

    /// What each of `messages` was zapped, in sats, for those that were.
    pub async fn zap_totals(&self, messages: &[String]) -> Result<BTreeMap<String, (u64, usize)>> {
        let ids: Vec<EventId> = messages.iter().filter_map(|m| parse_id(m).ok()).collect();
        Ok(self
            .node()
            .await?
            .zap_totals(&ids)
            .await?
            .into_iter()
            .map(|(id, t)| (id.to_hex(), (t.msat / 1000, t.count)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hit_event::MemoryStore;

    fn net() -> Net {
        Net {
            relays: Relays::Disabled,
            port: None,
            invite_addr: None,
            peers: Vec::new(),
        }
    }

    async fn open(dir: &Path, keys: &Arc<MemoryStore>) -> App {
        App::open(dir, keys.clone(), net()).await.unwrap()
    }

    #[tokio::test]
    async fn a_new_identity_cannot_reach_a_group_until_it_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryStore::default());
        let app = open(dir.path(), &keys).await;
        assert_eq!(app.stage(), Stage::New);
        assert!(app.reveal_key().is_err(), "no key to reveal yet");

        let npub = app.create_identity().unwrap();
        assert_eq!(app.stage(), Stage::Backup);
        assert!(
            app.create_group("x").await.is_err(),
            "no group before backup"
        );
        assert!(app.create_identity().is_err(), "one identity per device");

        // Quitting half way resumes at the backup step, key and all.
        drop(app);
        let app = open(dir.path(), &keys).await;
        assert_eq!(app.stage(), Stage::Backup);
        let key = app.reveal_key().unwrap();
        assert_eq!(Person::from_nsec(&key).unwrap().npub(), npub);

        // A wrong tail is refused; the right one finishes the step.
        assert!(app.confirm_backup("nope").await.is_err());
        let tail = &key[key.len() - CONFIRM_CHARS..];
        app.confirm_backup(tail).await.unwrap();
        assert_eq!(app.stage(), Stage::Ready);
        assert!(app.reveal_key().is_err(), "never shown after onboarding");

        app.set_name("Alice").await.unwrap();
        let g = app.create_group("Friends").await.unwrap();
        assert_eq!(app.groups().await.unwrap()[0].name, "Friends");
        assert_eq!(app.status().await.unwrap().name.as_deref(), Some("Alice"));
        let members = app.members(&g).await.unwrap();
        assert!(members[0].owner && members[0].me);
        assert_eq!(members[0].name.as_deref(), Some("Alice"));
        app.shutdown().await;
    }

    #[tokio::test]
    async fn a_backup_file_round_trips_and_an_import_skips_the_backup_step() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let app = open(d1.path(), &keys).await;
        let npub = app.create_identity().unwrap();
        let file = d1.path().join("me.ncryptsec");
        assert!(app.backup_to_file(&file, "short").await.is_err());
        app.backup_to_file(&file, "correct horse").await.unwrap();
        assert_eq!(app.stage(), Stage::Ready);
        app.shutdown().await;

        // Another device imports it: no backup step, the same person.
        let other = open(d2.path(), &Arc::new(MemoryStore::default())).await;
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("ncryptsec1"));
        assert!(other.import_identity(&text, Some("wrong")).await.is_err());
        assert_eq!(
            other
                .import_identity(&text, Some("correct horse"))
                .await
                .unwrap(),
            npub
        );
        assert_eq!(other.stage(), Stage::Ready);
        other.shutdown().await;
    }

    #[tokio::test]
    async fn two_apps_talk_and_a_message_written_offline_is_delivered_by_itself() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let a = open(d1.path(), &keys).await;
        a.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let b = open(d2.path(), &keys).await;
        b.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let (na, nb) = (a.node().await.unwrap(), b.node().await.unwrap());
        let a_addr = local(&na);
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));

        let g = a.create_group("Two").await.unwrap();
        let link = na
            .invite(&parse_key(&g).unwrap(), None, Some(a_addr))
            .await
            .unwrap();
        assert_eq!(b.join(&link.to_string()).await.unwrap(), g);
        assert!(b.join(&link.to_string()).await.is_err(), "already in it");
        a.send(&g, "general", "hello b", Vec::new()).await.unwrap();
        assert_eq!(count_within(&b, &g, 1).await, 1, "B never got A's message");

        // A goes away. B writes; it is stored at once, nobody to send to.
        a.shutdown().await;
        let sent = b
            .send(&g, "general", "written while a was gone", Vec::new())
            .await;
        assert!(sent.unwrap().mine);
        assert_eq!(b.messages(&g, "general").await.unwrap().len(), 2);

        // A comes back; the sync loops deliver it with nobody pressing a
        // button.
        let a = open(d1.path(), &keys).await;
        let na = a.node().await.unwrap();
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));
        assert_eq!(count_within(&a, &g, 2).await, 2);
        let got = a.messages(&g, "general").await.unwrap();
        // Both within one second here, so in either order: `created_at` is
        // whole seconds.
        let mut texts: Vec<_> = got.iter().map(|m| (m.text.as_str(), m.mine)).collect();
        texts.sort();
        assert_eq!(
            texts,
            [("hello b", true), ("written while a was gone", false)]
        );
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn reactions_withdrawals_and_unread_counts_that_survive_a_restart() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let a = open(d1.path(), &keys).await;
        a.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let b = open(d2.path(), &keys).await;
        b.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let (na, nb) = (a.node().await.unwrap(), b.node().await.unwrap());
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));
        let g = a.create_group("Three").await.unwrap();
        let link = na
            .invite(&parse_key(&g).unwrap(), None, Some(local(&na)))
            .await
            .unwrap();
        b.join(&link.to_string()).await.unwrap();

        // Two from B; A has read nothing yet.
        b.send(&g, "general", "one", Vec::new()).await.unwrap();
        let two = b.send(&g, "general", "two", Vec::new()).await.unwrap();
        assert_eq!(count_within(&a, &g, 2).await, 2);
        let unread = a.unread().await.unwrap();
        assert_eq!((unread.len(), unread[0].count), (1, 2));

        // A reacts; it hangs on the message, at A and — after a sync — at B.
        let r = a.react(&g, "general", &two.id, "🎉").await.unwrap();
        assert!(a.react(&g, "general", &two.id, "").await.is_err());
        // By id: both were said in one second, so in either order.
        let find = |list: Vec<MessageView>| list.into_iter().find(|m| m.id == two.id).unwrap();
        let at_a = find(a.messages(&g, "general").await.unwrap());
        assert_eq!(at_a.reactions.len(), 1);
        assert!(at_a.reactions[0].mine);
        nb.sync_with(local_addr(&na)).await.unwrap();
        let at_b = find(b.messages(&g, "general").await.unwrap());
        assert_eq!(at_b.reactions[0].emoji, "🎉");
        assert!(!at_b.reactions[0].mine);

        // A cannot withdraw B's message, only its own reaction.
        assert!(a.withdraw(&g, "general", &two.id).await.is_err());
        a.withdraw(&g, "general", &r.id).await.unwrap();
        assert!(
            find(a.messages(&g, "general").await.unwrap())
                .reactions
                .is_empty()
        );

        // Read up to "two": nothing unread, and still nothing after a restart.
        a.mark_read(&g, "general", two.at).await.unwrap();
        assert!(a.unread().await.unwrap().is_empty());
        a.shutdown().await;
        let a = open(d1.path(), &keys).await;
        assert!(a.unread().await.unwrap().is_empty());
        // An older mark never moves it back.
        a.mark_read(&g, "general", 1).await.unwrap();
        assert!(a.unread().await.unwrap().is_empty());
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn the_owner_moderates_and_the_window_is_told_what_each_may_do() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let a = open(d1.path(), &keys).await;
        a.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let b = open(d2.path(), &keys).await;
        b.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let (na, nb) = (a.node().await.unwrap(), b.node().await.unwrap());
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));
        let g = a.create_group("Mods").await.unwrap();
        let link = na
            .invite(&parse_key(&g).unwrap(), None, Some(local(&na)))
            .await
            .unwrap();
        b.join(&link.to_string()).await.unwrap();
        nb.sync_with(local_addr(&na)).await.unwrap();
        let bob = b.status().await.unwrap().npub.unwrap();

        // The owner may do everything to Bob; Bob, to the owner, nothing.
        let at_a = a.members(&g).await.unwrap();
        let row = at_a.iter().find(|m| m.npub == bob).unwrap();
        assert!(row.can_kick && row.can_ban && row.can_mute && row.can_set_roles);
        let at_b = b.members(&g).await.unwrap();
        let owner = at_b.iter().find(|m| m.owner).unwrap();
        assert!(!owner.can_kick && !owner.can_ban && !owner.can_mute);
        let ga = &a.groups().await.unwrap()[0];
        assert!(ga.me_owner && ga.can_rename && ga.can_compact && ga.can_ban);
        let gb = &b.groups().await.unwrap()[0];
        assert!(!gb.can_rename && !gb.can_compact && !gb.can_manage_channels);

        // Roles: the owner can hand out every one; Bob, none.
        assert!(a.roles(&g).await.unwrap().iter().all(|r| r.assignable));
        assert!(b.roles(&g).await.unwrap().iter().all(|r| !r.assignable));
        a.set_roles(&g, &bob, &["mod".into()]).await.unwrap();
        let row = a
            .members(&g)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.npub == bob)
            .unwrap();
        assert_eq!(row.role_ids, ["mod"]);

        // Channels: an id from the name, a second one numbered; slowmode.
        let id = a
            .save_channel(&g, None, "#Off Topic!", 0, false)
            .await
            .unwrap();
        assert_eq!(id, "off-topic");
        assert_eq!(
            a.save_channel(&g, None, "off topic", 0, false)
                .await
                .unwrap(),
            "off-topic-2"
        );
        a.save_channel(&g, Some("general"), "general", 30, false)
            .await
            .unwrap();
        let chans = a.groups().await.unwrap()[0].channels.clone();
        assert_eq!(
            chans.iter().find(|c| c.id == "general").unwrap().slowmode,
            30
        );
        a.delete_channel(&g, "off-topic-2").await.unwrap();
        assert!(b.save_channel(&g, None, "mine", 0, false).await.is_err());

        // A voice channel, made private: moderators and up only. Bob is a
        // moderator, so he still sees it — marked private — and may not
        // change it; public again, the mark goes.
        let lounge = a.save_channel(&g, None, "lounge", 0, true).await.unwrap();
        a.set_channel_private(&g, &lounge, true).await.unwrap();
        let view = |gs: Vec<GroupView>| gs[0].channels.iter().find(|c| c.id == lounge).cloned();
        let at_a = view(a.groups().await.unwrap()).unwrap();
        assert_eq!((at_a.kind, at_a.private), ("voice", true));
        nb.sync_with(local_addr(&na)).await.unwrap();
        assert!(view(b.groups().await.unwrap()).unwrap().private);
        assert!(b.set_channel_private(&g, &lounge, false).await.is_err());
        a.set_channel_private(&g, &lounge, false).await.unwrap();
        assert!(!view(a.groups().await.unwrap()).unwrap().private);

        // Mute, then ban; the ban list says who; unban.
        a.mute(&g, &bob, Some(10)).await.unwrap();
        let row = a
            .members(&g)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.npub == bob)
            .unwrap();
        assert!(row.muted_until.is_some());
        a.ban(&g, &bob).await.unwrap();
        assert_eq!(a.bans(&g).await.unwrap()[0].npub, bob);
        assert!(!a.members(&g).await.unwrap().iter().any(|m| m.npub == bob));
        a.unban(&g, &bob).await.unwrap();
        assert!(a.bans(&g).await.unwrap().is_empty());

        // The owner renames and compacts.
        a.rename_group(&g, "Renamed").await.unwrap();
        assert_eq!(a.compact(&g).await.unwrap(), 1);
        let ga = &a.groups().await.unwrap()[0];
        assert_eq!((ga.name.as_str(), ga.seq), ("Renamed", 1));
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn a_dm_between_two_apps_with_unread_counts_and_no_inbox_without_an_anchor() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let a = open(d1.path(), &keys).await;
        a.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let b = open(d2.path(), &keys).await;
        b.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let (na, nb) = (a.node().await.unwrap(), b.node().await.unwrap());
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));
        let g = a.create_group("Pair").await.unwrap();
        let link = na
            .invite(&parse_key(&g).unwrap(), None, Some(local(&na)))
            .await
            .unwrap();
        b.join(&link.to_string()).await.unwrap();
        na.sync_with(local_addr(&nb)).await.unwrap();
        let bob = b.status().await.unwrap().npub.unwrap();
        let alice = a.status().await.unwrap().npub.unwrap();

        assert!(a.send_dm(&alice, "to myself").await.is_err());
        assert!(a.send_dm(&bob, "   ").await.is_err());
        let sent = a.send_dm(&bob, "psst").await.unwrap();
        assert!(sent.mine && sent.with == bob);
        let mut got = Vec::new();
        for _ in 0..100 {
            got = b.dms().await.unwrap();
            if !got.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(got.len(), 1);
        assert!(!got[0].mine && got[0].with == alice && got[0].text == "psst");
        assert_eq!(b.dm_unread().await.unwrap().get(&alice), Some(&1));
        b.mark_read("dm", &alice, got[0].at).await.unwrap();
        assert!(b.dm_unread().await.unwrap().is_empty());

        // No anchor in any group: nothing to name as an inbox.
        let inbox = b.inbox().await.unwrap();
        assert!(!inbox.enabled && inbox.available.is_empty());
        assert!(b.set_inbox(true).await.is_err());
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn a_file_sent_from_one_app_is_shown_and_saved_by_another() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let a = open(d1.path(), &keys).await;
        a.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let b = open(d2.path(), &keys).await;
        b.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let (na, nb) = (a.node().await.unwrap(), b.node().await.unwrap());
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));
        let g = a.create_group("Pics").await.unwrap();
        let link = na
            .invite(&parse_key(&g).unwrap(), None, Some(local(&na)))
            .await
            .unwrap();
        b.join(&link.to_string()).await.unwrap();

        let path = d1.path().join("cat.png");
        std::fs::write(&path, b"\x89PNG not really, but bytes all the same").unwrap();
        let f = a.attach_file(&path).await.unwrap();
        assert_eq!((f.mime.as_str(), f.name.as_str()), ("image/png", "cat.png"));
        // A file alone is a message; nothing at all is not.
        assert!(a.send(&g, "general", "", Vec::new()).await.is_err());
        let m = a.send(&g, "general", "", vec![f.clone()]).await.unwrap();
        assert_eq!(m.files.len(), 1);

        nb.sync_with(local_addr(&na)).await.unwrap();
        let got = b.messages(&g, "general").await.unwrap();
        assert_eq!(got[0].files[0].hash, f.hash);
        assert_eq!(
            b.file_bytes(&f.hash).await.unwrap(),
            std::fs::read(&path).unwrap()
        );
        let out = d2.path().join("saved.png");
        b.export_file(&f.hash, &out).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&path).unwrap());
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn a_dm_file_crosses_end_to_end_and_the_window_never_sees_its_key() {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let keys = Arc::new(MemoryStore::default());
        let a = open(d1.path(), &keys).await;
        a.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let b = open(d2.path(), &keys).await;
        b.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
            .await
            .unwrap();
        let (na, nb) = (a.node().await.unwrap(), b.node().await.unwrap());
        na.add_peer(local_addr(&nb));
        nb.add_peer(local_addr(&na));
        let g = a.create_group("Pair").await.unwrap();
        let link = na
            .invite(&parse_key(&g).unwrap(), None, Some(local(&na)))
            .await
            .unwrap();
        b.join(&link.to_string()).await.unwrap();
        na.sync_with(local_addr(&nb)).await.unwrap();
        let bob = b.status().await.unwrap().npub.unwrap();

        let path = d1.path().join("notes.txt");
        std::fs::write(&path, b"between the two of us").unwrap();
        let sent = a.send_dm_file(&bob, &path).await.unwrap();
        let f = sent.file.clone().unwrap();
        assert_eq!(
            (f.name.as_str(), f.mime.as_str()),
            ("notes.txt", "text/plain")
        );
        // What the window is given has no key in it.
        let json = serde_json::to_string(&sent).unwrap();
        assert!(!json.contains("key") && !json.contains("nonce"));

        let mut got = Vec::new();
        for _ in 0..100 {
            got = b.dms().await.unwrap();
            if !got.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            got[0].file.as_ref().map(|f| f.name.as_str()),
            Some("notes.txt")
        );
        assert_eq!(
            b.dm_file_bytes(&got[0].id).await.unwrap(),
            b"between the two of us"
        );
        let out = d2.path().join("saved.txt");
        b.export_dm_file(&got[0].id, &out).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"between the two of us");
        a.shutdown().await;
        b.shutdown().await;
    }

    /// The sats `app` shows on message `id`.
    async fn zapped(app: &App, g: &str, id: &str) -> u64 {
        app.messages(g, "general")
            .await
            .unwrap()
            .into_iter()
            .find(|x| x.id == id)
            .map_or(0, |x| x.zap_sats)
    }

    /// Polls `check` for up to ten seconds.
    async fn within<F, Fut>(mut check: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..100 {
            if check().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_zap_from_the_wallet_and_one_by_qr_code_with_the_fee_on_top() {
        use hit_zap::fake::FakeLightning;
        let net = FakeLightning::start().await;
        net.account("alice", 10_000);
        let bob_addr = net.account("bob", 0);
        let fee: LightningAddress = net.account("hitenter", 0).parse().unwrap();
        let nwc = net.connect_wallet("alice");

        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let keys = Arc::new(MemoryStore::default());
        let mut apps = Vec::new();
        for d in &dirs {
            let app = open(d.path(), &keys).await;
            app.import_identity(&Person::generate().reveal_nsec_for_backup(), None)
                .await
                .unwrap();
            let zapper = Zapper::new(Some(fee.clone()))
                .unwrap()
                .with_timeouts(Duration::from_millis(100), Duration::from_secs(3));
            assert!(app.zapper.set(zapper).is_ok());
            apps.push(app);
        }
        let (a, b, c) = (&apps[0], &apps[1], &apps[2]);
        let nodes = [
            a.node().await.unwrap(),
            b.node().await.unwrap(),
            c.node().await.unwrap(),
        ];
        for x in &nodes {
            for y in &nodes {
                if x.id() != y.id() {
                    x.add_peer(local_addr(y));
                }
            }
        }
        let g = a.create_group("Tips").await.unwrap();
        let link = nodes[0]
            .invite(&parse_key(&g).unwrap(), None, Some(local(&nodes[0])))
            .await
            .unwrap();
        b.join(&link.to_string()).await.unwrap();
        c.join(&link.to_string()).await.unwrap();
        // Carol's node is the group's anchor; its public relay is the fake
        // network's.
        nodes[2].set_anchor_profile("anchor").await.unwrap();
        nodes[2]
            .set_anchor_relay(Some(&net.relay_url()))
            .await
            .unwrap();

        // Bob sets his address: the app asks its server for its zap key.
        let w = b.set_lightning_address(&bob_addr).await.unwrap();
        assert!(w.zappable && w.address.as_deref() == Some(bob_addr.as_str()));
        assert!(b.set_lightning_address("not an address").await.is_err());
        let m = b.send(&g, "general", "tip jar", Vec::new()).await.unwrap();
        let mine = a.send(&g, "general", "my own", Vec::new()).await.unwrap();

        // Alice's app has Bob's profile, the anchor's, and the message.
        assert!(
            within(|| async {
                for x in &nodes[1..] {
                    let _ = nodes[0].sync_with(local_addr(x)).await;
                }
                a.zap_quote(&g, &m.id, 1_000).await.is_ok()
            })
            .await,
            "{:?}",
            a.zap_quote(&g, &m.id, 1_000).await.err()
        );
        let q = a.zap_quote(&g, &m.id, 1_000).await.unwrap();
        assert_eq!((q.amount, q.fee, q.total), (1_000, 10, 1_010));
        assert_eq!(q.relays, vec![net.relay_url()]);
        assert!(!q.wallet);
        assert!(
            a.zap_quote(&g, &mine.id, 1_000).await.is_err(),
            "your own message"
        );
        assert!(
            a.zap(&g, &m.id, 1_000, "", false).await.is_err(),
            "no wallet yet"
        );

        // The wallet goes in the keychain, not app.json.
        let w = a.connect_wallet(&nwc).await.unwrap();
        assert!(w.connected && w.relays == vec![net.relay_url()]);
        assert!(
            a.connect_wallet("nostr+walletconnect://nope")
                .await
                .is_err()
        );
        let file = std::fs::read_to_string(dirs[0].path().join("app.json")).unwrap();
        assert!(!file.contains("walletconnect") && !file.contains("secret"));

        // --- From the wallet. -------------------------------------------
        let sent = a.zap(&g, &m.id, 1_000, "thanks", false).await.unwrap();
        assert!(sent.counted, "the receipt came back and was carried in");
        assert_eq!(sent.fee, 10);
        assert!(within(|| async { net.balance_sats("hitenter") == 10 }).await);
        assert_eq!(net.balance_sats("bob"), 1_000);
        assert_eq!(zapped(a, &g, &m.id).await, 1_000);
        assert!(
            within(|| async {
                zapped(b, &g, &m.id).await == 1_000 && zapped(c, &g, &m.id).await == 1_000
            })
            .await
        );

        // --- By QR code: any wallet pays the invoice, then the fee's. ---
        let inv = a.zap_invoice(&g, &m.id, 200, "", true).await.unwrap();
        assert_eq!(inv.sats, 200);
        assert!(inv.qr.starts_with("<?xml") || inv.qr.contains("<svg"));
        let other_wallet = Wallet::parse(&nwc).unwrap();
        other_wallet.pay(&inv.bolt11).await.unwrap();
        assert!(a.zap_wait().await.unwrap(), "the QR zap's receipt");
        let fee_inv = a.zap_fee_invoice().await.unwrap().expect("2 sats owed");
        assert_eq!(fee_inv.sats, 2);
        other_wallet.pay(&fee_inv.bolt11).await.unwrap();
        assert_eq!(net.balance_sats("hitenter"), 12);
        assert!(within(|| async { zapped(b, &g, &m.id).await == 1_200 }).await);
        let totals = a.zap_totals(std::slice::from_ref(&m.id)).await.unwrap();
        assert_eq!(totals[&m.id], (1_200, 2));

        let w = a.disconnect_wallet().await.unwrap();
        assert!(!w.connected);
        for app in &apps {
            app.shutdown().await;
        }
    }

    /// How many messages `app` holds in `general`, once it holds `want` or
    /// ten seconds pass.
    async fn count_within(app: &App, g: &str, want: usize) -> usize {
        let mut n = 0;
        for _ in 0..100 {
            n = app.messages(g, "general").await.unwrap().len();
            if n >= want {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        n
    }

    fn local(node: &Node) -> SocketAddr {
        let port = node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .unwrap()
            .port();
        ([127, 0, 0, 1], port).into()
    }

    fn local_addr(node: &Node) -> iroh::EndpointAddr {
        iroh::EndpointAddr::from_parts(node.id(), [iroh::TransportAddr::Ip(local(node))])
    }
}
