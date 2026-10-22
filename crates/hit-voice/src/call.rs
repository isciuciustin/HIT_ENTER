//! A call: this node with a QUIC connection to every other person in it —
//! a voice channel (a mesh, at most [`MAX_VOICE`]), or a 1:1 call from a DM.
//!
//! **A voice channel:**
//! - **Who is in it** comes from voice presence on the group's gossip
//!   (`hit_node::Node::voice_participants`); this node announces itself
//!   every few seconds while in.
//! - **Who dials whom**: of each pair, the device with the lower id. Never
//!   two connections for one pair, and nobody waits for a coordinator.
//! - **The gate** is the listener's: the dialling device must speak for a
//!   member who may *view* the voice channel; the call must have room. Their
//!   audio is played only if they may also *speak* there (`SEND_MESSAGES`,
//!   and not muted by a moderator) — checked at the join and every second.
//!
//! **A 1:1 call** has no presence and no group:
//! - **The caller dials every device** the callee has a binding for, with
//!   its own binding in the join; the first device to pick up wins and the
//!   others stop ringing (their connections are dropped).
//! - **The callee's gate**: the binding must name the connection's device
//!   and not be revoked; the caller must be someone this person shares a
//!   group with or has written to in a DM — a stranger cannot make anyone's
//!   app ring. Busy if already in a call or ringing.
//! - **It rings** for [`RING`], until the person answers or declines, or
//!   the caller gives up. When either side hangs up, the call ends.
//!
//! **Every 20 ms**, either way: a frame from the [`Input`] (if this person
//! is heard — not muted, and speaking or holding push-to-talk) is encoded
//! and sent to every peer as a datagram; and a frame from each peer's jitter
//! buffer is decoded, mixed and handed to the [`Output`] (unless deafened).

use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use hit_event::group::{ChannelKind, MAX_VOICE};
use hit_group::Permissions;
use hit_node::{Node, NodeError};
use iroh::{EndpointAddr, EndpointId, endpoint::Connection};
use nostr::prelude::{PublicKey, Timestamp};
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
    task::JoinHandle,
};

use crate::{
    codec::{Dec, Enc, FRAME, level},
    jitter::{Jitter, Slot},
    wire::{ALPN, Answer, Join, MAX_JOIN, Packet},
};

/// Where audio comes from: one 20 ms frame ([`FRAME`] samples, 48 kHz mono)
/// per call, if one is ready — the microphone, or a test's tone.
pub trait Input: Send + 'static {
    fn frame(&mut self) -> Option<Vec<f32>>;
}

/// Where the call's mix goes: one 20 ms frame per tick.
pub trait Output: Send + 'static {
    fn play(&mut self, frame: Vec<f32>);
}

/// How long a 1:1 call rings before the caller is told nobody answered.
pub const RING: Duration = Duration::from_secs(45);
/// How long the window is told why a call ended.
const ENDED_SHOWN: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The most people in a call, this one included.
    pub max: usize,
    /// Jitter buffer delay, in 20 ms frames.
    pub delay: usize,
    /// How often to announce being in the call.
    pub heartbeat: Duration,
    /// Someone whose "in" is older than this has left.
    pub presence_window: Duration,
    /// Voice activation: a frame louder than this is speech.
    pub vad_threshold: f32,
    /// How long a 1:1 call rings.
    pub ring: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max: MAX_VOICE,
            delay: 3,
            heartbeat: Duration::from_secs(5),
            presence_window: Duration::from_secs(15),
            vad_threshold: 0.02,
            ring: RING,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error("{0}")]
    Codec(#[from] crate::codec::CodecError),
    #[error("not a voice channel this person can join")]
    NotAllowed,
    #[error("the channel is full ({0} people)")]
    Full(usize),
    #[error("already in a call")]
    Busy,
    #[error("not in a call")]
    NotInCall,
    #[error("no device of theirs is known here — a group in common makes them reachable")]
    NoDevice,
    #[error("nobody is calling")]
    NotRinging,
}

/// What the window shows about a call.
#[derive(Debug, Clone, Serialize)]
pub struct CallState {
    /// A voice channel: the group's npub and the channel's id.
    pub group: Option<String>,
    pub channel: Option<String>,
    /// A 1:1 call: the other person's npub.
    pub with: Option<String>,
    /// A 1:1 call this person made, not answered yet.
    pub calling: bool,
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    /// May this person be heard here (`SEND_MESSAGES`, not muted)?
    pub can_speak: bool,
    pub me_speaking: bool,
    pub people: Vec<Participant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Participant {
    pub npub: String,
    /// A connection to them is up.
    pub connected: bool,
    pub speaking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Room {
    Channel { group: PublicKey, channel: String },
    Dm { with: PublicKey },
}

struct Peer {
    conn: Connection,
    jitter: Jitter,
    dec: Dec,
    /// May they be heard (checked every second).
    heard: bool,
    /// Last time a frame of theirs had speech in it.
    speaking_at: Option<tokio::time::Instant>,
}

struct Call {
    room: Room,
    peers: Mutex<HashMap<PublicKey, Peer>>,
    /// A 1:1 call: the other side picked up (or this side did).
    answered: AtomicBool,
    muted: AtomicBool,
    deafened: AtomicBool,
    push_to_talk: AtomicBool,
    talk_key: AtomicBool,
    can_speak: AtomicBool,
    speaking_at: Mutex<Option<tokio::time::Instant>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Call {
    fn new(room: Room, can_speak: bool) -> Arc<Self> {
        Arc::new(Self {
            room,
            peers: Mutex::new(HashMap::new()),
            answered: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            push_to_talk: AtomicBool::new(false),
            talk_key: AtomicBool::new(false),
            can_speak: AtomicBool::new(can_speak),
            speaking_at: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
        })
    }

    fn peers(&self) -> std::sync::MutexGuard<'_, HashMap<PublicKey, Peer>> {
        self.peers.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tasks(&self) -> std::sync::MutexGuard<'_, Vec<JoinHandle<()>>> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops everything and hangs up on everyone.
    fn stop(&self, why: &[u8]) {
        for t in self.tasks().drain(..) {
            t.abort();
        }
        for (_, p) in self.peers().drain() {
            p.conn.close(0u32.into(), why);
        }
    }
}

/// A 1:1 call ringing here.
struct Ringing {
    id: u64,
    from: PublicKey,
    decide: oneshot::Sender<bool>,
}

/// Voice for one node: at most one call at a time.
pub struct Voice {
    node: Node,
    config: Config,
    call: Mutex<Option<Arc<Call>>>,
    ringing: Mutex<Option<Ringing>>,
    rings: AtomicU64,
    /// Why the last call ended, and when — for the window, briefly.
    ended: Mutex<Option<(String, tokio::time::Instant)>>,
}

/// A speaking indicator holds for this long after the last loud frame.
const HANGOVER: Duration = Duration::from_millis(300);

impl Voice {
    /// Voice on `node`: serves `hit/voice/1` from now on.
    pub fn new(node: Node, config: Config) -> Arc<Self> {
        let voice = Arc::new(Self {
            node: node.clone(),
            config,
            call: Mutex::new(None),
            ringing: Mutex::new(None),
            rings: AtomicU64::new(0),
            ended: Mutex::new(None),
        });
        let weak: Weak<Self> = Arc::downgrade(&voice);
        node.serve_protocol(
            ALPN,
            Arc::new(move |conn| {
                let weak = weak.clone();
                Box::pin(async move {
                    if let Some(v) = weak.upgrade() {
                        v.accept(conn).await;
                    }
                })
            }),
        );
        voice
    }

    fn current(&self) -> Option<Arc<Call>> {
        self.call.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn ringing(&self) -> std::sync::MutexGuard<'_, Option<Ringing>> {
        self.ringing.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// May `person` join (`VIEW_CHANNEL`) — and be heard (`SEND_MESSAGES`,
    /// not muted)? `None` if they may not join at all.
    fn rights(&self, group: &PublicKey, channel: &str, person: &PublicKey) -> Option<bool> {
        let s = self.node.group(group)?;
        let c = s.channels().iter().find(|c| c.id == channel)?;
        if c.kind != ChannelKind::Voice || !s.is_member(person) {
            return None;
        }
        let perms = s.effective(person, channel);
        if !perms.contains(Permissions::VIEW_CHANNEL) {
            return None;
        }
        let muted = s.muted_until(person, Timestamp::now()).is_some();
        Some(perms.contains(Permissions::SEND_MESSAGES) && !muted)
    }

    /// Who may be heard in `room`: in a 1:1 call, the other person, always.
    fn heard(&self, room: &Room, person: &PublicKey) -> Option<bool> {
        match room {
            Room::Channel { group, channel } => self.rights(group, channel, person),
            Room::Dm { with } => Some(with == person),
        }
    }

    /// May `person` make this app ring? Someone this person shares a group
    /// with, or has written to in a DM. Not a stranger who learned a device
    /// id, and not someone who merely wrote to them.
    pub async fn may_call(&self, person: &PublicKey) -> bool {
        if *person == self.node.person() {
            return false;
        }
        if self.node.shares_group(person) {
            return true;
        }
        let me = self.node.person();
        self.node
            .dms()
            .await
            .is_ok_and(|dms| dms.iter().any(|d| d.from == me && d.to == *person))
    }

    /// Starts `call`'s loops and makes it the current one.
    fn start(
        self: &Arc<Self>,
        call: &Arc<Call>,
        input: Box<dyn Input>,
        output: Box<dyn Output>,
    ) -> Result<(), VoiceError> {
        let enc = Enc::new()?;
        *self.call.lock().unwrap_or_else(|e| e.into_inner()) = Some(call.clone());
        *self.ended.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let tick = tokio::spawn(audio_loop(self.clone(), call.clone(), input, output, enc));
        let keep = tokio::spawn(upkeep(self.clone(), call.clone()));
        call.tasks().extend([tick, keep]);
        Ok(())
    }

    /// Ends `call` if it is still the current one, saying why.
    fn end(&self, call: &Arc<Call>, why: &str) {
        {
            let mut cur = self.call.lock().unwrap_or_else(|e| e.into_inner());
            if !cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, call)) {
                return;
            }
            *cur = None;
        }
        *self.ended.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((why.to_owned(), tokio::time::Instant::now()));
        call.stop(b"ended");
    }

    /// Joins voice channel `channel` of `group`, with audio from `input`
    /// and to `output`.
    pub async fn join(
        self: &Arc<Self>,
        group: PublicKey,
        channel: &str,
        input: Box<dyn Input>,
        output: Box<dyn Output>,
    ) -> Result<(), VoiceError> {
        if self.current().is_some() {
            return Err(VoiceError::Busy);
        }
        let me = self.node.person();
        let can_speak = self
            .rights(&group, channel, &me)
            .ok_or(VoiceError::NotAllowed)?;
        let others = self
            .node
            .voice_participants(&group, channel, self.config.presence_window)
            .into_iter()
            .filter(|p| *p != me)
            .count();
        if others + 1 > self.config.max {
            return Err(VoiceError::Full(self.config.max));
        }
        self.node.announce_voice(&group, channel, true).await?;
        let call = Call::new(
            Room::Channel {
                group,
                channel: channel.to_owned(),
            },
            can_speak,
        );
        self.start(&call, input, output)
    }

    /// Calls `person` (a 1:1 call): rings every device of theirs known
    /// here. Returns once ringing; [`Voice::state`] says when they answer,
    /// and why it ended if it did.
    pub async fn call(
        self: &Arc<Self>,
        person: PublicKey,
        input: Box<dyn Input>,
        output: Box<dyn Output>,
    ) -> Result<(), VoiceError> {
        if self.current().is_some() {
            return Err(VoiceError::Busy);
        }
        let devices = self.node.devices_of(&person);
        if devices.is_empty() || person == self.node.person() {
            return Err(VoiceError::NoDevice);
        }
        let call = Call::new(Room::Dm { with: person }, true);
        self.start(&call, input, output)?;
        let dialling = tokio::spawn(ring_out(self.clone(), call.clone(), person, devices));
        call.tasks().push(dialling);
        Ok(())
    }

    /// Who is calling, if a call rings here.
    pub fn incoming(&self) -> Option<PublicKey> {
        self.ringing().as_ref().map(|r| r.from)
    }

    /// Picks up the call ringing here.
    pub async fn answer(
        self: &Arc<Self>,
        input: Box<dyn Input>,
        output: Box<dyn Output>,
    ) -> Result<(), VoiceError> {
        if self.current().is_some() {
            return Err(VoiceError::Busy);
        }
        let ring = self.ringing().take().ok_or(VoiceError::NotRinging)?;
        let call = Call::new(Room::Dm { with: ring.from }, true);
        self.start(&call, input, output)?;
        if ring.decide.send(true).is_err() {
            // They hung up between the ring and the click.
            self.end(&call, "they hung up");
        }
        Ok(())
    }

    /// Turns the call ringing here away.
    pub fn decline(&self) -> Result<(), VoiceError> {
        let ring = self.ringing().take().ok_or(VoiceError::NotRinging)?;
        let _ = ring.decide.send(false);
        Ok(())
    }

    /// Leaves the call — or hangs up a 1:1 call, ringing or not.
    pub async fn leave(&self) -> Result<(), VoiceError> {
        let call = self
            .call
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or(VoiceError::NotInCall)?;
        call.stop(b"left");
        if let Room::Channel { group, channel } = &call.room {
            self.node.announce_voice(group, channel, false).await?;
        }
        Ok(())
    }

    pub fn set_muted(&self, on: bool) {
        if let Some(c) = self.current() {
            c.muted.store(on, Ordering::Relaxed);
        }
    }

    pub fn set_deafened(&self, on: bool) {
        if let Some(c) = self.current() {
            c.deafened.store(on, Ordering::Relaxed);
        }
    }

    /// Push-to-talk on (only the talk key sends) or off (voice activation).
    pub fn set_push_to_talk(&self, on: bool) {
        if let Some(c) = self.current() {
            c.push_to_talk.store(on, Ordering::Relaxed);
        }
    }

    /// The talk key, down or up.
    pub fn talk(&self, down: bool) {
        if let Some(c) = self.current() {
            c.talk_key.store(down, Ordering::Relaxed);
        }
    }

    /// Why the last call ended, for a few seconds after it did.
    pub fn ended(&self) -> Option<String> {
        self.ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(_, at)| at.elapsed() < ENDED_SHOWN)
            .map(|(why, _)| why.clone())
    }

    pub fn state(&self) -> Option<CallState> {
        let c = self.current()?;
        let now = tokio::time::Instant::now();
        let recent = |at: Option<tokio::time::Instant>| at.is_some_and(|t| now - t < HANGOVER);
        let me = self.node.person();
        let in_room: BTreeSet<PublicKey> = match &c.room {
            Room::Channel { group, channel } => {
                self.node
                    .voice_participants(group, channel, self.config.presence_window)
            }
            Room::Dm { with } => [*with].into(),
        };
        let peers = c.peers();
        let mut people: Vec<Participant> = in_room
            .iter()
            .filter(|p| **p != me)
            .map(|p| Participant {
                npub: npub(p),
                connected: peers.contains_key(p),
                speaking: peers.get(p).is_some_and(|x| recent(x.speaking_at)),
            })
            .collect();
        people.sort_by(|a, b| a.npub.cmp(&b.npub));
        let (group, channel, with) = match &c.room {
            Room::Channel { group, channel } => (Some(npub(group)), Some(channel.clone()), None),
            Room::Dm { with } => (None, None, Some(npub(with))),
        };
        Some(CallState {
            calling: with.is_some() && !c.answered.load(Ordering::Relaxed),
            group,
            channel,
            with,
            muted: c.muted.load(Ordering::Relaxed),
            deafened: c.deafened.load(Ordering::Relaxed),
            push_to_talk: c.push_to_talk.load(Ordering::Relaxed),
            can_speak: c.can_speak.load(Ordering::Relaxed),
            me_speaking: recent(*c.speaking_at.lock().unwrap_or_else(|e| e.into_inner())),
            people,
        })
    }

    /// The people in a voice channel, for the channel list — whether or not
    /// this node is in it.
    pub fn participants(&self, group: &PublicKey, channel: &str) -> BTreeSet<PublicKey> {
        self.node
            .voice_participants(group, channel, self.config.presence_window)
    }

    /// An incoming `hit/voice/1` connection: the gate, then the peer.
    async fn accept(self: Arc<Self>, conn: Connection) {
        let device = conn.remote_id();
        let (mut send, recv) = match conn.accept_bi().await {
            Ok(s) => s,
            Err(_) => return,
        };
        let outcome = match read_line::<Join>(recv, Duration::from_secs(10)).await {
            None => Err("no join".to_owned()),
            Some(Join::Channel { group, channel }) => self.admit(device, &group, &channel),
            Some(Join::Dm { dm }) => self.ring_in(&conn, device, &dm).await,
        };
        match outcome {
            Ok((call, person, heard)) => {
                let ok = Answer {
                    ok: true,
                    why: None,
                };
                if write_line(&mut send, &ok).await.is_err() {
                    if matches!(call.room, Room::Dm { .. }) {
                        self.end(&call, "they hung up");
                    }
                    return;
                }
                self.add_peer(&call, person, heard, conn);
                // After the peer is in: the upkeep ends an answered 1:1 call
                // with nobody in it.
                call.answered.store(true, Ordering::Relaxed);
            }
            Err(why) => {
                let no = Answer {
                    ok: false,
                    why: Some(why.clone()),
                };
                let _ = write_line(&mut send, &no).await;
                let _ = send.finish();
                let _ = send.stopped().await;
                tracing::debug!("voice from {} refused: {why}", device.fmt_short());
                conn.close(0x10u32.into(), why.as_bytes());
            }
        }
    }

    /// The gate for a voice channel.
    fn admit(
        &self,
        device: EndpointId,
        group: &str,
        channel: &str,
    ) -> Result<(Arc<Call>, PublicKey, bool), String> {
        let call = self.current().ok_or("not in a call")?;
        let Room::Channel {
            group: g,
            channel: c,
        } = &call.room
        else {
            return Err("not in that call".into());
        };
        if *group != g.to_hex() || channel != c {
            return Err("not in that call".into());
        }
        let me = self.node.person();
        let person = self
            .node
            .persons_of(device)
            .into_iter()
            .find(|p| *p != me && self.rights(g, c, p).is_some())
            .ok_or("not a member who may join this channel")?;
        let heard = self.rights(g, c, &person).unwrap_or(false);
        let peers = call.peers().len();
        if !call.peers().contains_key(&person) && peers + 1 >= self.config.max {
            return Err(format!("the channel is full ({} people)", self.config.max));
        }
        Ok((call, person, heard))
    }

    /// The gate for a 1:1 call, then the ring: answers when the person does.
    async fn ring_in(
        &self,
        conn: &Connection,
        device: EndpointId,
        binding: &nostr::prelude::Event,
    ) -> Result<(Arc<Call>, PublicKey, bool), String> {
        let from = self
            .node
            .authenticate(binding, device)
            .map_err(|_| "who are you? the binding does not hold".to_owned())?;
        if !self.may_call(&from).await {
            return Err("not someone who may call this person".into());
        }
        let (decide, decided) = oneshot::channel();
        let id = self.rings.fetch_add(1, Ordering::Relaxed);
        {
            let mut ring = self.ringing();
            if self.current().is_some() || ring.is_some() {
                return Err("busy".into());
            }
            *ring = Some(Ringing { id, from, decide });
        }
        let answer = tokio::select! {
            d = decided => d.ok(),
            _ = conn.closed() => None,
            _ = tokio::time::sleep(self.config.ring) => None,
        };
        // Stop ringing, unless the person already took this one.
        {
            let mut ring = self.ringing();
            if ring.as_ref().is_some_and(|r| r.id == id) {
                *ring = None;
            }
        }
        match answer {
            Some(true) => {
                let call = self.current().ok_or("hung up")?;
                if call.room != (Room::Dm { with: from }) {
                    return Err("busy".into());
                }
                Ok((call, from, true))
            }
            Some(false) => Err("declined".into()),
            None => Err("no answer".into()),
        }
    }

    fn add_peer(&self, call: &Arc<Call>, person: PublicKey, heard: bool, conn: Connection) {
        let dec = match Dec::new() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("no decoder: {e}");
                return;
            }
        };
        let peer = Peer {
            conn: conn.clone(),
            jitter: Jitter::new(self.config.delay),
            dec,
            heard,
            speaking_at: None,
        };
        if let Some(old) = call.peers().insert(person, peer) {
            old.conn.close(0u32.into(), b"replaced");
        }
        let c = call.clone();
        let reader = tokio::spawn(async move {
            while let Ok(d) = conn.read_datagram().await {
                let Some(p) = Packet::decode(&d) else {
                    continue;
                };
                let mut peers = c.peers();
                if let Some(peer) = peers.get_mut(&person)
                    && peer.heard
                {
                    peer.jitter.push(p.seq, p.opus);
                }
            }
            c.peers().remove(&person);
        });
        call.tasks().push(reader);
    }

    /// Dials `device` (of `person`) into a voice channel call.
    async fn dial(self: &Arc<Self>, call: Arc<Call>, person: PublicKey, device: EndpointId) {
        let Room::Channel { group, channel } = &call.room else {
            return;
        };
        let join = Join::Channel {
            group: group.to_hex(),
            channel: channel.clone(),
        };
        if let Ok((conn, Answer { ok: true, .. })) =
            self.knock(device, &join, Duration::from_secs(10)).await
        {
            let heard = self.rights(group, channel, &person).unwrap_or(false);
            self.add_peer(&call, person, heard, conn);
        }
    }

    /// Connects to `device`, sends `join`, waits up to `patience` for the
    /// answer.
    async fn knock(
        &self,
        device: EndpointId,
        join: &Join,
        patience: Duration,
    ) -> Result<(Connection, Answer), String> {
        let conn = tokio::time::timeout(
            Duration::from_secs(10),
            self.node
                .endpoint()
                .connect(EndpointAddr::new(device), ALPN),
        )
        .await
        .map_err(|_| "unreachable".to_owned())?
        .map_err(|_| "unreachable".to_owned())?;
        let (mut send, recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
        write_line(&mut send, join)
            .await
            .map_err(|e| e.to_string())?;
        let answer = read_line::<Answer>(recv, patience)
            .await
            .ok_or("no answer")?;
        if !answer.ok {
            tracing::debug!("voice to {} refused: {:?}", device.fmt_short(), answer.why);
        }
        Ok((conn, answer))
    }
}

/// Rings every device of `person` at once; the first to pick up is the
/// call, and the rest are dropped — which stops them ringing.
async fn ring_out(voice: Arc<Voice>, call: Arc<Call>, person: PublicKey, devices: Vec<EndpointId>) {
    let join = Join::Dm {
        dm: voice.node.binding(),
    };
    let patience = voice.config.ring + Duration::from_secs(5);
    let mut set = tokio::task::JoinSet::new();
    for d in devices {
        let (v, j) = (voice.clone(), join.clone());
        set.spawn(async move { v.knock(d, &j, patience).await });
    }
    let mut why: Option<String> = None;
    while let Some(done) = set.join_next().await {
        match done {
            Ok(Ok((conn, Answer { ok: true, .. }))) => {
                set.abort_all();
                voice.add_peer(&call, person, true, conn);
                call.answered.store(true, Ordering::Relaxed);
                return;
            }
            Ok(Ok((_, Answer { why: w, .. }))) => {
                // A refusal says more than silence: "declined" over "busy"
                // over "no answer".
                let w = w.unwrap_or_else(|| "refused".into());
                let rank = |s: &str| match s {
                    "declined" => 3,
                    "busy" => 2,
                    "no answer" | "unreachable" => 0,
                    _ => 1,
                };
                if why.as_deref().is_none_or(|old| rank(&w) > rank(old)) {
                    why = Some(w);
                }
            }
            Ok(Err(w)) => {
                if why.is_none() {
                    why = Some(w);
                }
            }
            Err(_) => {}
        }
    }
    voice.end(&call, why.as_deref().unwrap_or("no answer"));
}

fn npub(p: &PublicKey) -> String {
    use nostr::prelude::ToBech32;
    p.to_bech32().unwrap_or_else(|_| p.to_hex())
}

async fn write_line<T: Serialize>(
    send: &mut iroh::endpoint::SendStream,
    v: &T,
) -> Result<(), std::io::Error> {
    let mut line = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    line.push(b'\n');
    send.write_all(&line).await.map_err(std::io::Error::other)?;
    send.flush().await.map_err(std::io::Error::other)
}

async fn read_line<T: serde::de::DeserializeOwned>(
    recv: iroh::endpoint::RecvStream,
    patience: Duration,
) -> Option<T> {
    let mut reader = BufReader::new(recv).take(MAX_JOIN as u64);
    let mut line = String::new();
    tokio::time::timeout(patience, reader.read_line(&mut line))
        .await
        .ok()?
        .ok()?;
    serde_json::from_str(line.trim()).ok()
}

/// Every second. A voice channel: announce (every heartbeat), dial who
/// should be dialled, drop who has left, and re-check who may be heard. A
/// 1:1 call: once answered, it ends when the other side goes.
async fn upkeep(voice: Arc<Voice>, call: Arc<Call>) {
    let me = voice.node.person();
    let my_device = voice.node.id();
    let mut last_beat = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (group, channel) = match &call.room {
            Room::Dm { .. } => {
                if call.answered.load(Ordering::Relaxed) && call.peers().is_empty() {
                    voice.end(&call, "they hung up");
                    return;
                }
                continue;
            }
            Room::Channel { group, channel } => (group, channel),
        };
        if last_beat.elapsed() >= voice.config.heartbeat {
            last_beat = tokio::time::Instant::now();
            let _ = voice.node.announce_voice(group, channel, true).await;
        }
        let room = voice.participants(group, channel);
        // Gone from the room: hang up.
        call.peers().retain(|p, peer| {
            let stay = room.contains(p);
            if !stay {
                peer.conn.close(0u32.into(), b"left");
            }
            stay
        });
        // Who may be heard, again: a mute or a ban mid-call takes effect.
        call.can_speak.store(
            voice.rights(group, channel, &me).unwrap_or(false),
            Ordering::Relaxed,
        );
        for (p, peer) in call.peers().iter_mut() {
            peer.heard = voice.heard(&call.room, p).unwrap_or(false);
        }
        // Dial whom we should: the lower device id of each pair dials.
        let connected: BTreeSet<PublicKey> = call.peers().keys().copied().collect();
        for p in room.iter().filter(|p| **p != me && !connected.contains(*p)) {
            if voice.rights(group, channel, p).is_none() {
                continue;
            }
            for d in voice.node.devices_of(p) {
                if my_device < d {
                    let (v, c, person) = (voice.clone(), call.clone(), *p);
                    tokio::spawn(async move { v.dial(c, person, d).await });
                }
            }
        }
    }
}

/// The 20 ms loop: send this person's frame, play everyone else's.
async fn audio_loop(
    voice: Arc<Voice>,
    call: Arc<Call>,
    mut input: Box<dyn Input>,
    mut output: Box<dyn Output>,
    mut enc: Enc,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seq: u16 = 0;
    let threshold = voice.config.vad_threshold;
    loop {
        tick.tick().await;
        // Out.
        if let Some(frame) = input.frame() {
            let loud = level(&frame) > threshold;
            if loud {
                *call.speaking_at.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(tokio::time::Instant::now());
            }
            let active = if call.push_to_talk.load(Ordering::Relaxed) {
                call.talk_key.load(Ordering::Relaxed)
            } else {
                call.speaking_at
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some_and(|t| t.elapsed() < HANGOVER)
            };
            let send = active
                && !call.muted.load(Ordering::Relaxed)
                && call.can_speak.load(Ordering::Relaxed);
            if send && let Ok(opus) = enc.encode(&frame) {
                let bytes = Packet {
                    seq,
                    speaking: loud,
                    opus,
                }
                .encode();
                for peer in call.peers().values() {
                    let _ = peer.conn.send_datagram(bytes.clone().into());
                }
            }
            seq = seq.wrapping_add(1);
        }
        // In.
        let mut mix = vec![0f32; FRAME];
        for peer in call.peers().values_mut() {
            let frame = match peer.jitter.pop() {
                Slot::Waiting => continue,
                Slot::Play(p) => peer.dec.decode(&p),
                Slot::Recover(next) => peer.dec.recover(&next),
                Slot::Conceal => peer.dec.conceal(),
            };
            let Ok(frame) = frame else { continue };
            if level(&frame) > threshold {
                peer.speaking_at = Some(tokio::time::Instant::now());
            }
            for (m, s) in mix.iter_mut().zip(frame) {
                *m += s;
            }
        }
        if call.deafened.load(Ordering::Relaxed) {
            mix.iter_mut().for_each(|s| *s = 0.0);
        }
        for s in mix.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        output.play(mix);
    }
}
