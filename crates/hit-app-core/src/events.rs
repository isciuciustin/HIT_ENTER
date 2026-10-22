//! Getting the node's events onto the screen — any screen.
//!
//! One task reads [`hit_node::Node::subscribe`], which sees an event only
//! after the store has it — **the mirror is written before the UI is told**
//! — and hands a [`UiEvent`] to the window's [`Sink`]. Others tell it, every
//! few seconds, who is here, and every 250 ms who is in which call.

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use hit_event::group;
use tokio::sync::broadcast::error::RecvError;

use crate::state::{
    App, DmView, MessageView, ReactionView, VoiceView, dm_view, message_view, npub, reaction_view,
};

const PRESENT_EVERY: Duration = Duration::from_secs(3);
const VOICE_EVERY: Duration = Duration::from_millis(250);

/// What a window is told. Ids are hex, people and groups npubs — the same
/// strings every `App` question takes.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// A message was stored.
    Message(MessageView),
    /// A reaction was stored.
    Reaction(ReactionView),
    /// A zap of this message was carried in (M10); ask for its total —
    /// whether it counts is the node's to say.
    Zap(String),
    /// A DM arrived — opened here; the wrap stays sealed on disk.
    Dm(DmView),
    /// Someone's profile changed.
    Profile(String),
    /// A group's state changed — members, roles, channels; ask for the rest.
    Group(String),
    /// Messages or reactions left the store — withdrawn, removed by a
    /// moderator, or silenced: drop these ids.
    Withdrawn(Vec<String>),
    /// Events were missed; whatever the window caches may be stale.
    Resync,
    /// The people here lately, whenever that set changes.
    Present(Vec<String>),
    /// Who is in which voice channel, and this person's call, whenever that
    /// changes.
    Voice(VoiceView),
    /// A call started ringing: ask the desktop for attention — the window
    /// has no sound of its own. Sent just before the `Voice` that shows it.
    Ringing,
}

/// Where a window takes its events. Called from tokio tasks, never from the
/// window's own thread: a sink hands the event over and returns.
pub trait Sink: Send + Sync + 'static {
    fn emit(&self, event: UiEvent);
}

/// Starts the pump once the node runs; a no-op before, or a second time
/// (`pumping` remembers).
pub async fn start(sink: Arc<dyn Sink>, pumping: &AtomicBool, app: &Arc<App>) {
    let Ok(node) = app.node().await else { return };
    if pumping.swap(true, Ordering::SeqCst) {
        return;
    }
    let me = node.person();
    let mut rx = node.subscribe();
    let s = sink.clone();
    let node_for_pump = node.clone();
    let pump = tokio::spawn(async move {
        let node = node_for_pump;
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(message) = message_view(&ev, &me) {
                        s.emit(UiEvent::Message(message));
                    } else if let Some(reaction) = reaction_view(&ev, &me) {
                        s.emit(UiEvent::Reaction(reaction));
                    } else if ev.kind == hit_event::zap::ZAP_CARRIER {
                        if let Some(target) = group::targets_of(&ev).first() {
                            s.emit(UiEvent::Zap(target.to_hex()));
                        }
                    } else if ev.kind == group::DELETION || ev.kind == group::DELETE_EVENT {
                        // What they actually dropped arrives on the node's
                        // removed channel, below.
                    } else if ev.kind == hit_event::dm::GIFT_WRAP {
                        // Ours to open only if it is to us; the outbox copy
                        // of one we sent is not.
                        if let Some(d) = node.open_dm(&ev) {
                            s.emit(UiEvent::Dm(dm_view(&d, &me)));
                        }
                    } else if ev.kind == group::PRESENCE || ev.kind == group::VOICE_PRESENCE {
                        // Counted by the node; the ticker below tells the window.
                    } else if ev.kind == hit_group::PROFILE {
                        s.emit(UiEvent::Profile(npub(&ev.pubkey)));
                    } else if let Some(g) = group::group_of(&ev) {
                        s.emit(UiEvent::Group(npub(&g)));
                    }
                }
                Err(RecvError::Lagged(_)) => s.emit(UiEvent::Resync),
                Err(RecvError::Closed) => break,
            }
        }
    });
    app.attach(pump).await;

    // What the store dropped after it was shown: withdrawn by its author,
    // removed by a moderator, or silenced by a ban or mute that came late.
    // Only these ids — a withdrawal naming somebody else's words removes
    // nothing, and so neither may the window.
    let mut removed = node.subscribe_removed();
    let s = sink.clone();
    let drops = tokio::spawn(async move {
        loop {
            match removed.recv().await {
                Ok(ids) => s.emit(UiEvent::Withdrawn(ids.iter().map(|i| i.to_hex()).collect())),
                Err(RecvError::Lagged(_)) => s.emit(UiEvent::Resync),
                Err(RecvError::Closed) => break,
            }
        }
    });
    app.attach(drops).await;

    let (s, a) = (sink.clone(), app.clone());
    let reach = tokio::spawn(async move {
        let mut last: Option<BTreeSet<String>> = None;
        loop {
            if let Ok(now) = a.present().await
                && last.as_ref() != Some(&now)
            {
                s.emit(UiEvent::Present(now.iter().cloned().collect()));
                last = Some(now);
            }
            tokio::time::sleep(PRESENT_EVERY).await;
        }
    });
    app.attach(reach).await;

    let (s, a) = (sink, app.clone());
    let voice = tokio::spawn(async move {
        let mut last = String::new();
        let mut rang: Option<String> = None;
        loop {
            if let Ok(v) = a.voice_view().await
                && let Ok(json) = serde_json::to_string(&v)
                && json != last
            {
                if v.incoming.is_some() && v.incoming != rang {
                    s.emit(UiEvent::Ringing);
                }
                rang = v.incoming.clone();
                s.emit(UiEvent::Voice(v));
                last = json;
            }
            tokio::time::sleep(VOICE_EVERY).await;
        }
    });
    app.attach(voice).await;
}
