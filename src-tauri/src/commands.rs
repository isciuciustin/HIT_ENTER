//! The bridge the frontend calls across.
//!
//! Two rules shape every command here.
//!
//! **The mirror answers first.** [`history`] and [`channels`] read the local
//! database and never touch the network, so the app paints instantly and works
//! with the wifi off. [`sync_channel`] is the separate, explicit call that goes
//! out to the server and writes what it finds back to disk. A UI that mixed
//! the two would be a UI that spins when the train enters a tunnel.
//!
//! **Errors keep their code.** `DEVICE_NOT_ENROLLED` means "show the password
//! form", `RATE_LIMITED` means "wait, and say how long". Flattening those into
//! a string would make the frontend match on prose.

use std::sync::Arc;

use he_client::mirror::MirroredServer;
use he_client::{ClientError, Session};
use he_proto::rpc::Auth;
use he_proto::{Channel, Message, Password};
use iroh::{EndpointAddr, EndpointId};
use serde::Serialize;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::events;
use crate::state::{App, Live};

/// An error the frontend can act on rather than only display.
#[derive(Debug, Serialize)]
pub struct CommandError {
    /// The server's `ErrorCode`, when the failure came from one — so the UI
    /// can branch on `DEVICE_NOT_ENROLLED` instead of on a sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    /// Seconds to wait, on `RATE_LIMITED`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

impl From<ClientError> for CommandError {
    fn from(err: ClientError) -> Self {
        Self {
            code: err.code().and_then(|code| {
                serde_json::to_value(code)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
            }),
            retry_after: err.retry_after().map(|d| d.as_secs()),
            message: err.to_string(),
        }
    }
}

impl CommandError {
    fn message(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
            retry_after: None,
        }
    }
}

type Result<T> = std::result::Result<T, CommandError>;

/// How long to try before calling a space offline.
///
/// Generous enough for a relayed connection on a slow network, short enough
/// that a server which is simply switched off says so rather than spinning.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// A server as the server rail renders it: what the mirror knows, plus whether
/// it is connected right now.
#[derive(Debug, Serialize)]
pub struct ServerSummary {
    #[serde(flatten)]
    pub server: MirroredServer,
    pub connected: bool,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub protocol: String,
    pub protocol_version: u32,
    /// This machine's `EndpointId`. Shown in settings so a user can recognise
    /// their own device in a server owner's list.
    pub device_id: String,
    pub limits: Limits,
}

/// The shared validation rules, handed to the frontend rather than restated
/// there.
///
/// A number typed into a Svelte file is a number that drifts from
/// `he_proto::limits` the first time one of them changes, and the symptom is a
/// composer that lets you write a message the server then refuses.
#[derive(Debug, Serialize)]
pub struct Limits {
    pub message_max_chars: usize,
    pub username_min_chars: usize,
    pub username_max_chars: usize,
    pub password_min_bytes: usize,
    pub channel_name_max_chars: usize,
}

impl Limits {
    const fn shared() -> Self {
        use he_proto::limits as l;
        Self {
            message_max_chars: l::MESSAGE_MAX_CHARS,
            username_min_chars: l::USERNAME_MIN_CHARS,
            username_max_chars: l::USERNAME_MAX_CHARS,
            password_min_bytes: l::PASSWORD_MIN_BYTES,
            channel_name_max_chars: l::CHANNEL_NAME_MAX_CHARS,
        }
    }
}

#[tauri::command]
pub fn app_info(app: State<'_, Arc<App>>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        protocol: String::from_utf8_lossy(he_proto::ALPN).into_owned(),
        protocol_version: he_proto::PROTOCOL_VERSION,
        device_id: app.device_id().to_owned(),
        limits: Limits::shared(),
    }
}

/// Every space this client knows, connected or not.
#[tauri::command]
pub async fn list_servers(app: State<'_, Arc<App>>) -> Result<Vec<ServerSummary>> {
    let servers = app.mirror().servers().await?;
    let mut summaries = Vec::with_capacity(servers.len());
    for server in servers {
        let session = app.session(&server.endpoint_id).await;
        summaries.push(ServerSummary {
            connected: session.is_some(),
            status: session
                .map(|s| events::describe(s.path()))
                .unwrap_or("offline"),
            server,
        });
    }
    Ok(summaries)
}

/// Joins a space for the first time, or enrols this device on an existing
/// account.
///
/// With an invite this registers; without one it is a password login that
/// enrols this machine. Either way the password is used exactly once and never
/// stored — the device key is the login from here on (PLAN §3).
#[tauri::command]
pub async fn join_server(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    username: String,
    password: String,
    invite: Option<String>,
) -> Result<ServerSummary> {
    let auth = match invite {
        Some(invite) => Auth::Register {
            invite,
            username: username.clone(),
            password: Password::new(password),
        },
        None => Auth::Password {
            username: username.clone(),
            password: Password::new(password),
        },
    };
    open_session(&app_handle, &app, &endpoint_id, auth).await
}

/// Reconnects to a space this device is already enrolled on. No password.
#[tauri::command]
pub async fn connect_server(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    endpoint_id: String,
) -> Result<ServerSummary> {
    // The username disambiguates a device holding several accounts here; the
    // mirror is where the client recorded which one it uses.
    let username = app
        .mirror()
        .server(&endpoint_id)
        .await?
        .map(|server| server.username);
    open_session(&app_handle, &app, &endpoint_id, Auth::Device { username }).await
}

async fn open_session(
    app_handle: &AppHandle,
    app: &Arc<App>,
    endpoint_id: &str,
    auth: Auth,
) -> Result<ServerSummary> {
    let id: EndpointId = endpoint_id
        .parse()
        .map_err(|_| CommandError::message("that is not an EndpointId"))?;

    events::emit_connection(app_handle, endpoint_id, "connecting");

    // Bounded, because iroh will spend the better part of a minute trying
    // discovery and then every relay before it concludes that a space is
    // simply down. That is the right amount of effort for a bad network and
    // far too long to leave "connecting" on screen for a server that is off.
    let dialing = app.client().connect(EndpointAddr::new(id), auth);
    let mut session = match tokio::time::timeout(CONNECT_TIMEOUT, dialing).await {
        Ok(Ok(session)) => session,
        Ok(Err(err)) => {
            events::emit_connection(app_handle, endpoint_id, "offline");
            return Err(err.into());
        }
        Err(_elapsed) => {
            events::emit_connection(app_handle, endpoint_id, "offline");
            return Err(CommandError::message(
                "could not reach that space — it may be offline",
            ));
        }
    };

    let ready = session.ready().clone();

    // Write what the handshake told us before anything else can fail: the
    // channel list is what the rail renders next time, with or without a
    // network.
    app.mirror()
        .upsert_server(endpoint_id, &ready.server_name, &ready.user.username, None)
        .await?;
    app.mirror()
        .replace_channels(endpoint_id, &ready.channels)
        .await?;

    let events_rx = session
        .take_events()
        .ok_or_else(|| CommandError::message("session event stream was already taken"))?;
    let session = Arc::new(session);

    let pump = tauri::async_runtime::spawn(events::pump(
        app_handle.clone(),
        app.mirror().clone(),
        session.clone(),
        events_rx,
        endpoint_id.to_owned(),
    ));

    let status = events::describe(session.path());
    app.insert_session(endpoint_id, Live { session, pump })
        .await;

    let server = app
        .mirror()
        .server(endpoint_id)
        .await?
        .ok_or_else(|| CommandError::message("the server vanished between two queries"))?;

    Ok(ServerSummary {
        server,
        connected: true,
        status,
    })
}

#[tauri::command]
pub async fn disconnect_server(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    endpoint_id: String,
) -> Result<()> {
    if app.remove_session(&endpoint_id).await {
        events::emit_connection(&app_handle, &endpoint_id, "offline");
    }
    Ok(())
}

/// Removes a space **and its mirror**.
#[tauri::command]
pub async fn forget_server(app: State<'_, Arc<App>>, endpoint_id: String) -> Result<()> {
    app.remove_session(&endpoint_id).await;
    app.mirror().forget_server(&endpoint_id).await?;
    Ok(())
}

/// The cached channel list. Never touches the network.
#[tauri::command]
pub async fn channels(app: State<'_, Arc<App>>, endpoint_id: String) -> Result<Vec<Channel>> {
    Ok(app.mirror().channels(&endpoint_id).await?)
}

/// A page of cached history, newest first, ending just before `before`.
///
/// Reads the mirror only. Instant, and correct with the network off.
#[tauri::command]
pub async fn history(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    channel_id: String,
    before: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<Message>> {
    let limit = limit.unwrap_or(he_proto::limits::BACKFILL_DEFAULT_LIMIT);
    Ok(app
        .mirror()
        .messages(&endpoint_id, &channel_id, before.as_deref(), limit)
        .await?)
}

/// Fetches a page from the server and writes it to the mirror.
///
/// Separate from [`history`] on purpose: this is the call that can block on a
/// network, so the UI decides when to make it rather than discovering the
/// latency inside a render.
#[tauri::command]
pub async fn sync_channel(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    channel_id: String,
    before: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<Message>> {
    let Some(session) = app.session(&endpoint_id).await else {
        // Not an error. Being offline is a normal state for this app, and the
        // caller already has the mirror's answer.
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(he_proto::limits::BACKFILL_DEFAULT_LIMIT);

    let messages = session
        .backfill(&channel_id, before.as_deref(), limit)
        .await?;
    app.mirror()
        .record_messages(&endpoint_id, &messages)
        .await?;
    Ok(messages)
}

/// What a `send` gave back to the caller that composed it.
#[derive(Debug, Serialize)]
pub struct Sent {
    /// The nonce the optimistic bubble is tagged with. The authoritative row
    /// arrives as a `he://message` event carrying the same one.
    pub nonce: String,
    /// False when there was no connection: the message is in the outbox and
    /// M5's drain is what will deliver it.
    pub delivered: bool,
}

/// Posts a message.
///
/// Written to the outbox *before* it is sent, so a message typed into a
/// connection that turns out to be dead is on disk rather than lost with the
/// keystroke. The server's echo clears it.
#[tauri::command]
pub async fn send_message(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    channel_id: String,
    content: String,
    nonce: Option<String>,
) -> Result<Sent> {
    he_proto::limits::validate_message_content(&content)
        .map_err(|err| CommandError::message(err.to_string()))?;

    let nonce = nonce.unwrap_or_else(|| Uuid::now_v7().to_string());
    app.mirror()
        .queue(&nonce, &endpoint_id, &channel_id, &content)
        .await?;

    let Some(session) = app.session(&endpoint_id).await else {
        return Ok(Sent {
            nonce,
            delivered: false,
        });
    };

    match send_with_nonce(&session, &channel_id, &content, &nonce).await {
        Ok(()) => Ok(Sent {
            nonce,
            delivered: true,
        }),
        // It stays in the outbox. The UI keeps its bubble marked pending, and
        // the message is still on disk if the app closes.
        Err(err) => Err(err.into()),
    }
}

async fn send_with_nonce(
    session: &Session,
    channel_id: &str,
    content: &str,
    nonce: &str,
) -> he_client::Result<()> {
    use he_proto::rpc::{Request, Response};
    match session
        .request(Request::Send {
            channel_id: channel_id.to_owned(),
            content: content.to_owned(),
            nonce: nonce.to_owned(),
        })
        .await?
    {
        Response::Ok => Ok(()),
        _ => Err(ClientError::Unexpected("response")),
    }
}

/// Mints an invite code for a space this client is connected to.
#[tauri::command]
pub async fn create_invite(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    expires_in: Option<u64>,
    max_uses: Option<i64>,
) -> Result<String> {
    let session = app
        .session(&endpoint_id)
        .await
        .ok_or_else(|| CommandError::message("not connected to that space"))?;
    Ok(session.create_invite(expires_in, max_uses).await?)
}

/// Everything still waiting to be sent, so the UI can mark those bubbles
/// pending after a restart.
#[tauri::command]
pub async fn pending_messages(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
) -> Result<Vec<PendingMessage>> {
    Ok(app
        .mirror()
        .pending(&endpoint_id)
        .await?
        .into_iter()
        .map(|queued| PendingMessage {
            nonce: queued.nonce,
            channel_id: queued.channel_id,
            content: queued.content,
            created_at: queued.created_at,
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct PendingMessage {
    pub nonce: String,
    pub channel_id: String,
    pub content: String,
    pub created_at: i64,
}
