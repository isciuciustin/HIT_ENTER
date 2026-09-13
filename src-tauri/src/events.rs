//! Getting a server's events onto the screen.
//!
//! One task per connected server. It owns that session's event stream, writes
//! everything it sees into the mirror **before** telling the UI, and emits a
//! Tauri event the frontend listens for.
//!
//! Mirror-first is the ordering that matters. If the UI were told first and
//! the app closed a millisecond later, the message would be on screen and not
//! on disk — and "everything is on your disk" would be true except for the
//! last thing anyone said.

use std::sync::Arc;
use std::time::Duration;

use he_client::{ConnectionPath, Mirror, Session};
use he_proto::{Message, ServerFrame};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

/// A message arrived. `nonce` is present only if *this* client sent it.
pub const MESSAGE_EVENT: &str = "he://message";
/// A message was rewritten by its author.
pub const EDITED_EVENT: &str = "he://edited";
/// A message was withdrawn by its author, and has to leave the screen.
pub const DELETED_EVENT: &str = "he://deleted";
/// A member gained or lost their last live session.
pub const PRESENCE_EVENT: &str = "he://presence";
/// Somebody is composing. Expires on its own; see
/// `he_proto::limits::TYPING_TIMEOUT_SECS`.
pub const TYPING_EVENT: &str = "he://typing";
/// The connection changed state — including a silent relay→direct upgrade.
pub const CONNECTION_EVENT: &str = "he://connection";
/// The space this machine hosts started, stopped, or was rebound.
pub const HOST_EVENT: &str = "he://host";
/// A `hitenter://` link arrived from the desktop — the user clicked an invite
/// somewhere else and this window is what opened.
pub const LINK_EVENT: &str = "he://link";

#[derive(Debug, Clone, Serialize)]
pub struct MessageEvent {
    pub server: String,
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditedEvent {
    pub server: String,
    pub message: Message,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeletedEvent {
    pub server: String,
    pub id: String,
    pub channel_id: String,
    pub deleted_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PresenceEvent {
    pub server: String,
    pub user_id: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypingEvent {
    pub server: String,
    pub channel_id: String,
    pub user_id: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionEvent {
    pub server: String,
    /// `direct`, `relayed`, `connecting` or `offline`.
    pub status: &'static str,
}

/// How often to re-read the connection path.
///
/// iroh opens through a relay and then *upgrades* to a direct path once hole
/// punching succeeds, without dropping the connection (PLAN §4). Nothing tells
/// us; the path just changes. Polling is what turns that into a status
/// indicator the user can see rather than a number that is wrong all evening.
const PATH_POLL: Duration = Duration::from_secs(3);

pub fn describe(path: ConnectionPath) -> &'static str {
    match path {
        ConnectionPath::Direct => "direct",
        ConnectionPath::Relayed => "relayed",
        ConnectionPath::Connecting => "connecting",
        ConnectionPath::Offline => "offline",
    }
}

/// Tells the window that hosting changed, without saying how.
///
/// The frontend asks `host_status` for the details rather than having them
/// pushed: there is exactly one shape of that answer, and duplicating it into
/// an event payload is how the two drift apart.
pub fn emit_host(app: &AppHandle) {
    let _ = app.emit(HOST_EVENT, ());
}

/// Hands a `hitenter://` link to the window.
///
/// Nothing is parsed here and nothing is dialled: the frontend opens the join
/// dialog with it, and the user decides. A link that joined a space on arrival
/// would make clicking a URL enough to enrol this device somewhere.
pub fn emit_link(app: &AppHandle, link: &str) {
    let _ = app.emit(LINK_EVENT, link);
}

pub fn emit_connection(app: &AppHandle, server: &str, status: &'static str) {
    let _ = app.emit(
        CONNECTION_EVENT,
        ConnectionEvent {
            server: server.to_owned(),
            status,
        },
    );
}

/// Writes a message to the mirror and then puts it on screen.
///
/// Disk first, screen second, in one place — every path that shows a message
/// goes through here, so there is one ordering to get right rather than one
/// per call site. A mirror write that fails is worth a log line and is not
/// worth dropping the message over: the user should still see what was said.
pub async fn deliver(
    app: &AppHandle,
    mirror: &Mirror,
    server: &str,
    message: Message,
    nonce: Option<String>,
) {
    if let Err(err) = mirror.record_message(server, &message).await {
        tracing::error!(%err, "could not mirror a message");
    }
    if let Some(nonce) = nonce.as_deref() {
        // Acknowledged, so it is no longer pending.
        let _ = mirror.dequeue(nonce).await;
    }
    // A message that arrived already edited or already withdrawn is not news
    // to the pane, it is a correction to something it may be showing — which
    // is the normal shape of a `resume` after a day offline.
    if message.deleted_at.is_some() {
        emit_deleted(app, mirror, server, &message).await;
        return;
    }
    if message.edited_at.is_some() {
        let _ = app.emit(
            EDITED_EVENT,
            EditedEvent {
                server: server.to_owned(),
                message,
            },
        );
        return;
    }
    let _ = app.emit(
        MESSAGE_EVENT,
        MessageEvent {
            server: server.to_owned(),
            message,
            nonce,
        },
    );
}

/// Applies a deletion to the mirror and then takes it off the screen.
async fn emit_deleted(app: &AppHandle, mirror: &Mirror, server: &str, message: &Message) {
    let deleted_at = message.deleted_at.unwrap_or_default();
    if let Err(err) = mirror.apply_delete(server, &message.id, deleted_at).await {
        tracing::error!(%err, "could not mirror a deletion");
    }
    let _ = app.emit(
        DELETED_EVENT,
        DeletedEvent {
            server: server.to_owned(),
            id: message.id.clone(),
            channel_id: message.channel_id.clone(),
            deleted_at,
        },
    );
}

/// Runs one server's event pump until the session ends.
pub async fn pump(
    app: AppHandle,
    mirror: Mirror,
    session: Arc<Session>,
    mut events: mpsc::Receiver<ServerFrame>,
    server: String,
) {
    let mut last_status = describe(session.path());
    emit_connection(&app, &server, last_status);

    let mut ticker = tokio::time::interval(PATH_POLL);
    ticker.tick().await; // the first tick is immediate

    loop {
        tokio::select! {
            frame = events.recv() => match frame {
                Some(ServerFrame::Message { message, nonce }) => {
                    deliver(&app, &mirror, &server, message, nonce).await;
                }
                Some(ServerFrame::Edited { message }) => {
                    // Disk first, screen second — the same rule as a new
                    // message. A correction on screen and not on disk makes
                    // the mirror wrong about the current text.
                    if let Err(err) = mirror.apply_edit(&server, &message).await {
                        tracing::error!(%err, "could not mirror an edit");
                    }
                    let _ = app.emit(
                        EDITED_EVENT,
                        EditedEvent { server: server.clone(), message },
                    );
                }
                Some(ServerFrame::Deleted { id, channel_id, deleted_at }) => {
                    if let Err(err) = mirror.apply_delete(&server, &id, deleted_at).await {
                        tracing::error!(%err, "could not mirror a deletion");
                    }
                    let _ = app.emit(
                        DELETED_EVENT,
                        DeletedEvent {
                            server: server.clone(),
                            id,
                            channel_id,
                            deleted_at,
                        },
                    );
                }
                // Presence and typing are live state and are never written to
                // disk: a stored "online" would be a lie the moment this app
                // closed, and a stored "typing" would be one eight seconds
                // later (PLAN §6).
                Some(ServerFrame::Presence { user_id, online }) => {
                    let _ = app.emit(
                        PRESENCE_EVENT,
                        PresenceEvent { server: server.clone(), user_id, online },
                    );
                }
                Some(ServerFrame::Typing { channel_id, user_id, username }) => {
                    let _ = app.emit(
                        TYPING_EVENT,
                        TypingEvent {
                            server: server.clone(),
                            channel_id,
                            user_id,
                            username,
                        },
                    );
                }
                Some(ServerFrame::Error(err)) => {
                    tracing::warn!(code = ?err.code, "server ended the session");
                    break;
                }
                Some(ServerFrame::Ready(_)) => {
                    tracing::warn!("unexpected second ready frame");
                }
                None => break,
            },

            _ = ticker.tick() => {
                let status = describe(session.path());
                if status != last_status {
                    // A relayed connection that became direct is good news and
                    // the user is told; the reverse is why their app got slow.
                    tracing::info!(%server, status, "connection path changed");
                    last_status = status;
                    emit_connection(&app, &server, status);
                }
            }
        }
    }

    tracing::info!(%server, "event pump stopped");
}
