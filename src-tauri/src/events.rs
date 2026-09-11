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
/// The connection changed state — including a silent relay→direct upgrade.
pub const CONNECTION_EVENT: &str = "he://connection";

#[derive(Debug, Clone, Serialize)]
pub struct MessageEvent {
    pub server: String,
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
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

pub fn emit_connection(app: &AppHandle, server: &str, status: &'static str) {
    let _ = app.emit(
        CONNECTION_EVENT,
        ConnectionEvent {
            server: server.to_owned(),
            status,
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
                    // Disk first, screen second.
                    if let Err(err) = mirror.record_message(&server, &message).await {
                        // Worth a log line, but not worth dropping the
                        // message: the user should still see what was said.
                        tracing::error!(%err, "could not mirror a message");
                    }
                    if let Some(nonce) = nonce.as_deref() {
                        // Acknowledged, so it is no longer pending.
                        let _ = mirror.dequeue(nonce).await;
                    }
                    let _ = app.emit(
                        MESSAGE_EVENT,
                        MessageEvent { server: server.clone(), message, nonce },
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

    emit_connection(&app, &server, describe(ConnectionPath::Offline));
    tracing::info!(%server, "event pump stopped");
}
