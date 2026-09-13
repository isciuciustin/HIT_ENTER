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
use he_proto::{Channel, InviteLink, Message, NetworkConfig, Password};
use iroh::EndpointAddr;
use serde::Serialize;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::events;
use crate::host;
use crate::session;
use crate::settings::Hosting;
use crate::state::App;

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

/// An `EndpointId`, or a refusal the frontend can show.
///
/// A space is addressed by a public key and nothing else, so "that is not an
/// EndpointId" is the only thing that can be wrong with an address before a
/// connection is attempted.
fn parse_endpoint_id(value: &str) -> Result<iroh::EndpointId> {
    value
        .parse()
        .map_err(|_| CommandError::message("that is not an EndpointId"))
}

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
    /// Unread across every channel, for the dot on the rail. Read from the
    /// mirror, so it is right with the network off and right at startup.
    pub unread: i64,
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
        let unread = app.mirror().unread_total(&server.endpoint_id).await?;
        summaries.push(ServerSummary {
            connected: session.is_some(),
            status: session
                .map(|s| events::describe(s.path()))
                .unwrap_or("offline"),
            unread,
            server,
        });
    }
    Ok(summaries)
}

/// Joins a space for the first time, or enrols this device on an existing
/// account.
///
/// `address` is anything a person might paste: a `hitenter://join?…` link, a
/// bare ticket, or an `EndpointId` (PLAN §5). A link also carries the invite
/// code; an explicit `invite` overrides it, so a stale link can be reused with
/// a fresh code.
///
/// With an invite this registers; without one it is a password login that
/// enrols this machine. Either way the password is used exactly once and never
/// stored — the device key is the login from here on (PLAN §3).
#[tauri::command]
pub async fn join_server(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    address: String,
    username: String,
    password: String,
    invite: Option<String>,
) -> Result<ServerSummary> {
    let link = InviteLink::parse_relaxed(&address)
        .map_err(|err| CommandError::message(err.to_string()))?;
    let invite = invite
        .filter(|code| !code.trim().is_empty())
        .or_else(|| link.code().map(str::to_owned));

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
    let summary = open_session(
        &app_handle,
        &app,
        link.addr().clone(),
        link.relay_url(),
        auth,
    )
    .await?;

    // Arriving somewhere new must not light up every channel with a badge
    // counting a conversation the user was never part of.
    app.mirror()
        .mark_all_read(&summary.server.endpoint_id)
        .await?;
    Ok(summary)
}

/// Reconnects to a space this device is already enrolled on. No password.
#[tauri::command]
pub async fn connect_server(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    endpoint_id: String,
) -> Result<ServerSummary> {
    let id = parse_endpoint_id(&endpoint_id)?;
    // The username disambiguates a device holding several accounts here; the
    // mirror is where the client recorded which one it uses. The relay is a
    // hint from the last time we saw this space — worth trying first, never
    // trusted: if it is stale, discovery re-resolves from the key (PLAN §4).
    let known = app.mirror().server(&endpoint_id).await?;
    let username = known.as_ref().map(|server| server.username.clone());
    let relay = known.and_then(|server| server.relay_url);

    let mut addr = EndpointAddr::new(id);
    if let Some(relay) = relay.as_deref()
        && let Ok(url) = relay.parse()
    {
        addr = addr.with_relay_url(url);
    }

    open_session(&app_handle, &app, addr, relay, Auth::Device { username }).await
}

async fn open_session(
    app_handle: &AppHandle,
    app: &Arc<App>,
    addr: EndpointAddr,
    relay_url: Option<String>,
    auth: Auth,
) -> Result<ServerSummary> {
    let endpoint_id = addr.id.to_string();
    let endpoint_id = endpoint_id.as_str();

    events::emit_connection(app_handle, endpoint_id, "connecting");

    // Bounded, because iroh will spend the better part of a minute trying
    // discovery and then every relay before it concludes that a space is
    // simply down. That is the right amount of effort for a bad network and
    // far too long to leave "connecting" on screen for a server that is off.
    let dialing = app.client().connect(addr, auth);
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
    let status = events::describe(session.path());

    // Write what the handshake told us before anything else can fail: the
    // channel list and the roster are what the rail renders next time, with or
    // without a network.
    app.mirror()
        .upsert_server(
            endpoint_id,
            &ready.server_name,
            &ready.user.username,
            Some(&ready.user.id),
            relay_url.as_deref(),
        )
        .await?;
    app.mirror()
        .replace_channels(endpoint_id, &ready.channels)
        .await?;
    app.mirror()
        .replace_members(endpoint_id, &ready.members)
        .await?;

    let events_rx = session
        .take_events()
        .ok_or_else(|| CommandError::message("session event stream was already taken"))?;

    // The slot is claimed before the supervisor starts, because the first
    // thing the supervisor does is ask whether the app still wants this space
    // connected — and the answer has to already be yes.
    let live = app.register(endpoint_id).await;
    live.attach(session::spawn(
        app_handle.clone(),
        Arc::clone(app),
        endpoint_id.to_owned(),
        session,
        events_rx,
    ));

    let server = app
        .mirror()
        .server(endpoint_id)
        .await?
        .ok_or_else(|| CommandError::message("the server vanished between two queries"))?;

    let unread = app.mirror().unread_total(endpoint_id).await?;
    Ok(ServerSummary {
        server,
        connected: true,
        status,
        unread,
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

/// An invite, in the two forms a person might use it.
#[derive(Debug, Serialize)]
pub struct Invite {
    /// `K7QP-2M4X-9WTZ` — readable aloud, and half of an invite.
    pub code: String,
    /// `hitenter://join?t=…&c=…` — the whole thing, and what to paste into a
    /// chat message (PLAN §5).
    pub link: String,
    /// False when the link carries no relay and no public address: the space
    /// is reachable on this network and nowhere else, and someone in another
    /// city will not get in. Only ever known for a space *this* machine hosts.
    pub reachable_remotely: bool,
}

/// Mints an invite code for a space this client is connected to.
///
/// The code comes back over the session like any other member's would —
/// hosting this space changes nothing about that path. What hosting adds is
/// the *address* half: only the process that owns the endpoint knows its own
/// relay and direct addresses, so a link for a space we host carries hints and
/// a link for someone else's carries only the key (which is enough, via
/// discovery, just slower to dial).
#[tauri::command]
pub async fn create_invite(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    expires_in: Option<u64>,
    max_uses: Option<i64>,
) -> Result<Invite> {
    let session = app
        .session(&endpoint_id)
        .await
        .ok_or_else(|| CommandError::message("not connected to that space"))?;
    let code = session.create_invite(expires_in, max_uses).await?;
    Ok(invite_for(&app, &endpoint_id, code).await)
}

async fn invite_for(app: &Arc<App>, endpoint_id: &str, code: String) -> Invite {
    let hosted = app
        .with_host(|host| {
            (host.endpoint_id() == endpoint_id).then(|| {
                (
                    host.invite_link(Some(code.clone())),
                    host.is_reachable_remotely(),
                )
            })
        })
        .await
        .flatten();

    match hosted {
        Some((link, reachable)) => Invite {
            code,
            link: link.to_string(),
            reachable_remotely: reachable,
        },
        None => {
            // Somebody else's space. We know its key and nothing else about
            // where it is, which is exactly what discovery is for.
            let link = endpoint_id
                .parse::<iroh::EndpointId>()
                .map(|id| InviteLink::new(EndpointAddr::new(id), Some(code.clone())).to_string())
                .unwrap_or_default();
            Invite {
                code,
                link,
                reachable_remotely: true,
            }
        }
    }
}

/// What a pasted string turns out to be, so the join dialog can fill itself in
/// rather than making the user split a link by hand.
#[derive(Debug, Serialize)]
pub struct ParsedLink {
    pub endpoint_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    /// True when this client is already in that space — in which case the
    /// answer is "you are already here", not a registration form.
    pub known: bool,
    /// True when the address is this machine's own space. Joining it is still
    /// an ordinary join over the network (PLAN §2.1); it is worth *saying* so
    /// the user is not confused about why their own space wants a password.
    pub is_own_space: bool,
}

/// Reads a `hitenter://` link, a ticket, or a bare address.
///
/// Pure parsing: nothing is dialled, nothing is stored, and an unreadable
/// string is an error the dialog shows rather than a failed connection ten
/// seconds later.
#[tauri::command]
pub async fn parse_link(app: State<'_, Arc<App>>, text: String) -> Result<ParsedLink> {
    let link =
        InviteLink::parse_relaxed(&text).map_err(|err| CommandError::message(err.to_string()))?;
    let endpoint_id = link.endpoint_id();
    Ok(ParsedLink {
        known: app.mirror().server(&endpoint_id).await?.is_some(),
        is_own_space: app.hosted_endpoint_id().await.as_deref() == Some(endpoint_id.as_str()),
        code: link.code().map(str::to_owned),
        relay_url: link.relay_url(),
        endpoint_id,
    })
}

// ---- hosting --------------------------------------------------------------

/// The state of the space this machine serves, or could serve.
#[derive(Debug, Serialize)]
pub struct HostStatus {
    /// A `server.db` exists here. Hosting can be switched on without creating
    /// anything.
    pub space_exists: bool,
    /// It is being served right now.
    pub running: bool,
    /// Start it when the app starts.
    pub auto_start: bool,
    /// The space's `EndpointId` — **not** this device's. They are separate
    /// keys with separate lifetimes (PLAN §3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    pub space_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// An address-only link, for pointing a second machine at this space.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// False while the space has no relay and no public address: it is
    /// reachable on this network and nowhere else.
    pub reachable_remotely: bool,
    /// Where `server.db` is, so an owner can back it up. It holds
    /// `server_meta.secret_key`, which *is* the space (PLAN §11).
    pub data_dir: String,
}

#[tauri::command]
pub async fn host_status(app: State<'_, Arc<App>>) -> Result<HostStatus> {
    Ok(status_of(&app).await)
}

async fn status_of(app: &Arc<App>) -> HostStatus {
    let settings = app.settings().await;
    let live = app
        .with_host(|host| {
            (
                host.endpoint_id(),
                host.space_name().to_owned(),
                host.invite_link(None).to_string(),
                host.is_reachable_remotely(),
            )
        })
        .await;

    let owner = owner_username(app).await;

    match live {
        Some((endpoint_id, space_name, link, reachable)) => HostStatus {
            space_exists: true,
            running: true,
            auto_start: settings.hosting.enabled,
            endpoint_id: Some(endpoint_id),
            space_name,
            owner,
            link: Some(link),
            reachable_remotely: reachable,
            data_dir: app.data_dir().display().to_string(),
        },
        None => HostStatus {
            space_exists: host::space_exists(app.data_dir()),
            running: false,
            auto_start: settings.hosting.enabled,
            endpoint_id: None,
            space_name: settings.hosting.space_name,
            owner: None,
            link: None,
            reachable_remotely: false,
            data_dir: app.data_dir().display().to_string(),
        },
    }
}

async fn owner_username(app: &Arc<App>) -> Option<String> {
    let server = app.host_server().await?;
    match host::owner_of(&server).await {
        Ok(owner) => owner.map(|user| user.username),
        Err(err) => {
            tracing::warn!(%err, "could not read the space's owner");
            None
        }
    }
}

/// What creating a space gives back: the space, and this machine's membership
/// of it.
#[derive(Debug, Serialize)]
pub struct SpaceCreated {
    pub host: HostStatus,
    pub server: ServerSummary,
}

/// Creates a space on this machine and joins it.
///
/// Three steps, and the third is the one that matters:
///
/// 1. `server.db` is created, which generates the space's identity.
/// 2. The owner account is made **locally**. It is the one account an invite
///    cannot gate, so it never crosses the network (PLAN §11).
/// 3. This machine's client then *dials the space by its `EndpointId`*, with
///    the owner's password, exactly as a stranger's client would — which is
///    what enrols this device and is why there is no separate host code path
///    to debug (PLAN §2.1).
#[tauri::command]
pub async fn create_space(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    name: String,
    username: String,
    password: String,
) -> Result<SpaceCreated> {
    if host::space_exists(app.data_dir()) {
        return Err(CommandError::message(
            "this machine already hosts a space — open it instead of creating one",
        ));
    }
    he_proto::limits::validate_username(&username)
        .map_err(|err| CommandError::message(err.to_string()))?;
    he_proto::limits::validate_password(&password)
        .map_err(|err| CommandError::message(err.to_string()))?;

    let mut settings = app.settings().await;
    settings.hosting = Hosting {
        enabled: true,
        space_name: name.trim().to_owned(),
    };
    app.put_settings(settings)
        .await
        .map_err(|err| CommandError::message(err.to_string()))?;

    let endpoint_id = app
        .start_hosting()
        .await
        .map_err(|err| CommandError::message(host::describe(&err)))?;

    // The owner exists before the first connection, because the connection is
    // what logs in as them.
    let owner = app
        .host_server()
        .await
        .ok_or_else(|| CommandError::message("the space stopped before it opened"))?
        .create_owner(&username, &Password::new(password.clone()))
        .await
        .map_err(|err| CommandError::message(host::describe(&err)));
    if let Err(err) = owner {
        // Leave nothing half-made: a space whose owner failed to be created
        // cannot be joined and cannot be created again.
        app.stop_hosting().await;
        return Err(err);
    }

    // Worth waiting for: an invite link minted before the space has reached a
    // relay only works on this LAN. The endpoint is taken out of the lock
    // first, so closing the space meanwhile does not queue behind this.
    if let Some(endpoint) = app.host_endpoint().await {
        host::wait_online(&endpoint).await;
    }

    let server = open_session(
        &app_handle,
        &app,
        own_space_addr(&app, &endpoint_id).await?,
        None,
        Auth::Password {
            username,
            password: Password::new(password),
        },
    )
    .await?;

    events::emit_host(&app_handle);
    Ok(SpaceCreated {
        host: status_of(&app).await,
        server,
    })
}

/// The address of our own space, hints included.
///
/// The hints are the only shortcut taken anywhere near the host's own
/// connection, and they are not a shortcut at all: they are the same address
/// hints that go into an invite link. The dial itself is an ordinary dial.
async fn own_space_addr(app: &Arc<App>, endpoint_id: &str) -> Result<EndpointAddr> {
    match app.with_host(|host| host.invite_link(None)).await {
        Some(link) => Ok(link.addr().clone()),
        None => Ok(EndpointAddr::new(parse_endpoint_id(endpoint_id)?)),
    }
}

/// Opens the space on this machine.
#[tauri::command]
pub async fn start_hosting(app_handle: AppHandle, app: State<'_, Arc<App>>) -> Result<HostStatus> {
    if !host::space_exists(app.data_dir()) {
        return Err(CommandError::message(
            "there is no space here yet — create one first",
        ));
    }
    app.start_hosting()
        .await
        .map_err(|err| CommandError::message(host::describe(&err)))?;

    let mut settings = app.settings().await;
    settings.hosting.enabled = true;
    app.put_settings(settings)
        .await
        .map_err(|err| CommandError::message(err.to_string()))?;

    rejoin_own_space(&app_handle, &app).await;
    events::emit_host(&app_handle);
    Ok(status_of(&app).await)
}

/// Reconnects this machine's client to the space it has just (re)opened.
///
/// Closing a space disconnects everyone, including the owner's own client —
/// correctly, since the server it was talking to went away. Reopening has to
/// put that back, or the host is left looking at their own space marked
/// offline with no way to reach it but restarting the app.
///
/// It is an ordinary dial with `Auth::Device`, the same one a returning member
/// makes: this device is enrolled, so it costs no password (PLAN §3). Failure
/// is logged and swallowed — the space is open either way, and that is what
/// the caller asked for.
async fn rejoin_own_space(app_handle: &AppHandle, app: &Arc<App>) {
    let Some(endpoint_id) = app.hosted_endpoint_id().await else {
        return;
    };
    // Only if this client is a member. A space can be hosted by a machine
    // whose user has not joined it — during `create_space`, for the moment
    // between the owner existing and the first connection.
    match app.mirror().server(&endpoint_id).await {
        Ok(Some(server)) => {
            let auth = Auth::Device {
                username: Some(server.username),
            };
            let addr = match own_space_addr(app, &endpoint_id).await {
                Ok(addr) => addr,
                Err(err) => {
                    tracing::warn!(error = %err.message, "could not address our own space");
                    return;
                }
            };
            if let Err(err) = open_session(app_handle, app, addr, None, auth).await {
                tracing::warn!(error = %err.message, "could not rejoin our own space");
            }
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(%err, "could not read the mirror"),
    }
}

/// Closes the space. Members see a clean goodbye rather than a timeout, and
/// the database and its identity stay exactly where they are.
#[tauri::command]
pub async fn stop_hosting(app_handle: AppHandle, app: State<'_, Arc<App>>) -> Result<HostStatus> {
    app.stop_hosting().await;

    let mut settings = app.settings().await;
    settings.hosting.enabled = false;
    app.put_settings(settings)
        .await
        .map_err(|err| CommandError::message(err.to_string()))?;

    events::emit_host(&app_handle);
    Ok(status_of(&app).await)
}

// ---- settings -------------------------------------------------------------

/// The settings, plus what the frontend needs to explain them.
#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub network: NetworkConfig,
    /// A sentence for the settings pane, built from the same struct rather
    /// than reassembled in Svelte.
    pub summary: String,
    /// True when nothing here would contact infrastructure the user did not
    /// choose. The pitch is "nobody can take this away from you"; this is the
    /// check (PLAN §4).
    pub self_contained: bool,
    pub data_dir: String,
    pub settings_path: String,
}

#[tauri::command]
pub async fn network_settings(app: State<'_, Arc<App>>) -> Result<SettingsView> {
    Ok(view_of(&app.settings().await.network, &app))
}

fn view_of(network: &NetworkConfig, app: &Arc<App>) -> SettingsView {
    SettingsView {
        summary: network.describe(),
        self_contained: network.is_self_contained(),
        data_dir: app.data_dir().display().to_string(),
        settings_path: crate::settings::path_of(app.data_dir())
            .display()
            .to_string(),
        network: network.clone(),
    }
}

/// Whether a settings change is live yet.
#[derive(Debug, Serialize)]
pub struct SettingsSaved {
    pub settings: SettingsView,
    /// The client endpoint was bound at startup with the old settings and is
    /// holding every open session; rebinding it would drop them all. So the
    /// new settings reach the client on the next launch, and the UI says so
    /// rather than letting the user wonder why nothing changed.
    pub restart_required: bool,
    /// The space, by contrast, was restarted here and now — it has an explicit
    /// on/off switch already, so there is nothing to be surprised by.
    pub host_restarted: bool,
}

/// Saves relay and discovery settings.
#[tauri::command]
pub async fn set_network_settings(
    app_handle: AppHandle,
    app: State<'_, Arc<App>>,
    network: NetworkConfig,
) -> Result<SettingsSaved> {
    let mut settings = app.settings().await;
    if settings.network == network {
        return Ok(SettingsSaved {
            settings: view_of(&settings.network, &app),
            restart_required: false,
            host_restarted: false,
        });
    }
    settings.network = network;
    app.put_settings(settings.clone())
        .await
        .map_err(|err| CommandError::message(err.to_string()))?;

    let host_restarted = if app.is_hosting().await {
        app.start_hosting()
            .await
            .map_err(|err| CommandError::message(host::describe(&err)))?;
        // The space came back on a new endpoint binding, so the session this
        // client held to it is gone with the old one.
        rejoin_own_space(&app_handle, &app).await;
        events::emit_host(&app_handle);
        true
    } else {
        false
    };

    Ok(SettingsSaved {
        settings: view_of(&settings.network, &app),
        restart_required: true,
        host_restarted,
    })
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

/// Rewrites one of this account's own messages.
///
/// The authoritative row comes back as a `he://edited` event — the same one
/// every other member gets — so there is one code path that applies a change
/// to a message rather than one for the author and one for everybody else.
#[tauri::command]
pub async fn edit_message(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    message_id: String,
    content: String,
) -> Result<()> {
    he_proto::limits::validate_message_content(&content)
        .map_err(|err| CommandError::message(err.to_string()))?;
    let session = app
        .session(&endpoint_id)
        .await
        // Deliberately not queued. An edit composed offline would have to be
        // reconciled against whatever happened to the message in the meantime,
        // and "your edit will be applied at some point" is a worse promise
        // than "you are offline".
        .ok_or_else(|| CommandError::message("not connected to that space"))?;
    session.edit_message(&message_id, &content).await?;
    Ok(())
}

/// Withdraws one of this account's own messages.
#[tauri::command]
pub async fn delete_message(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    message_id: String,
) -> Result<()> {
    let session = app
        .session(&endpoint_id)
        .await
        .ok_or_else(|| CommandError::message("not connected to that space"))?;
    session.delete_message(&message_id).await?;
    Ok(())
}

/// Says that this account is composing in a channel.
///
/// Silently does nothing when offline, and the failure of a typing indicator
/// is never worth telling anyone about: there is nothing the user could do and
/// nothing was lost.
#[tauri::command]
pub async fn typing(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    channel_id: String,
) -> Result<()> {
    if let Some(session) = app.session(&endpoint_id).await {
        let _ = session.typing(&channel_id).await;
    }
    Ok(())
}

/// Who has a live session on a space right now, by user id.
///
/// Read from process state rather than from the mirror, because presence is
/// not a thing that can be stored: a row saying "online" would be a lie the
/// moment this app closed (PLAN §6). An offline space answers with nobody,
/// which the window renders as "unknown" rather than "everybody is away".
#[tauri::command]
pub async fn online(app: State<'_, Arc<App>>, endpoint_id: String) -> Result<Vec<String>> {
    Ok(app.online(&endpoint_id).await)
}

/// The cached member list. Never touches the network.
#[tauri::command]
pub async fn members(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
) -> Result<Vec<he_proto::Member>> {
    Ok(app.mirror().members(&endpoint_id).await?)
}

/// Unread counts per channel, for the badge in the rail.
#[derive(Debug, Serialize)]
pub struct Unread {
    pub channel_id: String,
    pub unread: i64,
}

#[tauri::command]
pub async fn unread(app: State<'_, Arc<App>>, endpoint_id: String) -> Result<Vec<Unread>> {
    Ok(app
        .mirror()
        .unread(&endpoint_id)
        .await?
        .into_iter()
        .map(|(channel_id, unread)| Unread { channel_id, unread })
        .collect())
}

/// Marks a channel read up to and including `message_id`.
///
/// Only ever moves forward, in the mirror: scrolling back through history is
/// not the same as un-reading it.
#[tauri::command]
pub async fn mark_read(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    channel_id: String,
    message_id: String,
) -> Result<()> {
    app.mirror()
        .mark_read(&endpoint_id, &channel_id, &message_id)
        .await?;
    Ok(())
}

// ---- owner tools (PLAN §12, M6) -------------------------------------------
//
// Every one of these needs a live session, and none of them is queued when
// there is not one. Moderation is a decision about a space, and a decision
// that will be applied "at some point, against whatever state you find" is a
// worse promise than "you are offline".

async fn connected(app: &Arc<App>, endpoint_id: &str) -> Result<Arc<Session>> {
    app.session(endpoint_id)
        .await
        .ok_or_else(|| CommandError::message("not connected to that space"))
}

/// Creates a channel. Owner only; the server is what enforces that.
#[tauri::command]
pub async fn create_channel(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    name: String,
    topic: Option<String>,
) -> Result<Channel> {
    let topic = topic.filter(|t| !t.trim().is_empty());
    let session = connected(&app, &endpoint_id).await?;
    let channel = session.create_channel(&name, topic.as_deref()).await?;
    // The `he://channels` event that follows updates the mirror and the rail;
    // this returns the new channel so the caller can select it without
    // waiting for the round trip it just made.
    Ok(channel)
}

/// Deletes a channel and every message in it. The server refuses the last one.
#[tauri::command]
pub async fn delete_channel(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    channel_id: String,
) -> Result<()> {
    connected(&app, &endpoint_id)
        .await?
        .delete_channel(&channel_id)
        .await?;
    Ok(())
}

/// Logs a member out of every machine they are enrolled on.
#[tauri::command]
pub async fn kick_member(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    user_id: String,
) -> Result<()> {
    connected(&app, &endpoint_id).await?.kick(&user_id).await?;
    Ok(())
}

/// Bans or un-bans a member.
#[tauri::command]
pub async fn set_member_banned(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    user_id: String,
    banned: bool,
) -> Result<()> {
    connected(&app, &endpoint_id)
        .await?
        .set_banned(&user_id, banned)
        .await?;
    Ok(())
}

/// Kicks one machine off. Your own, or anybody's if you are the owner.
#[tauri::command]
pub async fn revoke_device(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    user_id: String,
    device_id: String,
) -> Result<()> {
    connected(&app, &endpoint_id)
        .await?
        .revoke_device(&user_id, &device_id)
        .await?;
    Ok(())
}

/// The devices enrolled for an account. `None` asks about your own.
#[tauri::command]
pub async fn devices(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    user_id: Option<String>,
) -> Result<Vec<he_proto::DeviceInfo>> {
    let devices = connected(&app, &endpoint_id)
        .await?
        .devices(user_id.as_deref())
        .await?;
    Ok(devices)
}

/// Every invite on the space. Owner only.
#[tauri::command]
pub async fn invites(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
) -> Result<Vec<he_proto::InviteInfo>> {
    Ok(connected(&app, &endpoint_id).await?.invites().await?)
}

/// Deletes an invite. Accounts already made with it stay.
#[tauri::command]
pub async fn revoke_invite(
    app: State<'_, Arc<App>>,
    endpoint_id: String,
    code: String,
) -> Result<()> {
    connected(&app, &endpoint_id)
        .await?
        .revoke_invite(&code)
        .await?;
    Ok(())
}
