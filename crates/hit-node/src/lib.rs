//! One node, many roles (M2–M3, `docs/PLAN.md`; `docs/DESIGN.md` §4–6, §9).
//!
//! A [`Node`] is an iroh endpoint serving `hit/nostr/1` as a Nostr relay,
//! *and* a client that syncs with other nodes over the same protocol — the
//! same code both ways — *and* a member of each of its groups' gossip. The
//! app, an anchor and the CLI are this node with different policies; **no
//! anchor-only path for something an app also does.**
//!
//! The rules that bind this crate:
//!
//! - Read the peer's `EndpointId` from the iroh connection, never from a
//!   message body. A session starts with each side's binding
//!   (`hit_proto::hello`), checked against the connection; a gossip
//!   connection is mapped to a person through the binding book.
//! - Verify every event before storing, forwarding or showing it: signature
//!   ([`hit_event::Verified`]), then `hit-group`. Every path an event takes
//!   into the store — a peer's `EVENT`, a fetch, gossip, our own — goes
//!   through `Inner::accept_event`.
//! - **A peer serves only what the asker may see**, judged per event by
//!   `hit_group::Groups::may_read`: for a `REQ`, for a negentropy set, and
//!   for what goes on gossip.
//! - iroh owns the transport: no HTTP or WebSocket in the chat plane.
//! - The mirror is written before the UI is told: [`Node::subscribe`] sees
//!   an event only after the store has it.
//! - Each group lives on exactly one endpoint, with all of its ALPNs (S5).

mod accept;
mod blobs;
mod device;
mod gossip;
mod relay;
mod sync;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use hit_event::{
    BindingBook, BindingError, DEVICE_BINDING, DeviceKey, GroupKey, KeyError, Person, SecretStore,
    Verified, dm,
    group::{self, Channel, ChannelKind, DEFAULT_ROLE, RoleDef, STATE_KINDS, Snapshot, group_of},
    is_revoked, needs_renewal, sign_binding, verify_binding_at,
};
use hit_group::{FOLD_KINDS, GroupState, Groups, Judgement, Permissions, Refusal};
use hit_proto::{
    ALPN, contact::ContactLink, frame::FrameError, hello::HelloError, invite::InviteLink,
};
use hit_store::{Store, StoreError, Stored};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, TransportAddr,
    address_lookup::memory::MemoryLookup, endpoint::presets,
};
use nostr::prelude::{Event, EventBuilder, Filter, PublicKey, SingleLetterTag, Timestamp};
use tokio::{
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
};

pub use blobs::FilePolicy;
pub use sync::SyncStats;

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("device key file: {0}")]
    DeviceKey(String),
    #[error("could not bind the endpoint: {0}")]
    Bind(String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("this device has been revoked; bind it again deliberately before using it")]
    DeviceRevoked,
    #[error("the peer did not open with a binding: {0}")]
    Hello(#[from] HelloError),
    #[error("the peer's binding was refused: {0}")]
    PeerBinding(#[from] BindingError),
    #[error("refused: {0}")]
    Refused(#[from] Refusal),
    #[error("dialled {dialled} but reached {reached}")]
    WrongPeer {
        dialled: EndpointId,
        reached: EndpointId,
    },
    #[error("the peer refused us: {0}")]
    RefusedByPeer(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("no such group here")]
    NoSuchGroup,
    #[error("the keychain holds no key for this group")]
    NoGroupKey,
    #[error("gossip: {0}")]
    Gossip(String),
    #[error("no bootstrap peer accepted the join: {0}")]
    JoinFailed(String),
    #[error("no such event here")]
    NoSuchEvent,
    #[error("only your own messages and reactions can be withdrawn")]
    NotYours,
    #[error("file: {0}")]
    Blob(String),
    #[error("nobody who has this file could be reached")]
    NoProvider,
    #[error("not a zap: {0}")]
    Zap(#[from] hit_event::zap::ZapError),
    #[error("the receipt is not signed by the recipient's wallet provider")]
    NotTheirProvider,
}

/// What an event was zapped, counting only receipts its author's provider
/// signed (M10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZapTotal {
    pub msat: u64,
    pub count: usize,
    /// Who zapped, by name, without repeats; anonymous zaps are only
    /// counted.
    pub zappers: Vec<PublicKey>,
}

/// Which relays the endpoint uses.
#[derive(Debug, Clone)]
pub enum Relays {
    /// n0's public relays and discovery (iroh's `N0` preset), which publishes
    /// only the relay URL, never an IP (`DESIGN.md` §8).
    N0,
    /// None: direct addresses only. Tests and a LAN.
    Disabled,
    /// Our own relay (an `iroh-relay` beside an anchor, M6), with n0's
    /// discovery still finding devices by id.
    Custom(iroh::RelayUrl),
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Holds `device.key` and `node.db`.
    pub data_dir: PathBuf,
    pub relays: Relays,
    /// UDP port to bind; `None` for any. A fixed port is how two processes on
    /// one machine find each other with relays off.
    pub port: Option<u16>,
}

/// Where an event came from — decides whether we pass it on over gossip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Signed here.
    Local,
    /// A peer's `EVENT` on `hit/nostr/1`: it may be the only node that has
    /// it (a joiner, say), so we gossip it on.
    Push,
    /// Fetched during a sync: the peer had it, so its group has it.
    Fetch,
    /// Off gossip, which forwards it itself.
    Gossip,
}

/// A running node. Cheap to clone.
#[derive(Clone)]
pub struct Node {
    inner: Arc<Inner>,
    stop: watch::Sender<bool>,
}

pub(crate) struct Inner {
    person: Person,
    device: DeviceKey,
    endpoint: Endpoint,
    lookup: MemoryLookup,
    store: Store,
    book: Mutex<BindingBook>,
    groups: RwLock<Groups>,
    own_binding: Mutex<Event>,
    events: broadcast::Sender<Event>,
    /// The files (M8).
    blobs: Arc<blobs::Blobs>,
    /// Ids of events this node dropped — withdrawn, removed by a moderator,
    /// or silenced — after the window may have shown them.
    removed: broadcast::Sender<Vec<nostr::prelude::EventId>>,
    gossip: gossip::Gossips,
    intake: mpsc::Sender<Event>,
    /// Devices to sync with that no binding has told us about yet: invite
    /// bootstrap peers, and the CLI's `peer`.
    hints: Mutex<HashSet<EndpointId>>,
    /// Devices that completed a hello with us since the sync loop last
    /// looked: they are evidently up, so their backoff is reset.
    heard: Mutex<HashSet<EndpointId>>,
    /// When each device last completed a session with us, either way: what
    /// the app shows as "reachable".
    reached: Mutex<HashMap<EndpointId, tokio::time::Instant>>,
    /// The devices of the inbox anchors named by people we share a group
    /// with (M7): synced with, so a DM sent while they were away reaches
    /// one. Refreshed from the store by the sync loop.
    inbox_devices: Mutex<BTreeSet<EndpointId>>,
    /// Who is in which voice channel (M9): `(group, channel)` → person →
    /// when their last "in" arrived. An "out" removes them.
    voice: Mutex<HashMap<(PublicKey, String), HashMap<PublicKey, tokio::time::Instant>>>,
    /// Protocols other crates serve here, by ALPN (M9).
    protocols: RwLock<HashMap<Vec<u8>, ProtocolFn>>,
    /// When each person's last presence reached us, off gossip.
    present: Mutex<HashMap<PublicKey, tokio::time::Instant>>,
    /// How many state-changing events of each group the last fold read —
    /// so a fold can be redone when another process (the CLI, signing into
    /// the same store while `run` serves it) added some.
    folded: Mutex<HashMap<PublicKey, usize>>,
    /// One group-changing event at a time: an action is judged by folding
    /// it with what is stored, and two judged against the same old state
    /// could each pass where together they would not.
    ///
    /// A read/write lock: an event that changes a group's state takes it
    /// for writing across judge, fold, insert, refold and purge; every other
    /// group event takes it for reading across judge and insert. Otherwise a
    /// message judged by the state before a ban could be stored *after* the
    /// ban's purge, and stay (M5's done-when test caught exactly that).
    fold_lock: tokio::sync::RwLock<()>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// What [`Inner::accept_event`] decided: `(accepted, NIP-01 OK message)`.
pub(crate) type Verdict = (bool, String);

/// A brand-new group's roles: HIT_ENTER_2's ladder.
/// How long handing a restricted room's voice presence to one device may
/// take: a heartbeat later there is a fresh one.
const VOICE_HAND_TIMEOUT: Duration = Duration::from_secs(4);

fn default_roles() -> Vec<RoleDef> {
    vec![
        RoleDef {
            id: DEFAULT_ROLE.into(),
            name: "Member".into(),
            perms: Permissions::DEFAULT_MEMBER.bits(),
            position: 0,
        },
        RoleDef {
            id: "mod".into(),
            name: "Moderator".into(),
            perms: Permissions::DEFAULT_MODERATOR.bits(),
            position: 10,
        },
        RoleDef {
            id: "admin".into(),
            name: "Admin".into(),
            perms: Permissions::DEFAULT_ADMIN.bits(),
            position: 20,
        },
    ]
}

impl Node {
    /// Starts a node for `person`, with this machine's device key (created on
    /// first start) and the store in `config.data_dir`.
    pub async fn start(person: Person, config: Config) -> Result<Self, NodeError> {
        std::fs::create_dir_all(&config.data_dir)
            .map_err(|e| NodeError::DeviceKey(e.to_string()))?;
        let device = device::load_or_create(&config.data_dir.join("device.key"))?;
        let store = Store::open(&config.data_dir.join("node.db")).await?;

        // Every binding we hold, back into the book. Expired ones are refused
        // by the book and simply not loaded.
        let now = Timestamp::now();
        let mut book = BindingBook::new();
        for ev in store.query(&Filter::new().kind(DEVICE_BINDING)).await? {
            let _ = book.insert(ev, now);
        }
        let own_binding = own_binding(&person, &device, &store, &mut book, now).await?;

        let mut groups = Groups::new(person.public_key());
        for g in store.carried_groups().await? {
            groups.carry(g);
            let state = GroupState::fold(g, &group_events(&store, g).await?);
            groups.set_state(g, state);
        }
        for list in store.query(&Filter::new().kind(dm::INBOX_LIST)).await? {
            groups.set_inboxes(list.pubkey, inbox_npubs(&list));
        }

        let builder = match config.relays {
            Relays::N0 => Endpoint::builder(presets::N0),
            Relays::Disabled => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
            Relays::Custom(url) => Endpoint::builder(presets::N0)
                .relay_mode(RelayMode::Custom(iroh::RelayMap::from(url.clone()))),
        };
        let builder = match config.port {
            Some(port) => builder
                .bind_addr(SocketAddr::from(([0, 0, 0, 0], port)))
                .map_err(|e| NodeError::Bind(e.to_string()))?,
            None => builder,
        };
        let gate = gossip::GateHook::default();
        let endpoint = builder
            .secret_key(device.clone())
            .alpns(vec![ALPN.to_vec(), iroh_blobs::ALPN.to_vec()])
            .hooks(gate.clone())
            .bind()
            .await
            .map_err(|e| NodeError::Bind(e.to_string()))?;
        // Addresses we are told out of band (an invite's `@ip:port`, the
        // CLI's `peer`), next to whatever discovery the preset has.
        let lookup = MemoryLookup::new();
        endpoint
            .address_lookup()
            .map_err(|e| NodeError::Bind(e.to_string()))?
            .add(lookup.clone());

        let blob_store =
            Arc::new(blobs::Blobs::open(&config.data_dir.join("blobs"), store.clone()).await?);
        let (events, _) = broadcast::channel(1024);
        let (removed, _) = broadcast::channel(256);
        let (intake, mut intake_rx) = mpsc::channel::<Event>(1024);
        let inner = Arc::new(Inner {
            person,
            device,
            endpoint,
            lookup,
            store,
            book: Mutex::new(book),
            groups: RwLock::new(groups),
            own_binding: Mutex::new(own_binding),
            events,
            blobs: blob_store,
            removed,
            gossip: gossip::Gossips::default(),
            intake,
            hints: Mutex::new(HashSet::new()),
            heard: Mutex::new(HashSet::new()),
            reached: Mutex::new(HashMap::new()),
            present: Mutex::new(HashMap::new()),
            voice: Mutex::new(HashMap::new()),
            protocols: RwLock::new(HashMap::new()),
            inbox_devices: Mutex::new(BTreeSet::new()),
            folded: Mutex::new(HashMap::new()),
            fold_lock: tokio::sync::RwLock::new(()),
            tasks: Mutex::new(Vec::new()),
        });

        gate.set(&inner);

        // Gossip payloads take the one way in, one at a time.
        let weak = Arc::downgrade(&inner);
        let intake_task = tokio::spawn(async move {
            while let Some(ev) = intake_rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                if let Err(e) = inner.accept_event(ev, Source::Gossip).await {
                    tracing::warn!("storing a gossiped event failed: {e}");
                }
            }
        });
        let (stop, stop_rx) = watch::channel(false);
        let accept_task = tokio::spawn(accept::run(inner.clone(), stop_rx));
        inner.tasks().extend([intake_task, accept_task]);

        let carried: Vec<PublicKey> = inner.groups().carried().copied().collect();
        for g in carried {
            inner.group_changed(g).await;
        }
        Ok(Self { inner, stop })
    }

    pub fn id(&self) -> EndpointId {
        self.inner.endpoint.id()
    }

    pub fn addr(&self) -> EndpointAddr {
        self.inner.endpoint.addr()
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.inner.endpoint
    }

    pub fn person(&self) -> PublicKey {
        self.inner.person.public_key()
    }

    pub fn npub(&self) -> String {
        self.inner.person.npub()
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// This device's current binding, as presented to peers.
    pub fn binding(&self) -> Event {
        self.inner.own_binding().clone()
    }

    /// Events as they are stored — after the store has them, never before.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Ids of events dropped from the store after they were stored:
    /// withdrawn by their author, removed by a moderator, or silenced by a
    /// ban or mute that arrived after them.
    pub fn subscribe_removed(&self) -> broadcast::Receiver<Vec<nostr::prelude::EventId>> {
        self.inner.removed.subscribe()
    }

    /// Sets this person's inbox list (kind 10050): the anchors that keep
    /// their DMs while they are away — each with the device to dial, from
    /// the bindings held here. Empty: none. Dated after the previous one.
    pub async fn set_inbox(&self, anchors: &[PublicKey]) -> Result<Event, NodeError> {
        let now = Timestamp::now();
        let named: Vec<(PublicKey, EndpointId)> = {
            let book = self.inner.book();
            anchors
                .iter()
                .flat_map(|a| book.devices_of(a, now).into_iter().map(move |d| (*a, d)))
                .collect()
        };
        let me = self.person();
        let current = self
            .inner
            .store
            .query(&Filter::new().kind(dm::INBOX_LIST).author(me).limit(1))
            .await?;
        let at = current.first().map_or(now, |c| now.max(c.created_at + 1));
        self.sign_and_publish(dm::inbox_list(&named).custom_created_at(at))
            .await
    }

    /// `person`'s inbox anchors, as their list (held here) names them.
    pub async fn inbox_of(
        &self,
        person: &PublicKey,
    ) -> Result<Vec<(PublicKey, Option<EndpointId>)>, NodeError> {
        let list = self
            .inner
            .store
            .query(&Filter::new().kind(dm::INBOX_LIST).author(*person).limit(1))
            .await?;
        Ok(list.first().map(dm::inboxes_of).unwrap_or_default())
    }

    /// A link to this person and this device (and their other devices
    /// known here), for someone with no group in common to write to them.
    /// `addr` is for relays off: where this device listens.
    pub fn contact_link(&self, addr: Option<std::net::SocketAddr>) -> ContactLink {
        let me = self.person();
        let mut devices = vec![(self.id(), addr)];
        devices.extend(
            self.devices_of(&me)
                .into_iter()
                .filter(|d| *d != self.id())
                .map(|d| (d, None)),
        );
        ContactLink {
            person: me,
            devices,
        }
    }

    /// Opens a contact link: dials its devices, keeps the binding of each
    /// that proves (in the `hit/nostr/1` hello) it speaks for the person
    /// named. Returns that person; an error if no device of theirs did.
    pub async fn add_contact(&self, link: &ContactLink) -> Result<PublicKey, NodeError> {
        if link.person == self.person() {
            return Err(NodeError::Protocol("that is your own link".into()));
        }
        let mut last = NodeError::Protocol("none of their devices answered".into());
        let mut any = false;
        for (d, addr) in &link.devices {
            let peer = match addr {
                Some(a) => {
                    let p = EndpointAddr::from_parts(*d, [iroh::TransportAddr::Ip(*a)]);
                    self.add_peer(p.clone());
                    p
                }
                None => EndpointAddr::new(*d),
            };
            match sync::introduce(&self.inner, peer).await {
                Ok((person, binding)) if person == link.person => {
                    self.inner.learn_binding(binding, person).await?;
                    any = true;
                }
                Ok(_) => {
                    last =
                        NodeError::Protocol("a device in the link speaks for someone else".into());
                }
                Err(e) => last = e,
            }
        }
        if any { Ok(link.person) } else { Err(last) }
    }

    /// Sends a DM (NIP-17): two wraps, ours and theirs, stored here; theirs
    /// handed at once to their devices and their inbox anchors, and synced
    /// to them until one has it. Returns the message as we see it.
    pub async fn send_dm(&self, to: &PublicKey, text: &str) -> Result<dm::Dm, NodeError> {
        let (theirs, mine) = self.inner.person.wrap_dm(to, text, Some(&self.binding()))?;
        self.deliver_dm(to, theirs, mine).await
    }

    /// Sends a file in a DM (NIP-17 kind 15, M8b): encrypted here with a
    /// fresh key, the ciphertext added to this node's store and granted to
    /// the two people, the key sealed in the message. No node — an anchor
    /// keeping the wrap, a member asking for the bytes — can open it.
    pub async fn send_dm_file(
        &self,
        to: &PublicKey,
        path: &std::path::Path,
    ) -> Result<dm::Dm, NodeError> {
        let plain = tokio::fs::read(path)
            .await
            .map_err(|e| NodeError::Blob(e.to_string()))?;
        let size = plain.len() as u64;
        if size == 0 || size > group::MAX_ATTACHMENT_BYTES {
            return Err(NodeError::Blob(format!(
                "a file must be 1 byte to {} MB",
                group::MAX_ATTACHMENT_BYTES / 1_000_000
            )));
        }
        let (sealed, key, nonce, original_sha256, sha256) = dm::seal_file(&plain)?;
        drop(plain);
        let hash = self.inner.blobs.add_bytes(sealed).await?;
        let me = self.person();
        self.inner.store.grant_blob(&hash, to).await?;
        self.inner.store.grant_blob(&hash, &me).await?;
        self.inner.store.touch_blob(&hash, true).await?;
        let att = self.attach_meta(path);
        let file = dm::DmFile {
            hash,
            sha256,
            original_sha256,
            size,
            mime: att.0,
            name: att.1,
            key,
            nonce,
        };
        let (theirs, mine) = self
            .inner
            .person
            .wrap_dm_file(to, &file, Some(&self.binding()))?;
        self.deliver_dm(to, theirs, mine).await
    }

    /// A DM file's bytes, decrypted: the ciphertext fetched (from the other
    /// person's devices, or this person's others) if it is not here,
    /// verified by hash, opened with the key from the sealed message, and
    /// checked against the original's hash.
    pub async fn dm_file_bytes(&self, dm_id: &str) -> Result<Vec<u8>, NodeError> {
        let d = self
            .dms()
            .await?
            .into_iter()
            .find(|d| d.id == dm_id)
            .ok_or_else(|| NodeError::Blob("no such DM here".into()))?;
        let file = d
            .file
            .ok_or_else(|| NodeError::Blob("that DM has no file".into()))?;
        let h = blobs::parse_hash(&file.hash)?;
        if !self.inner.blobs.complete(&h).await? {
            let me = self.person();
            let other = if d.from == me { d.to } else { d.from };
            let now = Timestamp::now();
            let mine = self.id();
            let devices: Vec<EndpointAddr> = {
                let book = self.inner.book();
                book.devices_of(&other, now)
                    .into_iter()
                    .chain(book.devices_of(&me, now))
                    .filter(|x| *x != mine)
                    .map(EndpointAddr::new)
                    .collect()
            };
            self.inner
                .blobs
                .fetch_first(&self.inner.endpoint, devices, &h)
                .await?;
        }
        self.inner.store.touch_blob(&file.hash, false).await?;
        let sealed = self.inner.blobs.bytes(&h).await?;
        dm::open_file(&file, &sealed)
            .ok_or_else(|| NodeError::Blob("the file did not open with its key".into()))
    }

    /// `(media type, file name)` for a path, as `attach` gives them.
    fn attach_meta(&self, path: &std::path::Path) -> (String, String) {
        let name: String = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into())
            .replace(['/', '\\', '\0'], "_")
            .chars()
            .take(group::MAX_ATTACHMENT_NAME)
            .collect();
        (guess_mime(path), name)
    }

    /// Stores both wraps of a DM and hands the recipient's to their devices
    /// and inbox anchors; returns the message as we see it.
    async fn deliver_dm(
        &self,
        to: &PublicKey,
        theirs: Event,
        mine: Event,
    ) -> Result<dm::Dm, NodeError> {
        let opened = self
            .inner
            .person
            .open_dm(&mine)
            .ok_or_else(|| NodeError::Protocol("our own DM did not open".into()))?;
        self.inner.publish(mine).await?;
        self.inner.publish(theirs.clone()).await?;
        let now = Timestamp::now();
        let mut devices: Vec<EndpointId> = self.inner.book().devices_of(to, now);
        for (_, d) in self.inbox_of(to).await? {
            devices.extend(d);
        }
        devices.retain(|d| *d != self.id());
        for d in devices {
            self.inner.hints().insert(d);
            let (inner, ev) = (self.inner.clone(), theirs.clone());
            tokio::spawn(async move {
                if let Err(e) = sync::deliver(&inner, EndpointAddr::new(d), ev).await {
                    tracing::debug!("handing a DM to {} failed: {e}", d.fmt_short());
                }
            });
        }
        Ok(opened)
    }

    /// Every DM this node holds for this person — sent and received — opened,
    /// oldest first, each once.
    pub async fn dms(&self) -> Result<Vec<dm::Dm>, NodeError> {
        let me = self.person();
        let wraps = self
            .inner
            .store
            .query(
                &Filter::new()
                    .kind(dm::GIFT_WRAP)
                    .custom_tag(SingleLetterTag::LOWERCASE_P, me.to_hex()),
            )
            .await?;
        let mut seen = HashSet::new();
        let mut out: Vec<dm::Dm> = wraps
            .iter()
            .filter_map(|w| self.inner.person.open_dm(w))
            .filter(|d| seen.insert(d.id.clone()))
            .collect();
        out.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Opens one wrap, if it is a DM to this person. For the window's live
    /// feed.
    pub fn open_dm(&self, wrap: &Event) -> Option<dm::Dm> {
        self.inner.person.open_dm(wrap)
    }

    // --- files (M8) --------------------------------------------------------

    /// What this node keeps: a member's cache, or an anchor's everything.
    pub fn set_file_policy(&self, policy: FilePolicy) {
        *self
            .inner
            .blobs
            .policy
            .write()
            .unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Adds a file from disk to this node's store, ready to attach to a
    /// message: its hashes, size, name and a type guessed from its name.
    pub async fn attach(&self, path: &std::path::Path) -> Result<group::Attachment, NodeError> {
        let (hash, sha256, size) = self.inner.blobs.add_path(path).await?;
        self.inner.store.touch_blob(&hash, true).await?;
        let name: String = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into())
            .replace(['/', '\\', '\0'], "_")
            .chars()
            .take(group::MAX_ATTACHMENT_NAME)
            .collect();
        Ok(group::Attachment {
            hash,
            sha256,
            size,
            mime: guess_mime(path),
            name,
        })
    }

    /// Posts `text` with `files` (each from [`Self::attach`]) to `channel`.
    pub async fn say_with(
        &self,
        group: &PublicKey,
        channel: &str,
        text: &str,
        files: &[group::Attachment],
    ) -> Result<Event, NodeError> {
        self.sign_and_publish(group::message_with(group, channel, text, files))
            .await
    }

    /// Makes sure this node holds the file `hash`, fetching it if not —
    /// from whoever may have it: the author of a message carrying it, the
    /// group's anchors, then every other member. Each serves only if we may
    /// read that message, and every byte is checked against the hash.
    pub async fn fetch_file(&self, hash: &str) -> Result<(), NodeError> {
        let h = blobs::parse_hash(hash)?;
        if self.inner.blobs.complete(&h).await? {
            self.inner.store.touch_blob(hash, false).await?;
            return Ok(());
        }
        let carrying = self.inner.store.carrying(hash).await?;
        if carrying.is_empty() {
            return Err(NodeError::Blob("no message here carries that file".into()));
        }
        let now = Timestamp::now();
        let me = self.id();
        let mut order: Vec<EndpointId> = Vec::new();
        let push = |d: EndpointId, order: &mut Vec<EndpointId>| {
            if d != me && !order.contains(&d) {
                order.push(d);
            }
        };
        for ev in &carrying {
            for d in self.inner.book().devices_of(&ev.pubkey, now) {
                push(d, &mut order);
            }
        }
        for ev in &carrying {
            if let Some(g) = group_of(ev) {
                for a in self.anchors(&g).await? {
                    for d in self.inner.book().devices_of(&a, now) {
                        push(d, &mut order);
                    }
                }
                for d in self.inner.member_devices(&g) {
                    push(d, &mut order);
                }
            }
        }
        self.inner
            .blobs
            .fetch_first(
                &self.inner.endpoint,
                order.into_iter().map(EndpointAddr::new).collect(),
                &h,
            )
            .await?;
        self.inner.store.touch_blob(hash, false).await?;
        Ok(())
    }

    /// Asks `from` for the file `hash` directly, whatever this node holds.
    /// For tests of the serving gate: a stranger, or a member asking for a
    /// file in a channel they cannot see, must be refused.
    pub async fn fetch_file_from(&self, from: EndpointAddr, hash: &str) -> Result<(), NodeError> {
        let h = blobs::parse_hash(hash)?;
        self.inner.blobs.fetch(&self.inner.endpoint, from, &h).await
    }

    /// The bytes of a file this node holds (fetch it first).
    pub async fn file_bytes(&self, hash: &str) -> Result<Vec<u8>, NodeError> {
        let h = blobs::parse_hash(hash)?;
        self.inner.store.touch_blob(hash, false).await?;
        self.inner.blobs.bytes(&h).await
    }

    /// Writes a file this node holds to `to`.
    pub async fn save_file(&self, hash: &str, to: &std::path::Path) -> Result<(), NodeError> {
        let h = blobs::parse_hash(hash)?;
        self.inner.blobs.export(&h, to).await
    }

    /// Fetches every file a held message carries that is not here yet — an
    /// anchor's pin (M8). Returns how many it got.
    pub async fn pin_files(&self) -> Result<usize, NodeError> {
        let mut got = 0;
        for (hash, _) in self.inner.store.attached().await? {
            let h = blobs::parse_hash(&hash)?;
            if self.inner.blobs.complete(&h).await? {
                continue;
            }
            match self.fetch_file(&hash).await {
                Ok(()) => got += 1,
                Err(e) => tracing::debug!("pinning {hash}: {e}"),
            }
        }
        Ok(got)
    }

    // --- for voice (M9) ----------------------------------------------------

    /// Serves `alpn` on this node's endpoint with `handler`: each incoming
    /// connection on it, after the handshake. For protocols that live in
    /// other crates (`hit-voice`); they gate their own peers.
    pub fn serve_protocol(&self, alpn: &[u8], handler: ProtocolFn) {
        self.inner
            .protocols
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(alpn.to_vec(), handler);
        self.inner.update_alpns();
    }

    /// The persons a device speaks for, by the bindings held here.
    pub fn persons_of(&self, device: EndpointId) -> Vec<PublicKey> {
        self.inner.book().persons_of(device, Timestamp::now())
    }

    /// A person's devices, by the bindings held here.
    pub fn devices_of(&self, person: &PublicKey) -> Vec<EndpointId> {
        self.inner.book().devices_of(person, Timestamp::now())
    }

    /// The person a connection's device speaks for, from a binding it
    /// `presented` (for protocols that carry one: a 1:1 call). `device` is
    /// read off the connection by the caller; the binding must name it, and
    /// nothing this node holds may have revoked it.
    pub fn authenticate(
        &self,
        presented: &Event,
        device: EndpointId,
    ) -> Result<PublicKey, NodeError> {
        self.inner
            .book()
            .authenticate(presented, device, Timestamp::now())
            .map_err(|e| NodeError::Protocol(format!("binding: {e}")))
    }

    /// Whether `person` and this one are both members of some group.
    pub fn shares_group(&self, person: &PublicKey) -> bool {
        let me = self.person();
        self.groups()
            .into_iter()
            .any(|(_, s)| s.is_some_and(|s| s.is_member(&me) && s.is_member(person)))
    }

    /// Announces this person in (or out of) voice channel `channel`.
    ///
    /// A channel everyone can view: on the group's gossip, like any
    /// presence. **A channel not everyone can view**: never on gossip —
    /// that would tell every member who sits in a room they cannot see —
    /// but handed directly to each device of each member who may view it,
    /// over `hit/nostr/1`, the way a DM is handed to its recipient. Each
    /// receiver judges it like any event.
    pub async fn announce_voice(
        &self,
        group: &PublicKey,
        channel: &str,
        inside: bool,
    ) -> Result<(), NodeError> {
        let ev = self
            .inner
            .person
            .sign(group::voice_presence(group, channel, inside))?;
        let restricted = self.inner.groups().gossip_group(&ev).is_none();
        let (ok, why) = self.inner.accept_event(ev.clone(), Source::Local).await?;
        if !ok {
            return Err(NodeError::Protocol(why));
        }
        if restricted {
            for d in self.voice_viewers(group, channel) {
                let (inner, ev) = (self.inner.clone(), ev.clone());
                tokio::spawn(async move {
                    let handed = tokio::time::timeout(
                        VOICE_HAND_TIMEOUT,
                        sync::deliver(&inner, EndpointAddr::new(d), ev),
                    )
                    .await;
                    if !matches!(handed, Ok(Ok(_))) {
                        tracing::debug!("voice presence to {} not delivered", d.fmt_short());
                    }
                });
            }
        }
        Ok(())
    }

    /// Every device, other than this one, of every member who may view
    /// `channel` of `group`.
    fn voice_viewers(&self, group: &PublicKey, channel: &str) -> Vec<EndpointId> {
        let Some(state) = self.group(group) else {
            return Vec::new();
        };
        let me = self.id();
        let now = Timestamp::now();
        let book = self.inner.book();
        state
            .members()
            .filter(|p| {
                state
                    .effective(p, channel)
                    .contains(hit_group::Permissions::VIEW_CHANNEL)
            })
            .flat_map(|p| book.devices_of(p, now))
            .filter(|d| *d != me)
            .collect()
    }

    /// Who is in voice channel `channel` of `group`: an "in" heard within
    /// `within`, and no "out" since. This node's own person too, if it
    /// announced.
    pub fn voice_participants(
        &self,
        group: &PublicKey,
        channel: &str,
        within: Duration,
    ) -> BTreeSet<PublicKey> {
        let now = tokio::time::Instant::now();
        self.inner
            .voice
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(*group, channel.to_owned()))
            .map(|room| {
                room.iter()
                    .filter(|(_, at)| now.duration_since(**at) <= within)
                    .map(|(p, _)| *p)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Bans `target` (`9001` with `["ban"]`).
    pub async fn ban(&self, group: &PublicKey, target: &PublicKey) -> Result<Event, NodeError> {
        self.sign_and_publish(group::ban_user(group, target)).await
    }

    /// Lifts `target`'s ban.
    pub async fn unban(&self, group: &PublicKey, target: &PublicKey) -> Result<Event, NodeError> {
        self.sign_and_publish(group::unban_user(group, target))
            .await
    }

    /// Mutes `target` until `until`, or lifts a mute (`None`).
    pub async fn mute(
        &self,
        group: &PublicKey,
        target: &PublicKey,
        until: Option<Timestamp>,
    ) -> Result<Event, NodeError> {
        self.sign_and_publish(group::mute_user(group, target, until))
            .await
    }

    /// Changes the group (`9002`): every edit, or none.
    pub async fn edit(&self, group: &PublicKey, edits: &[group::Edit]) -> Result<Event, NodeError> {
        self.sign_and_publish(group::edit_metadata(group, edits))
            .await
    }

    /// A moderator removing messages or reactions in `channel` (`9005`).
    pub async fn remove_events(
        &self,
        group: &PublicKey,
        channel: &str,
        targets: &[nostr::prelude::EventId],
    ) -> Result<Event, NodeError> {
        self.sign_and_publish(group::delete_event(group, channel, targets))
            .await
    }

    /// Hands `event` to `peer` as it is, **without judging it here first**,
    /// and returns the peer's answer. Only for tests of the refusal path —
    /// "the CLI, lying" (PLAN M5): an honest node publishes through its own
    /// judgement.
    pub async fn push_unjudged(
        &self,
        peer: EndpointAddr,
        event: Event,
    ) -> Result<(bool, String), NodeError> {
        sync::deliver(&self.inner, peer, event).await
    }

    /// Signs `builder` as this node's person, without judging or storing it.
    /// With [`Self::push_unjudged`], how a patched client behaves.
    pub fn sign_unjudged(&self, builder: EventBuilder) -> Result<Event, NodeError> {
        Ok(self.inner.person.sign(builder)?)
    }

    /// Tells the node where a device is, and to sync with it even before a
    /// binding names it. With relays on, the id alone is enough.
    pub fn add_peer(&self, addr: EndpointAddr) {
        if addr.id == self.id() {
            return;
        }
        if !addr.addrs.is_empty() {
            self.inner.lookup.set_endpoint_info(addr.clone());
        }
        self.inner.hints().insert(addr.id);
    }

    /// The groups this node carries, with their state where it has one.
    pub fn groups(&self) -> Vec<(PublicKey, Option<GroupState>)> {
        let groups = self.inner.groups();
        groups
            .carried()
            .map(|g| (*g, groups.state(g).cloned()))
            .collect()
    }

    pub fn group(&self, group: &PublicKey) -> Option<GroupState> {
        self.inner.groups().state(group).cloned()
    }

    /// Creates a group owned by this node's person: a new group key (saved
    /// to `secrets`), its first snapshot — one `general` channel and
    /// HIT_ENTER_2's role ladder — and the owner's `9007`.
    pub async fn create_group(
        &self,
        name: &str,
        secrets: &(dyn SecretStore + Sync),
    ) -> Result<PublicKey, NodeError> {
        self.create_group_with(name, false, secrets).await
    }

    /// A new group, public (M13: anyone may find, read and join it) or
    /// private, which is the default.
    pub async fn create_group_with(
        &self,
        name: &str,
        public: bool,
        secrets: &(dyn SecretStore + Sync),
    ) -> Result<PublicKey, NodeError> {
        let key = GroupKey::generate();
        key.save(secrets)?;
        let g = key.public_key();
        let now = Timestamp::now();
        self.inner.store.carry_group(&g, now).await?;
        self.inner.groups_mut().carry(g);
        for ev in genesis(&self.inner.person, &key, name, public, now)? {
            self.inner.publish(ev).await?;
        }
        Ok(g)
    }

    /// Signs and stores a new snapshot of `group` at `seq + 1` — compaction,
    /// by a holder of the group key (`DESIGN.md` §6 step 4).
    pub async fn compact(
        &self,
        group: &PublicKey,
        secrets: &(dyn SecretStore + Sync),
    ) -> Result<u64, NodeError> {
        let key =
            GroupKey::load(secrets, &group.to_bech32_or_hex())?.ok_or(NodeError::NoGroupKey)?;
        let me = self.person();
        let (seq, events) = compaction(&self.inner.store, &key, me).await?;
        for ev in events {
            self.inner.publish(ev).await?;
        }
        Ok(seq)
    }

    /// Mints an invite to `group` and returns its link, naming this device
    /// (at `addr`, when relays are off) as the bootstrap peer.
    pub async fn invite(
        &self,
        group: &PublicKey,
        valid_for: Option<Duration>,
        addr: Option<SocketAddr>,
    ) -> Result<InviteLink, NodeError> {
        let code = group::new_invite_code()?;
        let expires = valid_for.map(|d| Timestamp::now() + d);
        self.sign_and_publish(group::create_invite(group, &code, expires))
            .await?;
        Ok(InviteLink {
            group: *group,
            code: Some(code),
            peers: invite_peers(&self.inner.store, self.person(), group, self.id(), addr).await?,
        })
    }

    /// A public group's link (M13): no invite to mint, so nothing is
    /// signed and it never expires — it names the group and where to find
    /// it, its anchors first. Refused for a group that is not public.
    pub async fn public_link(
        &self,
        group: &PublicKey,
        addr: Option<SocketAddr>,
    ) -> Result<InviteLink, NodeError> {
        if !self.inner.groups().state(group).is_some_and(|s| s.is_public()) {
            return Err(NodeError::Refused(Refusal::NoSuchInvite));
        }
        Ok(InviteLink {
            group: *group,
            code: None,
            peers: invite_peers(&self.inner.store, self.person(), group, self.id(), addr).await?,
        })
    }

    /// Reads a public group without joining it (M13): carries it **in
    /// memory only** — a restart puts it down; joining keeps it — and syncs
    /// it from the link's peers, which serve a public group to anyone.
    /// Refused, and put down again, if the group turns out not public.
    pub async fn follow(&self, link: &InviteLink) -> Result<SyncStats, NodeError> {
        let g = link.group;
        let already = self.inner.groups().is_carried(&g);
        if !already {
            self.inner.groups_mut().carry(g);
        }
        let mut why: Vec<String> = Vec::new();
        for (id, addr) in &link.peers {
            let peer = match addr {
                Some(a) => EndpointAddr::from_parts(*id, [TransportAddr::Ip(*a)]),
                None => EndpointAddr::new(*id),
            };
            self.add_peer(peer.clone());
            match sync::sync(&self.inner, peer, None).await {
                Ok(stats) if self.group(&g).is_some_and(|s| s.is_public()) => {
                    return Ok(stats);
                }
                Ok(_) => why.push(format!("{}: not a public group", id.fmt_short())),
                Err(e) => why.push(format!("{}: {e}", id.fmt_short())),
            }
        }
        if !already {
            self.inner.groups_mut().drop_group(&g);
        }
        if why.is_empty() {
            why.push("no peers in the link".into());
        }
        Err(NodeError::JoinFailed(why.join("; ")))
    }

    /// Joins a group from an invite link: carry the group, hand a `9021` to
    /// a bootstrap peer — nobody else needs to be online — then sync with
    /// it, which brings the group's state and our own join back.
    pub async fn join(&self, link: &InviteLink) -> Result<SyncStats, NodeError> {
        let g = link.group;
        self.inner.store.carry_group(&g, Timestamp::now()).await?;
        self.inner.groups_mut().carry(g);
        let ev = self.inner.person.sign(match &link.code {
            Some(code) => group::join(&g, code),
            None => group::join_public(&g),
        })?;
        // Every peer's answer, not just the last: an anchor's refusal must
        // not be hidden behind the timeout of a member's app that is off.
        let mut why: Vec<String> = Vec::new();
        for (id, addr) in &link.peers {
            let peer = match addr {
                Some(a) => EndpointAddr::from_parts(*id, [TransportAddr::Ip(*a)]),
                None => EndpointAddr::new(*id),
            };
            self.add_peer(peer.clone());
            match sync::deliver(&self.inner, peer.clone(), ev.clone()).await {
                Ok((true, _)) => return sync::sync(&self.inner, peer, None).await,
                Ok((false, w)) => why.push(format!("{}: {w}", id.fmt_short())),
                Err(e) => why.push(format!("{}: {e}", id.fmt_short())),
            }
        }
        if why.is_empty() {
            why.push("no peers in the link".into());
        }
        Err(NodeError::JoinFailed(why.join("; ")))
    }

    /// Posts `text` to `channel` of `group`.
    pub async fn say(
        &self,
        group: &PublicKey,
        channel: &str,
        text: &str,
    ) -> Result<Event, NodeError> {
        self.sign_and_publish(group::message(group, channel, text))
            .await
    }

    /// Puts `target` in `group` with exactly `roles` (`9000`).
    pub async fn put(
        &self,
        group: &PublicKey,
        target: &PublicKey,
        roles: &[&str],
    ) -> Result<Event, NodeError> {
        self.sign_and_publish(group::put_user(group, target, roles))
            .await
    }

    /// Removes `target` from `group` (`9001`).
    pub async fn remove(&self, group: &PublicKey, target: &PublicKey) -> Result<Event, NodeError> {
        self.sign_and_publish(group::remove_user(group, target))
            .await
    }

    /// The messages of `group` this node holds, oldest first; one channel,
    /// or all.
    pub async fn messages(
        &self,
        group: &PublicKey,
        channel: Option<&str>,
    ) -> Result<Vec<Event>, NodeError> {
        let mut out = messages(&self.inner.store, group).await?;
        if let Some(c) = channel {
            out.retain(|e| group::channel_of(e) == Some(c));
        }
        Ok(out)
    }

    /// Sets this person's display name: their kind 0, with every other
    /// field of the current one kept (`hit_event::profile`), dated after it.
    pub async fn set_name(&self, name: &str) -> Result<Event, NodeError> {
        self.set_profile(|prev| hit_event::profile::with_name(prev, name))
            .await
    }

    /// Marks this node's person as an anchor (M6), named `name`: NIP-24's
    /// `bot` in its kind 0. Apps say so, and name its device in invite links.
    pub async fn set_anchor_profile(&self, name: &str) -> Result<Event, NodeError> {
        self.set_profile(|prev| hit_event::profile::as_anchor(prev, name))
            .await
    }

    /// Our kind 0, rebuilt from the current one by `build`, **dated after
    /// it**: two in the same second would be settled by NIP-01's lowest-id
    /// rule, not by which is newer.
    async fn set_profile(
        &self,
        build: impl FnOnce(Option<&Event>) -> EventBuilder,
    ) -> Result<Event, NodeError> {
        let current = self.own_profile().await?;
        let at = current
            .as_ref()
            .map_or_else(Timestamp::now, |c| Timestamp::now().max(c.created_at + 1));
        self.sign_and_publish(build(current.as_ref()).custom_created_at(at))
            .await
    }

    /// Sets this person's Lightning address and the key its server signs
    /// zap receipts with (M10) — or removes both. The key is what every
    /// member checks a receipt for them against; the app reads it from the
    /// address's server when the address is set.
    pub async fn set_lud16(&self, address: Option<(&str, &PublicKey)>) -> Result<Event, NodeError> {
        self.set_profile(|prev| hit_event::zap::with_lud16(prev, address))
            .await
    }

    /// Says where this anchor's public `wss://` relay is (M10), so zap
    /// receipts for its groups can be delivered there.
    pub async fn set_anchor_relay(&self, url: Option<&str>) -> Result<Event, NodeError> {
        self.set_profile(|prev| hit_event::profile::with_relay(prev, url))
            .await
    }

    /// The `wss://` relays of `group`'s anchors: the only relays a zap in
    /// this group names for its receipt (`DESIGN.md` §11).
    pub async fn receipt_relays(&self, group: &PublicKey) -> Result<Vec<String>, NodeError> {
        let anchors = self.anchors(group).await?;
        if anchors.is_empty() {
            return Ok(Vec::new());
        }
        let profiles = self
            .inner
            .store
            .query(&Filter::new().kind(hit_group::PROFILE).authors(anchors))
            .await?;
        Ok(profiles
            .iter()
            .filter_map(hit_event::profile::relay_of)
            .collect())
    }

    /// A person's Lightning address and their provider's key, from the
    /// profile held here.
    pub async fn lud16_of(
        &self,
        person: &PublicKey,
    ) -> Result<Option<(String, Option<PublicKey>)>, NodeError> {
        let profile = self
            .inner
            .store
            .query(&Filter::new().kind(hit_group::PROFILE).author(*person).limit(1))
            .await?;
        Ok(profile.first().and_then(|p| {
            Some((
                hit_event::zap::lud16_of(p)?,
                hit_event::zap::zap_pubkey_of(p),
            ))
        }))
    }

    /// Brings a zap receipt into the group of the message it zaps (M10):
    /// checked by NIP-57's rules and against the recipient's provider key,
    /// then carried ([`hit_event::zap::carrier`]) and synced like any
    /// event. `None` if this receipt is already carried here. The sender's
    /// app calls it with the receipt it fetched; an anchor, with one
    /// delivered to its `wss://`.
    pub async fn carry_zap(&self, receipt: &Event) -> Result<Option<Event>, NodeError> {
        let zap = hit_event::zap::check_receipt(receipt)?;
        let held = self.inner.store.by_ids(&[zap.target]).await?;
        let target = held.first().ok_or(NodeError::NoSuchEvent)?;
        let (Some(g), Some(channel)) = (group_of(target), group::channel_of(target)) else {
            return Err(NodeError::NoSuchEvent);
        };
        if target.pubkey != zap.recipient {
            return Err(hit_event::zap::ZapError::Mismatch.into());
        }
        let provider = self.lud16_of(&zap.recipient).await?.and_then(|(_, k)| k);
        if provider != Some(zap.provider) {
            return Err(NodeError::NotTheirProvider);
        }
        if self.zaps_of(&[zap.target]).await?.iter().any(|z| z.receipt == zap.receipt) {
            return Ok(None);
        }
        let channel = channel.to_string();
        self.sign_and_publish(hit_event::zap::carrier(&g, &channel, receipt, &zap))
            .await
            .map(Some)
    }

    /// Every zap of `targets` this node holds that counts: a valid receipt,
    /// for the target's author, signed by the provider **their profile**
    /// names — a receipt signed by any other key is held and shown nowhere.
    /// One per receipt, however many members carried it.
    pub async fn zaps_of(
        &self,
        targets: &[nostr::prelude::EventId],
    ) -> Result<Vec<hit_event::zap::Zap>, NodeError> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let carriers = self
            .inner
            .store
            .query(
                &Filter::new()
                    .kind(hit_event::zap::ZAP_CARRIER)
                    .events(targets.iter().copied()),
            )
            .await?;
        if carriers.is_empty() {
            return Ok(Vec::new());
        }
        let authors: HashMap<_, _> = self
            .inner
            .store
            .by_ids(targets)
            .await?
            .into_iter()
            .map(|e| (e.id, e.pubkey))
            .collect();
        let recipients: BTreeSet<PublicKey> = authors.values().copied().collect();
        let providers: HashMap<PublicKey, PublicKey> = self
            .inner
            .store
            .query(&Filter::new().kind(hit_group::PROFILE).authors(recipients))
            .await?
            .iter()
            .filter_map(|p| Some((p.pubkey, hit_event::zap::zap_pubkey_of(p)?)))
            .collect();
        let mut seen = BTreeSet::new();
        let mut zaps: Vec<_> = carriers
            .iter()
            .filter_map(|c| hit_event::zap::carried(c).ok())
            .filter(|z| {
                authors.get(&z.target) == Some(&z.recipient)
                    && providers.get(&z.recipient) == Some(&z.provider)
                    && seen.insert(z.receipt)
            })
            .collect();
        zaps.sort_by_key(|z| z.paid_at);
        Ok(zaps)
    }

    /// What each of `targets` was zapped ([`Self::zaps_of`]), for those
    /// that were.
    pub async fn zap_totals(
        &self,
        targets: &[nostr::prelude::EventId],
    ) -> Result<HashMap<nostr::prelude::EventId, ZapTotal>, NodeError> {
        let mut totals: HashMap<_, ZapTotal> = HashMap::new();
        for z in self.zaps_of(targets).await? {
            let t = totals.entry(z.target).or_default();
            t.msat += z.amount_msat;
            t.count += 1;
            if let Some(s) = z.sender
                && !t.zappers.contains(&s)
            {
                t.zappers.push(s);
            }
        }
        Ok(totals)
    }

    /// Our own kind 0, if we have one.
    pub async fn own_profile(&self) -> Result<Option<Event>, NodeError> {
        let me = self.person();
        Ok(self
            .inner
            .store
            .query(&Filter::new().kind(hit_group::PROFILE).author(me).limit(1))
            .await?
            .into_iter()
            .next())
    }

    /// The members of `group` whose profile says they are anchors.
    pub async fn anchors(&self, group: &PublicKey) -> Result<Vec<PublicKey>, NodeError> {
        let members: Vec<PublicKey> = self
            .group(group)
            .map(|s| s.members().copied().collect())
            .unwrap_or_default();
        if members.is_empty() {
            return Ok(Vec::new());
        }
        let profiles = self
            .inner
            .store
            .query(
                &Filter::new()
                    .kind(hit_group::PROFILE)
                    .authors(members.iter().copied()),
            )
            .await?;
        Ok(profiles
            .iter()
            .filter(|p| hit_event::profile::is_anchor(p))
            .map(|p| p.pubkey)
            .collect())
    }

    /// What `reader` may see of what this node holds, matching `filter` —
    /// the same answer `hit/nostr/1` gives. For a bridge that serves the
    /// node another way (an anchor's `ws://`, M6).
    pub async fn read_as(
        &self,
        reader: &PublicKey,
        filter: &Filter,
    ) -> Result<Vec<Event>, NodeError> {
        self.inner.visible(reader, filter).await
    }

    /// May `reader` see `event`? The one rule, for a bridge's live feed.
    pub fn may_read(&self, reader: &PublicKey, event: &Event) -> bool {
        self.inner
            .groups()
            .may_read(reader, event, Timestamp::now())
    }

    /// Hands `event` to this node from outside iroh — a bridge's `EVENT` —
    /// through the one gate every event passes. `(accepted, NIP-01 OK
    /// message)`.
    pub async fn accept(&self, event: Event) -> Result<(bool, String), NodeError> {
        self.inner.accept_event(event, Source::Push).await
    }

    /// The display names this node holds for `people` — those it has a
    /// profile for, with a name in it.
    pub async fn names(
        &self,
        people: &[PublicKey],
    ) -> Result<HashMap<PublicKey, String>, NodeError> {
        if people.is_empty() {
            return Ok(HashMap::new());
        }
        let evs = self
            .inner
            .store
            .query(
                &Filter::new()
                    .kind(hit_group::PROFILE)
                    .authors(people.iter().copied()),
            )
            .await?;
        Ok(evs
            .iter()
            .filter_map(|e| Some((e.pubkey, hit_event::profile::name_of(e)?)))
            .collect())
    }

    /// Reacts to `target` with `emoji`. The target must be held here: its
    /// author goes in the reaction's `p` tag.
    pub async fn react(
        &self,
        group: &PublicKey,
        channel: &str,
        target: &nostr::prelude::EventId,
        emoji: &str,
    ) -> Result<Event, NodeError> {
        let held = self.inner.store.by_ids(&[*target]).await?;
        let author = held
            .first()
            .map(|e| e.pubkey)
            .ok_or(NodeError::NoSuchEvent)?;
        self.sign_and_publish(group::reaction(group, channel, target, &author, emoji))
            .await
    }

    /// Withdraws our own `targets` — messages or reactions — in `channel`
    /// (NIP-09). Anyone else's are refused here rather than signed for
    /// nothing.
    pub async fn delete(
        &self,
        group: &PublicKey,
        channel: &str,
        targets: &[nostr::prelude::EventId],
    ) -> Result<Event, NodeError> {
        let me = self.person();
        let held = self.inner.store.by_ids(targets).await?;
        if held.len() != targets.len() || held.iter().any(|e| e.pubkey != me) {
            return Err(NodeError::NotYours);
        }
        self.sign_and_publish(group::deletion(group, channel, targets))
            .await
    }

    /// Says "I am here" on the gossip of every group we are a member of.
    pub async fn announce_presence(&self) -> Result<(), NodeError> {
        let me = self.person();
        let groups: Vec<PublicKey> = self
            .inner
            .groups()
            .groups_of(&me)
            .map(GroupState::id)
            .collect();
        for g in groups {
            let ev = self.inner.person.sign(group::presence(&g))?;
            self.inner.publish(ev).await?;
        }
        Ok(())
    }

    /// The people this node has reason to think are here: a device of
    /// theirs completed a session with us, or their presence reached us
    /// over gossip, within `within`.
    pub fn present(&self, within: Duration) -> BTreeSet<PublicKey> {
        let now = tokio::time::Instant::now();
        let mut out = self.reachable(within);
        out.extend(
            self.inner
                .present
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|(_, at)| now.duration_since(**at) <= within)
                .map(|(p, _)| *p),
        );
        out
    }

    /// The events of `kinds` in `channel` of `group`, oldest first.
    pub async fn channel_events(
        &self,
        group: &PublicKey,
        channel: &str,
        kinds: &[nostr::prelude::Kind],
    ) -> Result<Vec<Event>, NodeError> {
        let h = SingleLetterTag::LOWERCASE_H;
        let mut evs = self
            .inner
            .store
            .query(
                &Filter::new()
                    .kinds(kinds.iter().copied())
                    .custom_tag(h, group.to_hex()),
            )
            .await?;
        evs.retain(|e| group::channel_of(e) == Some(channel));
        evs.reverse();
        Ok(evs)
    }

    /// The people with a device this node completed a session with —
    /// either way — within `within`. Not presence: a person whose devices
    /// we have not tried lately is absent from this set, not offline.
    pub fn reachable(&self, within: Duration) -> BTreeSet<PublicKey> {
        let now = tokio::time::Instant::now();
        let devices: Vec<EndpointId> = self
            .inner
            .reached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, at)| now.duration_since(**at) <= within)
            .map(|(d, _)| *d)
            .collect();
        let ts = Timestamp::now();
        let book = self.inner.book();
        devices
            .into_iter()
            .flat_map(|d| book.persons_of(d, ts))
            .collect()
    }

    /// Signs and publishes our own event. **An edit, a role change, a
    /// removal or a mute is dated after this person's last one of the same
    /// kind in its group**: actions fold in `(created_at, id)` order, so a
    /// toggle and its undo made in the same second would otherwise apply in
    /// the order of their ids, which is chance. Only one's own actions of
    /// the same kind: dating anything after *another* person's action, or
    /// an invite after the group's creation, would put it ahead of what
    /// others sign after it — a join before its own invite.
    async fn sign_and_publish(&self, builder: EventBuilder) -> Result<Event, NodeError> {
        let mut ev = self.inner.person.sign(builder.clone())?;
        let ordered = [
            group::PUT_USER,
            group::REMOVE_USER,
            group::EDIT_METADATA,
            group::MUTE_USER,
        ];
        if ordered.contains(&ev.kind)
            && let Some(g) = group_of(&ev)
        {
            let me = self.person();
            let last = group_events(&self.inner.store, g)
                .await?
                .iter()
                .filter(|e| e.pubkey == me && e.kind == ev.kind)
                .map(|e| e.created_at)
                .max();
            if let Some(n) = last
                && n >= ev.created_at
            {
                ev = self
                    .inner
                    .person
                    .sign(builder.custom_created_at(n + 1u64))?;
            }
        }
        self.inner.publish(ev.clone()).await?;
        Ok(ev)
    }

    /// Reconciles everything both sides may see with `peer`, both ways.
    pub async fn sync_with(&self, peer: EndpointAddr) -> Result<SyncStats, NodeError> {
        sync::sync(&self.inner, peer, None).await
    }

    /// As [`Self::sync_with`], but presenting `binding` instead of our own.
    /// **Only for tests of the refusal path** — "the CLI, lying" (PLAN M2,
    /// M5). An honest node never presents a binding that is not its own.
    pub async fn sync_presenting(
        &self,
        peer: EndpointAddr,
        binding: Event,
    ) -> Result<SyncStats, NodeError> {
        sync::sync(&self.inner, peer, Some(binding)).await
    }

    /// Sends one `REQ` to `peer` and returns what it serves — without
    /// checking or storing it. For tests and the CLI: "what does this relay
    /// show me?"
    pub async fn fetch(&self, peer: EndpointAddr, filter: Filter) -> Result<Vec<Event>, NodeError> {
        sync::request(&self.inner, peer, filter).await
    }

    /// The devices this node syncs with: every device of everyone it shares
    /// a group with, plus the hints. Not its own.
    pub fn sync_targets(&self) -> Vec<EndpointId> {
        self.inner.sync_targets()
    }

    /// Syncs with every target every `every`, forever: each target in its
    /// own task, so a device that is down — a dial that takes its time to
    /// fail — never holds up the others. A failing target is backed off up
    /// to five minutes, until it reaches us itself. Also re-signs our
    /// binding when it is due, and re-dials each group's gossip members.
    pub fn spawn_sync_loop(&self, every: Duration) -> JoinHandle<()> {
        /// One sync may not take longer than this, whatever the dial does.
        const SYNC_TIMEOUT: Duration = Duration::from_secs(30);
        let node = self.clone();
        tokio::spawn(async move {
            let mut wait: HashMap<EndpointId, Duration> = HashMap::new();
            let mut due: HashMap<EndpointId, tokio::time::Instant> = HashMap::new();
            let mut busy: HashSet<EndpointId> = HashSet::new();
            let (done_tx, mut done_rx) = mpsc::unbounded_channel();
            loop {
                while let Ok((id, ok)) = done_rx.try_recv() {
                    busy.remove(&id);
                    let w = wait.entry(id).or_insert(Duration::ZERO);
                    *w = if ok {
                        Duration::ZERO
                    } else {
                        (*w * 2).clamp(every, Duration::from_secs(300))
                    };
                    due.insert(id, tokio::time::Instant::now() + (*w).max(every));
                }
                // A device that just reached us is up: stop backing off it.
                for id in node.inner.take_heard() {
                    wait.remove(&id);
                    due.remove(&id);
                }
                if let Err(e) = node.inner.refresh_inbox_devices().await {
                    tracing::warn!("reading inbox lists failed: {e}");
                }
                if let Err(e) = node.inner.refold_if_changed().await {
                    tracing::warn!("re-reading group state failed: {e}");
                }
                if let Err(e) = node.inner.renew_binding_if_due().await {
                    tracing::warn!("renewing our binding failed: {e}");
                }
                let now = tokio::time::Instant::now();
                for id in node.sync_targets() {
                    if busy.contains(&id) || due.get(&id).is_some_and(|d| now < *d) {
                        continue;
                    }
                    busy.insert(id);
                    let (node, done) = (node.clone(), done_tx.clone());
                    tokio::spawn(async move {
                        let res = tokio::time::timeout(
                            SYNC_TIMEOUT,
                            node.sync_with(EndpointAddr::new(id)),
                        )
                        .await;
                        let ok = match res {
                            Ok(Ok(stats)) => {
                                node.inner.reached_now(id);
                                if stats.fetched + stats.pushed > 0 {
                                    tracing::info!("synced with {}: {stats}", id.fmt_short());
                                }
                                true
                            }
                            Ok(Err(e)) => {
                                tracing::debug!("sync with {} failed: {e}", id.fmt_short());
                                false
                            }
                            Err(_) => {
                                tracing::debug!("sync with {} timed out", id.fmt_short());
                                false
                            }
                        };
                        let _ = done.send((id, ok));
                    });
                }
                node.inner.redial_gossip().await;
                tokio::time::sleep(every).await;
            }
        })
    }

    /// Stops accepting, leaves gossip, closes the endpoint.
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        self.inner.gossip.shutdown().await;
        for t in self.inner.tasks().drain(..) {
            t.abort();
        }
        self.inner.endpoint.close().await;
        self.inner.blobs.shutdown().await;
    }
}

trait Bech32OrHex {
    fn to_bech32_or_hex(&self) -> String;
}

impl Bech32OrHex for PublicKey {
    fn to_bech32_or_hex(&self) -> String {
        use nostr::prelude::ToBech32;
        self.to_bech32().unwrap_or_else(|_| self.to_hex())
    }
}

/// This data directory's device id, creating the key if there is none yet.
pub fn device_id(data_dir: &std::path::Path) -> Result<EndpointId, NodeError> {
    std::fs::create_dir_all(data_dir).map_err(|e| NodeError::DeviceKey(e.to_string()))?;
    Ok(device::load_or_create(&data_dir.join("device.key"))?.public())
}

/// Does an event of `kind` change its group's state — so it is judged and
/// stored under the fold lock, and the group refolded after? Not what is
/// said in a channel, and not presence.
fn changes_state(kind: nostr::prelude::Kind) -> bool {
    !group::CHANNEL_KINDS.contains(&kind)
        && kind != group::PRESENCE
        && kind != group::VOICE_PRESENCE
}

/// A protocol another crate serves on this node's endpoint (M9: voice):
/// handed each incoming connection on its ALPN, after the QUIC handshake.
pub type ProtocolFn =
    Arc<dyn Fn(iroh::endpoint::Connection) -> futures_lite_box::BoxFuture + Send + Sync>;

/// A boxed `Send` future, without another dependency.
pub mod futures_lite_box {
    pub type BoxFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
}

/// Every event the fold reads for `group`: the group key's state, the
/// owner's `9007`, and the actions.
async fn group_events(store: &Store, group: PublicKey) -> Result<Vec<Event>, StoreError> {
    let h = SingleLetterTag::LOWERCASE_H;
    let mut evs = store
        .query(
            &Filter::new()
                .kinds(FOLD_KINDS)
                .custom_tag(h, group.to_hex()),
        )
        .await?;
    evs.extend(
        store
            .query(
                &Filter::new()
                    .kinds(STATE_KINDS)
                    .author(group)
                    .identifier(group.to_hex()),
            )
            .await?,
    );
    Ok(evs)
}

async fn messages(store: &Store, group: &PublicKey) -> Result<Vec<Event>, StoreError> {
    let h = SingleLetterTag::LOWERCASE_H;
    let mut evs = store
        .query(
            &Filter::new()
                .kind(group::CHAT_MESSAGE)
                .custom_tag(h, group.to_hex()),
        )
        .await?;
    evs.reverse();
    Ok(evs)
}

/// A new group's first events: the group key's snapshot at `seq` 0 — one
/// `general` channel, HIT_ENTER_2's role ladder, the owner as its only
/// member — and the owner's own `9007`.
pub fn genesis(
    owner: &Person,
    key: &GroupKey,
    name: &str,
    public: bool,
    now: Timestamp,
) -> Result<Vec<Event>, NodeError> {
    let me = owner.public_key();
    let snap = Snapshot {
        seq: 0,
        name: name.to_owned(),
        public,
        // A public group's floor applies whatever this says.
        newcomer_wait: 0,
        channels: vec![Channel {
            id: "general".into(),
            name: "general".into(),
            kind: ChannelKind::Text,
            slowmode: 0,
        }],
        overrides: Vec::new(),
        roles: default_roles(),
        owner: Some(me),
        members: [(me, BTreeSet::new())].into(),
        bans: BTreeSet::new(),
        mutes: std::collections::BTreeMap::new(),
    };
    let mut evs: Vec<Event> = snap.sign(key, now)?.into();
    evs.push(owner.sign(group::create_group(&key.public_key()).custom_created_at(now))?);
    Ok(evs)
}

/// A compaction of the group `key` signs for: the state `store` folds to,
/// as a snapshot at `seq + 1`, **dated one second after every event of the
/// group it holds** — so nothing it folds is applied again (`hit-group`'s
/// state.rs). Returns the new `seq` and the four events, unstored.
pub async fn compaction(
    store: &Store,
    key: &GroupKey,
    me: PublicKey,
) -> Result<(u64, Vec<Event>), NodeError> {
    let g = key.public_key();
    let state = groups_in(store, me)
        .await?
        .state(&g)
        .cloned()
        .ok_or(NodeError::NoSuchGroup)?;
    let newest = group_events(store, g)
        .await?
        .iter()
        .chain(messages(store, &g).await?.iter())
        .map(|e| e.created_at)
        .max()
        .unwrap_or_default();
    let snap = state.to_snapshot();
    let at = Timestamp::now().max(newest + 1);
    Ok((snap.seq, snap.sign(key, at)?.into()))
}

/// The groups `store` carries, folded — what a node would hold for `me`.
pub async fn groups_in(store: &Store, me: PublicKey) -> Result<Groups, NodeError> {
    let mut groups = Groups::new(me);
    for g in store.carried_groups().await? {
        groups.carry(g);
        groups.set_state(g, GroupState::fold(g, &group_events(store, g).await?));
    }
    Ok(groups)
}

/// A media type from a file's extension, and nothing cleverer: nobody acts
/// on it but an icon and the decision to show a picture (HIT_ENTER_2).
fn guess_mime(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
    .to_owned()
}

/// The anchors an inbox list names, as npubs.
fn inbox_npubs(list: &Event) -> BTreeSet<PublicKey> {
    dm::inboxes_of(list).into_iter().map(|(p, _)| p).collect()
}

/// Who an invite link to `group` names (M6): the group's anchors' devices
/// first — they are there at 4am — then `me_device` (at `addr`, with relays
/// off); at most `MAX_PEERS`. From the store alone, so the CLI's `invite`
/// and a running node's say the same.
pub async fn invite_peers(
    store: &Store,
    me: PublicKey,
    group: &PublicKey,
    me_device: EndpointId,
    addr: Option<SocketAddr>,
) -> Result<Vec<(EndpointId, Option<SocketAddr>)>, NodeError> {
    let now = Timestamp::now();
    let mut book = BindingBook::new();
    for ev in store.query(&Filter::new().kind(DEVICE_BINDING)).await? {
        let _ = book.insert(ev, now);
    }
    let members: Vec<PublicKey> = groups_in(store, me)
        .await?
        .state(group)
        .map(|s| s.members().copied().collect())
        .unwrap_or_default();
    let mut peers: Vec<(EndpointId, Option<SocketAddr>)> = Vec::new();
    if !members.is_empty() {
        let profiles = store
            .query(
                &Filter::new()
                    .kind(hit_group::PROFILE)
                    .authors(members.iter().copied()),
            )
            .await?;
        for p in profiles.iter().filter(|p| hit_event::profile::is_anchor(p)) {
            for d in book.devices_of(&p.pubkey, now) {
                if d != me_device && !peers.iter().any(|(x, _)| *x == d) {
                    peers.push((d, None));
                }
            }
        }
    }
    peers.truncate(hit_proto::invite::MAX_PEERS - 1);
    peers.push((me_device, addr));
    Ok(peers)
}

/// Judges and stores `event` **without** a running endpoint — what
/// `hit-cli` does for anything that only signs (create, invite, say, kick)
/// while a node may be running in another process on the same store: two
/// endpoints on one device key fail silently (S5). The same `hit-group`
/// judgement as a running node's, fold included; the running node pushes
/// the event on its next sync.
pub async fn publish_offline(store: &Store, me: PublicKey, event: Event) -> Result<(), NodeError> {
    let verified = Verified::check(event).map_err(|e| NodeError::Protocol(e.to_string()))?;
    let now = Timestamp::now();
    match groups_in(store, me).await?.judge(&verified, now)? {
        Judgement::Store => {}
        Judgement::FoldWith(g) => {
            let mut evs = group_events(store, g).await?;
            evs.push((*verified).clone());
            if !GroupState::fold(g, &evs).is_some_and(|s| s.applied(&verified.id)) {
                return Err(Refusal::NotApplied.into());
            }
        }
    }
    store.insert(&verified).await?;
    Ok(())
}

impl Inner {
    fn own_binding(&self) -> std::sync::MutexGuard<'_, Event> {
        self.own_binding.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn book(&self) -> std::sync::MutexGuard<'_, BindingBook> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn groups(&self) -> std::sync::RwLockReadGuard<'_, Groups> {
        self.groups.read().unwrap_or_else(|e| e.into_inner())
    }

    fn groups_mut(&self) -> std::sync::RwLockWriteGuard<'_, Groups> {
        self.groups.write().unwrap_or_else(|e| e.into_inner())
    }

    fn hints(&self) -> std::sync::MutexGuard<'_, HashSet<EndpointId>> {
        self.hints.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `device` just completed a hello with us.
    pub(crate) fn heard_from(&self, device: EndpointId) {
        self.reached_now(device);
        self.heard
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(device);
    }

    fn reached_now(&self, device: EndpointId) {
        self.reached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(device, tokio::time::Instant::now());
    }

    fn take_heard(&self) -> HashSet<EndpointId> {
        std::mem::take(&mut *self.heard.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn tasks(&self) -> std::sync::MutexGuard<'_, Vec<JoinHandle<()>>> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps `binding`, a device binding of `person` that reached us outside
    /// any group — sealed in their DM, or from the hello of a device their
    /// contact link named. Checked as any binding is (signature, proof,
    /// expiry, the book's replacement rules), then stored, so it outlives
    /// a restart. `hit-group` would refuse it (no group in common); this is
    /// the one way such a binding is kept, and only one this node asked for
    /// or was handed by its own person's correspondent.
    pub(crate) async fn learn_binding(
        &self,
        binding: Event,
        person: PublicKey,
    ) -> Result<(), NodeError> {
        if binding.kind != DEVICE_BINDING || binding.pubkey != person {
            return Err(NodeError::Protocol("not a binding of that person".into()));
        }
        let verified = Verified::check(binding)
            .map_err(|_| NodeError::Protocol("binding: bad id or signature".into()))?;
        self.book()
            .insert((*verified).clone(), Timestamp::now())
            .map_err(|e| NodeError::Protocol(format!("binding: {e}")))?;
        // Stored even when the book already had it: a session's hello puts
        // a binding in the book (memory) without storing it, and the store
        // keeps only the newest per device anyway.
        self.store.insert(&verified).await?;
        Ok(())
    }

    /// Our own event: the same gate as anyone's. Refused is an error here.
    async fn publish(&self, ev: Event) -> Result<(), NodeError> {
        let (ok, why) = self.accept_event(ev, Source::Local).await?;
        if ok {
            Ok(())
        } else {
            Err(NodeError::Protocol(why))
        }
    }

    /// **The** way an event enters this node, from any direction: a peer's
    /// `EVENT`, a fetch during sync, gossip, or our own. Signature, then
    /// `hit-group` (for an action: folded with everything held, and kept
    /// only if the fold applies it), then — for bindings — the binding
    /// book's own checks, then the store, and only then the subscribers and
    /// gossip.
    pub(crate) async fn accept_event(
        &self,
        event: Event,
        source: Source,
    ) -> Result<Verdict, NodeError> {
        let Ok(verified) = Verified::check(event) else {
            return Ok((false, "invalid: bad id or signature".into()));
        };
        let now = Timestamp::now();
        let group = group_of(&verified);
        // Taken *before* judging, and held to the end: see `fold_lock`.
        let _write;
        let _read;
        if group.is_some() && changes_state(verified.kind) {
            _write = Some(self.fold_lock.write().await);
        } else if group.is_some() {
            _read = Some(self.fold_lock.read().await);
        }
        let mut judgement = self.groups().judge(&verified, now);
        // The sender's own outbox copy of a DM (M7): not addressed to us,
        // ours to keep until its recipient or their anchor has it.
        if source == Source::Local
            && verified.kind == dm::GIFT_WRAP
            && judgement == Err(Refusal::NotOurs)
        {
            judgement = Ok(Judgement::Store);
        }
        match judgement {
            Err(refusal) => return Ok((false, format!("blocked: {refusal}"))),
            Ok(Judgement::Store) => {}
            Ok(Judgement::FoldWith(g)) => {
                let mut evs = group_events(&self.store, g).await?;
                evs.push((*verified).clone());
                let applied = GroupState::fold(g, &evs).is_some_and(|s| s.applied(&verified.id));
                if !applied {
                    return Ok((false, format!("blocked: {}", Refusal::NotApplied)));
                }
            }
        }
        if verified.kind == DEVICE_BINDING
            && let Err(e) = self.book().insert((*verified).clone(), now)
        {
            return Ok((false, format!("invalid: {e}")));
        }
        if hit_group::DELETABLE_KINDS.contains(&verified.kind) {
            if self
                .store
                .is_deleted(&verified.id, &verified.pubkey)
                .await?
            {
                return Ok((false, "blocked: withdrawn by its author".into()));
            }
            if self.removed_by_moderator(&verified).await? {
                return Ok((false, "blocked: removed by a moderator".into()));
            }
        }
        if verified.kind == group::CHAT_MESSAGE
            && let Some(wait) = self.too_soon(&verified).await?
        {
            return Ok((false, format!("blocked: {}", Refusal::TooSoon(wait))));
        }
        let stored = self.store.insert(&verified).await?;
        if stored == Stored::New && verified.kind == group::DELETION {
            let gone = self
                .store
                .apply_deletion(&verified, &hit_group::DELETABLE_KINDS)
                .await?;
            self.tell_removed(gone);
        }
        if stored == Stored::New && verified.kind == group::DELETE_EVENT {
            self.apply_removal(&verified).await?;
        }
        if stored == Stored::New
            && verified.kind == dm::GIFT_WRAP
            && let Some(d) = self.person.open_dm(&verified)
        {
            // A DM file to us (M8b): the two people may be handed its bytes
            // — recorded now, since no group says so.
            if let Some(f) = &d.file {
                self.store.grant_blob(&f.hash, &d.from).await?;
                self.store.grant_blob(&f.hash, &d.to).await?;
            }
            // The sender's device, sealed in the rumor (M9): kept, so a
            // reply — or a call — reaches them with no group in common.
            if d.from != self.person.public_key()
                && let Some(b) = d.device
                && let Err(e) = self.learn_binding(b, d.from).await
            {
                tracing::debug!("a DM's device binding was not kept: {e}");
            }
        }
        if stored == Stored::New && verified.kind == dm::INBOX_LIST {
            self.groups_mut()
                .set_inboxes(verified.pubkey, inbox_npubs(&verified));
        }
        if stored == Stored::Ephemeral
            && verified.kind == group::VOICE_PRESENCE
            && let (Some(g), Some(c)) = (group, group::channel_of(&verified))
        {
            let mut voice = self.voice.lock().unwrap_or_else(|e| e.into_inner());
            let room = voice.entry((g, c.to_owned())).or_default();
            if group::voice_inside(&verified) {
                room.insert(verified.pubkey, tokio::time::Instant::now());
            } else {
                room.remove(&verified.pubkey);
            }
        }
        if stored == Stored::Ephemeral && verified.kind == group::PRESENCE {
            self.present
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(verified.pubkey, tokio::time::Instant::now());
        }
        if stored == Stored::New
            && let Some(g) = group
            && changes_state(verified.kind)
        {
            let state = GroupState::fold(g, &group_events(&self.store, g).await?);
            self.groups_mut().set_state(g, state);
            self.group_changed(g).await;
            self.purge_silenced(g).await?;
        }
        Ok(match stored {
            Stored::New => {
                let gossip_to = self.groups().gossip_group(&verified);
                let event = verified.into_event();
                // No subscribers is fine; the store is the record.
                let _ = self.events.send(event.clone());
                if matches!(source, Source::Local | Source::Push)
                    && let Some(g) = gossip_to
                {
                    self.gossip.broadcast(&g, &event).await;
                }
                (true, String::new())
            }
            Stored::Duplicate => (true, "duplicate: already have it".into()),
            Stored::Stale => (true, "duplicate: a newer version replaces it".into()),
            // Not stored, so never synced: told to subscribers and passed
            // on over gossip, and gone.
            Stored::Ephemeral => {
                let gossip_to = self.groups().gossip_group(&verified);
                let event = verified.into_event();
                let _ = self.events.send(event.clone());
                if matches!(source, Source::Local | Source::Push)
                    && let Some(g) = gossip_to
                {
                    self.gossip.broadcast(&g, &event).await;
                }
                (true, String::new())
            }
        })
    }

    /// Refolds every carried group whose state-changing events in the store
    /// changed since the last fold. Events this node accepts refold at once;
    /// this catches those another process wrote — the CLI signs `kick`,
    /// `ban`, `promote` into the store while `run` serves it, and without
    /// this the running node went on judging by the state before them
    /// (found by `scripts/m5-by-hand.sh`: a banned user's lies were taken
    /// by the very moderator who banned her).
    async fn refold_if_changed(&self) -> Result<(), NodeError> {
        let carried: Vec<PublicKey> = self.groups().carried().copied().collect();
        for g in carried {
            let _guard = self.fold_lock.write().await;
            let evs = group_events(&self.store, g).await?;
            let seen = self
                .folded
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&g)
                .copied();
            if seen == Some(evs.len()) {
                continue;
            }
            self.folded
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(g, evs.len());
            if seen.is_none() {
                // The first look after start: the start's own fold is current.
                continue;
            }
            self.groups_mut().set_state(g, GroupState::fold(g, &evs));
            self.group_changed(g).await;
            self.purge_silenced(g).await?;
        }
        Ok(())
    }

    /// The devices of every inbox anchor named by someone we share a group
    /// with: where a DM to them can wait (M7).
    async fn refresh_inbox_devices(&self) -> Result<(), NodeError> {
        let peers = self.groups().peers();
        if peers.is_empty() {
            return Ok(());
        }
        let lists = self
            .store
            .query(
                &Filter::new()
                    .kind(dm::INBOX_LIST)
                    .authors(peers.iter().copied()),
            )
            .await?;
        let devices: BTreeSet<EndpointId> = lists
            .iter()
            .flat_map(dm::inboxes_of)
            .filter_map(|(_, d)| d)
            .collect();
        *self.inbox_devices.lock().unwrap_or_else(|e| e.into_inner()) = devices;
        Ok(())
    }

    /// Has a moderator's `9005`, in the same group and channel, removed
    /// `ev`? Checked when it arrives, so a peer still holding it cannot push
    /// it back.
    async fn removed_by_moderator(&self, ev: &Verified) -> Result<bool, NodeError> {
        let removals = self.store.naming(&ev.id, group::DELETE_EVENT).await?;
        Ok(removals
            .iter()
            .any(|r| group_of(r) == group_of(ev) && group::channel_of(r) == group::channel_of(ev)))
    }

    /// Applies a stored `9005`: removes the targets held here that are
    /// messages or reactions **in its group and channel** — a moderator of
    /// one channel removes nothing in another.
    async fn apply_removal(&self, removal: &Verified) -> Result<(), NodeError> {
        let held = self.store.by_ids(&group::targets_of(removal)).await?;
        let gone: Vec<_> = held
            .iter()
            .filter(|e| {
                hit_group::DELETABLE_KINDS.contains(&e.kind)
                    && group_of(e) == group_of(removal)
                    && group::channel_of(e) == group::channel_of(removal)
            })
            .map(|e| e.id)
            .collect();
        self.store.remove(&gone).await?;
        self.tell_removed(gone);
        Ok(())
    }

    /// Slowmode (M5): the seconds `msg` came too soon after (or before)
    /// another message of its author in the same channel, if it did. Judged
    /// against the messages held here — so it stops honest clients and a
    /// burst, not a patched client that spaces its timestamps out
    /// (`PROTOCOL.md`).
    async fn too_soon(&self, msg: &Verified) -> Result<Option<u32>, NodeError> {
        let (Some(g), Some(channel)) = (group_of(msg), group::channel_of(msg)) else {
            return Ok(None);
        };
        let wait = self.groups().state(&g).map_or(0, |s| s.slowmode(channel));
        if wait == 0 {
            return Ok(None);
        }
        let span = u64::from(wait) - 1;
        let near = self
            .store
            .query(
                &Filter::new()
                    .kind(group::CHAT_MESSAGE)
                    .author(msg.pubkey)
                    .custom_tag(SingleLetterTag::LOWERCASE_H, g.to_hex())
                    .since(Timestamp::from(
                        msg.created_at.as_secs().saturating_sub(span),
                    ))
                    .until(msg.created_at + span),
            )
            .await?;
        Ok(near
            .iter()
            .any(|e| e.id != msg.id && group::channel_of(e) == Some(channel))
            .then_some(wait))
    }

    /// Drops what `group`'s silences say should not be here: a removed or
    /// muted person's messages and reactions dated inside their span, which
    /// reached us before the removal or mute did (`DESIGN.md` §6).
    async fn purge_silenced(&self, group: PublicKey) -> Result<(), NodeError> {
        let silences: Vec<hit_group::Silence> = self
            .groups()
            .state(&group)
            .map(|s| s.silences().to_vec())
            .unwrap_or_default();
        let mut gone = Vec::new();
        for s in silences {
            gone.extend(
                self.store
                    .authored_in(&group, &s.who, &hit_group::DELETABLE_KINDS, s.from, s.until)
                    .await?,
            );
        }
        if !gone.is_empty() {
            self.store.remove(&gone).await?;
            self.tell_removed(gone);
        }
        Ok(())
    }

    fn tell_removed(&self, ids: Vec<nostr::prelude::EventId>) {
        if !ids.is_empty() {
            let _ = self.removed.send(ids);
        }
    }

    /// After `group`'s state changed: join its gossip if we are now a
    /// member, leave it if we are not, and close live connections of
    /// devices whose person is no longer a member.
    async fn group_changed(&self, group: PublicKey) {
        let me = self.person.public_key();
        let member = self
            .groups()
            .state(&group)
            .is_some_and(|s| s.is_member(&me));
        if member && !self.gossip.has(&group) {
            let peers = self.member_devices(&group);
            match self
                .gossip
                .start(&self.endpoint, group, peers, self.intake.clone())
                .await
            {
                Ok(()) => self.update_alpns(),
                Err(e) => tracing::warn!("joining gossip failed: {e}"),
            }
        } else if !member && self.gossip.has(&group) {
            self.gossip.stop(&group).await;
            self.update_alpns();
        }
        self.gossip
            .revoke(&group, |device| self.device_is_member(device, &group));
    }

    fn update_alpns(&self) {
        let mut alpns = vec![ALPN.to_vec(), iroh_blobs::ALPN.to_vec()];
        alpns.extend(
            self.protocols
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .keys()
                .cloned(),
        );
        alpns.extend(self.gossip.alpns());
        self.endpoint.set_alpns(alpns);
    }

    /// Does `device` speak for a member of `group`? Through the binding
    /// book: the device proved its id in the handshake, the binding proves
    /// the person.
    pub(crate) fn device_is_member(&self, device: EndpointId, group: &PublicKey) -> bool {
        let persons = self.book().persons_of(device, Timestamp::now());
        let groups = self.groups();
        groups
            .state(group)
            .is_some_and(|s| persons.iter().any(|p| s.is_member(p)))
    }

    /// The devices of `group`'s other members.
    fn member_devices(&self, group: &PublicKey) -> Vec<EndpointId> {
        let me = self.endpoint.id();
        let members: Vec<PublicKey> = self
            .groups()
            .state(group)
            .map(|s| s.members().copied().collect())
            .unwrap_or_default();
        let now = Timestamp::now();
        let book = self.book();
        members
            .iter()
            .flat_map(|p| book.devices_of(p, now))
            .filter(|d| *d != me)
            .collect()
    }

    /// An incoming gossip connection, already admitted by the gate hook.
    pub(crate) async fn gossip_accept(&self, group: PublicKey, conn: iroh::endpoint::Connection) {
        self.gossip.admit(&group, conn).await;
    }

    async fn redial_gossip(&self) {
        let groups: Vec<PublicKey> = self.groups().carried().copied().collect();
        for g in groups {
            let peers = self.member_devices(&g);
            self.gossip.join_peers(&g, peers).await;
        }
    }

    fn sync_targets(&self) -> Vec<EndpointId> {
        let me = self.endpoint.id();
        let now = Timestamp::now();
        let people = self.groups().peers();
        let mut out: BTreeSet<EndpointId> = {
            let book = self.book();
            people
                .iter()
                .flat_map(|p| book.devices_of(p, now))
                .collect()
        };
        out.extend(self.hints().iter().copied());
        out.extend(
            self.inbox_devices
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .copied(),
        );
        out.remove(&me);
        out.into_iter().collect()
    }

    /// Authenticates a peer's hello: binding valid for the connection's
    /// device and not revoked. **Who they are, not what they may do**: a
    /// stranger gets a session in which every request is judged — and finds
    /// nothing to read, and nothing it sends stored, unless it brings a join
    /// an invite honours.
    pub(crate) async fn authenticate(
        &self,
        presented: &Event,
        conn: EndpointId,
    ) -> Result<PublicKey, NodeError> {
        let person = self
            .book()
            .authenticate(presented, conn, Timestamp::now())?;
        // A member's binding is worth keeping: it is how others find them.
        // A stranger's is refused by `hit-group`, and that is fine.
        self.accept_event(presented.clone(), Source::Fetch).await?;
        Ok(person)
    }

    /// Everything `reader` may see that matches `filter` — the one place a
    /// `REQ` answer and a negentropy set come from.
    pub(crate) async fn visible(
        &self,
        reader: &PublicKey,
        filter: &Filter,
    ) -> Result<Vec<Event>, NodeError> {
        let evs = self.store.query(filter).await?;
        let now = Timestamp::now();
        let groups = self.groups();
        Ok(evs
            .into_iter()
            .filter(|e| groups.may_read(reader, e, now))
            .collect())
    }

    /// The set a negentropy session with `peer` reconciles over: what it
    /// may see, **minus the two parties' own bindings** — those were the
    /// session's first frames, both ways, and offering them again would
    /// only be refused (a stranger's) or fetched twice (a joiner's) — **and
    /// minus expired bindings**, which every peer's book refuses: a member
    /// offline past their binding's expiry was re-offered every round
    /// (the VPS anchor, 2026-10-24).
    pub(crate) async fn reconcilable(
        &self,
        peer: &PublicKey,
        filter: &Filter,
    ) -> Result<Vec<Event>, NodeError> {
        let me = self.person.public_key();
        let now = Timestamp::now();
        let mut evs = self.visible(peer, filter).await?;
        evs.retain(|e| {
            e.kind != DEVICE_BINDING
                || (e.pubkey != me
                    && e.pubkey != *peer
                    && e.tags.expiration().is_some_and(|exp| exp > now))
        });
        Ok(evs)
    }

    async fn renew_binding_if_due(&self) -> Result<(), NodeError> {
        let now = Timestamp::now();
        if !needs_renewal(&self.own_binding(), now) {
            return Ok(());
        }
        let fresh = sign_binding(
            &self.person,
            &self.device,
            now,
            hit_event::binding::DEFAULT_TTL,
        )?;
        self.publish(fresh.clone()).await?;
        *self.own_binding() = fresh;
        Ok(())
    }
}

/// Our binding for this device: the stored one if still good, else a fresh
/// one. A *revocation* for this device stops the node — re-binding a device
/// the person revoked must be a deliberate act, never an automatic one.
async fn own_binding(
    person: &Person,
    device: &DeviceKey,
    store: &Store,
    book: &mut BindingBook,
    now: Timestamp,
) -> Result<Event, NodeError> {
    let mine = store
        .query(
            &Filter::new()
                .kind(DEVICE_BINDING)
                .author(person.public_key())
                .identifier(device.public().to_string()),
        )
        .await?;
    if let Some(current) = mine.first() {
        if is_revoked(current) {
            return Err(NodeError::DeviceRevoked);
        }
        if !needs_renewal(current, now) && verify_binding_at(current, device.public(), now).is_ok()
        {
            return Ok(current.clone());
        }
    }
    let fresh = sign_binding(person, device, now, hit_event::binding::DEFAULT_TTL)?;
    let verified =
        Verified::check(fresh.clone()).map_err(|e| NodeError::Protocol(e.to_string()))?;
    book.insert(fresh.clone(), now)?;
    store.insert(&verified).await?;
    Ok(fresh)
}
