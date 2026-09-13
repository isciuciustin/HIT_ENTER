//! The iroh side of the server: accepting connections and serving sessions.
//!
//! **Accepting a connection is not authorization** (PLAN §11). Any endpoint on
//! the internet that knows this server's `EndpointId` can complete a QUIC
//! handshake with it; all that buys them is the right to send one [`Hello`]
//! and be told no. Everything past that point needs a [`Session`], and a
//! `Session` is only ever produced by [`handshake`], which only ever produces
//! one by calling into the authentication [`Server`] shipped in M1.
//!
//! ## Stream shapes (PLAN §9)
//!
//! - **Control stream** — the first bi-stream on the connection, opened by the
//!   client and held open. `Hello` up; `Ready`/`Error` down, then events for
//!   as long as the session lasts.
//! - **Request streams** — one bi-stream per RPC. Read a `Request`, write a
//!   `Response`, close. They are cheap, so nothing is multiplexed by hand and
//!   there is no request-id field to correlate.
//!
//! ## Where the device identity comes from
//!
//! [`Connection::remote_id`], and nowhere else. iroh proved it during the TLS
//! handshake. The `Hello` body has no field for it, on purpose.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use he_proto::io::{read_frame, write_frame};
use he_proto::rpc::{Auth, ErrorCode, Hello, ProtocolError, Ready, Request, Response};
use he_proto::{Channel, FrameError, InviteLink, Member, Message, NetworkConfig, ServerFrame};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

use crate::error::{Result, ServerError};
use crate::presence::Presence;
use crate::rpc::{self, Outgoing, Revoked, to_protocol_error};
use crate::{Server, User};

/// Caps that apply before anybody has proved who they are.
///
/// An `EndpointId` costs nothing to generate, so "one connection per device"
/// is not a limit. These are global, and they exist so that a stranger with a
/// script cannot make the host's machine unusable (PLAN §11).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Live connections, authenticated or not.
    pub max_connections: usize,
    /// Handshakes in flight at once. Lower than `max_connections` because a
    /// handshake can cost an Argon2id hash, which is ~100 ms of CPU on
    /// purpose.
    pub max_handshakes: usize,
    /// How long a connection may sit without completing its handshake.
    pub handshake_timeout: Duration,
    /// Request streams a single session may have in flight.
    pub max_requests_per_session: usize,
    /// Events buffered for a session that is reading slowly. Past this it is
    /// told it lagged rather than being allowed to stall every other member.
    pub event_buffer: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 256,
            max_handshakes: 8,
            handshake_timeout: Duration::from_secs(30),
            max_requests_per_session: 16,
            event_buffer: 256,
        }
    }
}

/// One authenticated connection: who it is, and which device it is on.
///
/// Produced only by a successful handshake. There is no constructor that skips
/// authentication, which is the invariant the whole module rests on.
#[derive(Debug, Clone)]
pub struct Session {
    pub user: User,
    /// Read from the iroh connection. Never from a message body.
    pub endpoint_id: EndpointId,
    id: u64,
}

impl Session {
    /// Identifies this connection within the process, so a broadcast event can
    /// tell "the connection that sent this" from every other one. Not an
    /// identity — it is not stable, not secret, and never leaves the process.
    pub fn connection_id(&self) -> u64 {
        self.id
    }
}

/// Something that happened in the space, on its way to every live session.
///
/// Two variants carry an origin connection, for opposite reasons: a `Message`
/// so its sender — and only its sender — gets its nonce back, and a `Typing`
/// so its sender is the one connection that does *not* get it.
#[derive(Debug, Clone)]
enum Fanout {
    Message {
        message: Message,
        /// `(connection id, nonce)` of whoever sent it. Only that connection
        /// gets the nonce; everyone else gets the same event without one,
        /// because only the sender has an optimistic bubble to reconcile
        /// (PLAN §9).
        origin: (u64, String),
    },
    Edited {
        message: Message,
    },
    Deleted {
        id: String,
        channel_id: String,
        deleted_at: i64,
    },
    Presence {
        user_id: String,
        online: bool,
        /// The connection whose arrival caused it, when one did. That
        /// connection already knows it is online; a second device on the same
        /// account does not, and still gets told.
        origin: Option<u64>,
    },
    Typing {
        channel_id: String,
        user_id: String,
        username: String,
        /// The connection that is typing. It already knows.
        origin: u64,
    },
    Channels {
        channels: Vec<Channel>,
    },
    Members {
        members: Vec<Member>,
    },
    /// Aimed at particular sessions, and delivered to nobody else. The pump
    /// closes the connection after writing it.
    Revoked {
        target: Revoked,
    },
}

/// The session an event is being rendered for.
///
/// Everything in here came off the iroh connection or out of the handshake —
/// never out of a message body (PLAN §11).
#[derive(Debug, Clone, Copy)]
struct Recipient<'a> {
    connection_id: u64,
    user_id: &'a str,
    endpoint_id: &'a str,
}

impl Revoked {
    /// Whether a revocation is about this session.
    fn matches(&self, to: Recipient<'_>) -> bool {
        match self {
            // A kick or a ban: every machine this account is signed in on.
            Self::Account { user_id } => user_id == to.user_id,
            // One enrolment: this account, on this machine. A second account
            // on the same key is a different enrolment and stays connected.
            Self::Device {
                user_id,
                endpoint_id,
            } => user_id == to.user_id && endpoint_id == to.endpoint_id,
        }
    }
}

impl Fanout {
    /// The frame one session should see, or `None` if this event is not for
    /// that session at all.
    fn frame_for(self, to: Recipient<'_>) -> Option<ServerFrame> {
        let connection_id = to.connection_id;
        Some(match self {
            Self::Message {
                message,
                origin: (origin, nonce),
            } => ServerFrame::Message {
                message,
                nonce: (origin == connection_id).then_some(nonce),
            },
            Self::Channels { channels } => ServerFrame::Channels { channels },
            Self::Members { members } => ServerFrame::Members { members },
            Self::Revoked { target } if !target.matches(to) => return None,
            Self::Revoked { .. } => ServerFrame::Revoked,
            Self::Edited { message } => ServerFrame::Edited { message },
            Self::Deleted {
                id,
                channel_id,
                deleted_at,
            } => ServerFrame::Deleted {
                id,
                channel_id,
                deleted_at,
            },
            Self::Presence { origin, .. } if origin == Some(connection_id) => return None,
            Self::Presence {
                user_id, online, ..
            } => ServerFrame::Presence { user_id, online },
            Self::Typing { origin, .. } if origin == connection_id => return None,
            Self::Typing {
                channel_id,
                user_id,
                username,
                ..
            } => ServerFrame::Typing {
                channel_id,
                user_id,
                username,
            },
        })
    }
}

impl Outgoing {
    /// Stamps an event with the connection that caused it.
    ///
    /// The origin is read from the [`Session`], which came from the iroh
    /// connection — never from anything the client put in the request.
    async fn into_fanout(self, server: &Server, session: &Session) -> Result<Fanout> {
        Ok(match self {
            Self::Message { message, nonce } => Fanout::Message {
                message,
                origin: (session.id, nonce),
            },
            Self::Edited { message } => Fanout::Edited { message },
            Self::Deleted {
                id,
                channel_id,
                deleted_at,
            } => Fanout::Deleted {
                id,
                channel_id,
                deleted_at,
            },
            Self::Typing { channel_id } => Fanout::Typing {
                channel_id,
                user_id: session.user.id.clone(),
                username: session.user.username.clone(),
                origin: session.id,
            },
            // Read here rather than assembled by the handler, so the list that
            // goes out is the one in the database *after* the change rather
            // than one the caller built from what it thought it did.
            Self::Channels => Fanout::Channels {
                channels: server.channels().await?,
            },
            Self::Members => Fanout::Members {
                members: server.members().await?,
            },
            Self::Revoked(target) => Fanout::Revoked { target },
        })
    }
}

/// The `hit-enter/0` protocol handler.
#[derive(Debug, Clone)]
pub struct ChatProtocol {
    server: Arc<Server>,
    events: broadcast::Sender<Fanout>,
    limits: Limits,
    connections: Arc<Semaphore>,
    handshakes: Arc<Semaphore>,
    next_id: Arc<AtomicU64>,
    /// Who is connected, and who is typing. Live state, never persisted —
    /// see [`crate::presence`].
    presence: Arc<Presence>,
}

impl ChatProtocol {
    pub fn new(server: Arc<Server>) -> Self {
        Self::with_limits(server, Limits::default())
    }

    pub fn with_limits(server: Arc<Server>, limits: Limits) -> Self {
        let (events, _) = broadcast::channel(limits.event_buffer);
        Self {
            server,
            events,
            limits,
            connections: Arc::new(Semaphore::new(limits.max_connections)),
            handshakes: Arc::new(Semaphore::new(limits.max_handshakes)),
            next_id: Arc::new(AtomicU64::new(1)),
            presence: Arc::new(Presence::default()),
        }
    }

    pub fn server(&self) -> &Arc<Server> {
        &self.server
    }

    /// Runs one connection to completion. Returns `Err` only for faults worth
    /// a log line — a refused handshake is a normal outcome, not an error.
    async fn run(&self, conn: Connection) -> std::result::Result<(), ConnectionFailed> {
        let Ok(_slot) = self.connections.clone().try_acquire_owned() else {
            // Full. Closing immediately is kinder than a connection that hangs
            // and kinder to the host than accepting without bound.
            tracing::warn!("connection refused: at capacity");
            conn.close(BUSY_CODE.into(), b"server at capacity");
            return Ok(());
        };

        let endpoint_id = conn.remote_id();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        // Subscribed before the handshake finishes, so a message posted while
        // this client was still logging in is delivered rather than dropped
        // into the gap between "read the channel list" and "start listening".
        let events = self.events.subscribe();

        // The client opens the control stream first and holds it open.
        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|err| ConnectionFailed(err.to_string()))?;

        let session = {
            let _handshake_slot = self.handshake_slot(&mut send).await?;
            match tokio::time::timeout(
                self.limits.handshake_timeout,
                self.handshake(&endpoint_id, id, &mut send, &mut recv),
            )
            .await
            {
                Ok(Ok(session)) => session,
                // Refused: the client has been told why, and that is the end
                // of the connection, not an error for the host to read about.
                Ok(Err(_refused)) => {
                    conn.close(REFUSED_CODE.into(), b"handshake refused");
                    return Ok(());
                }
                Err(_elapsed) => {
                    tracing::debug!(%endpoint_id, "handshake timed out");
                    conn.close(TIMEOUT_CODE.into(), b"handshake timed out");
                    return Ok(());
                }
            }
        };

        tracing::info!(
            user = %session.user.username,
            %endpoint_id,
            connection = id,
            "session established"
        );

        // Announced *after* the subscription above, so this session is not
        // told about its own arrival, and before any request is served, so a
        // member is online from the first frame they could act on.
        if self.presence.join(&session.user.id) {
            self.fan_out(Fanout::Presence {
                user_id: session.user.id.clone(),
                online: true,
                origin: Some(id),
            });
        }

        // The control stream is now one-way: events, until the session ends.
        let pump = tokio::spawn(pump_events(
            conn.clone(),
            send,
            events,
            id,
            session.user.id.clone(),
            endpoint_id.to_string(),
        ));
        let result = self.serve_requests(&conn, &session).await;
        pump.abort();

        if self.presence.leave(&session.user.id, id) {
            // No origin: the connection that caused this is the one that just
            // went away, and there is nobody left on it to exclude.
            self.fan_out(Fanout::Presence {
                user_id: session.user.id.clone(),
                online: false,
                origin: None,
            });
        }

        tracing::info!(user = %session.user.username, connection = id, "session ended");
        result
    }

    /// Hands an event to every live session.
    ///
    /// A send error means nobody is subscribed, which includes the ordinary
    /// case where this is the only connection. Nothing here is stored by the
    /// fan-out, so there is nothing to lose by it going nowhere.
    fn fan_out(&self, event: Fanout) {
        let _ = self.events.send(event);
    }

    /// Waits for a handshake slot, telling the client to back off rather than
    /// queueing it behind an unbounded number of Argon2id hashes.
    async fn handshake_slot(
        &self,
        send: &mut SendStream,
    ) -> std::result::Result<OwnedSemaphorePermit, ConnectionFailed> {
        match self.handshakes.clone().try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(_) => {
                send_final(
                    send,
                    &ServerFrame::Error(ProtocolError::rate_limited(
                        HANDSHAKE_BACKOFF.as_secs().max(1),
                    )),
                )
                .await;
                Err(ConnectionFailed("handshake queue full".into()))
            }
        }
    }

    /// Reads the `Hello`, authenticates it, and answers `Ready` or `Error`.
    ///
    /// `endpoint_id` comes from the connection. It is an argument rather than
    /// something read here so that there is exactly one line in the whole
    /// server that decides what a device is, and it is in [`Self::run`].
    async fn handshake(
        &self,
        endpoint_id: &EndpointId,
        id: u64,
        send: &mut SendStream,
        recv: &mut RecvStream,
    ) -> std::result::Result<Session, Refused> {
        let hello: Hello = match read_frame(recv).await {
            Ok(hello) => hello,
            Err(err) => return self.refuse(send, protocol_error_for(&err)).await,
        };

        if hello.proto != he_proto::PROTOCOL_VERSION {
            // The ALPN already refuses an incompatible peer; this catches a
            // same-ALPN skew during development, where it is a clearer message
            // than a parse failure three frames later.
            tracing::debug!(%endpoint_id, proto = hello.proto, "protocol version mismatch");
            return self
                .refuse(send, ProtocolError::new(ErrorCode::Protocol))
                .await;
        }

        let authenticated = match hello.auth {
            Auth::Device { username } => self
                .server
                .authenticate_device(endpoint_id, username.as_deref())
                .await
                .map(|(user, _device)| user),
            Auth::Password { username, password } => self
                .server
                .login(endpoint_id, &username, &password, None)
                .await
                .map(|(user, _device)| user),
            Auth::Register {
                invite,
                username,
                password,
            } => self
                .server
                .register(endpoint_id, &invite, &username, &password, None)
                .await
                .map(|(user, _device)| user),
        };

        let user = match authenticated {
            Ok(user) => user,
            Err(err) => {
                // At debug, and by code only: which of "no such account" and
                // "wrong password" it was must not reach the peer, and the
                // password must not reach the log.
                tracing::debug!(%endpoint_id, error = %err, "handshake refused");
                let protocol = to_protocol_error(&err);
                return self.refuse(send, protocol).await;
            }
        };

        let ready = match self.ready_for(&user).await {
            Ok(ready) => ready,
            Err(err) => {
                let protocol = to_protocol_error(&err);
                return self.refuse(send, protocol).await;
            }
        };

        if write_frame(send, &ServerFrame::Ready(ready)).await.is_err() {
            return Err(Refused);
        }

        Ok(Session {
            user,
            endpoint_id: *endpoint_id,
            id,
        })
    }

    async fn ready_for(&self, user: &User) -> Result<Ready> {
        // The reader of this frame is online by the time they read it, so say
        // so — the alternative is a list that includes you only when a second
        // device of yours happens to be connected, which is the sort of
        // inconsistency a client would have to paper over for ever.
        let mut online = self.presence.online();
        if !online.contains(&user.id) {
            online.push(user.id.clone());
        }

        Ok(Ready {
            server_name: self.server.name().to_owned(),
            user: user.as_member(),
            channels: self.server.channels().await?,
            members: self.server.members().await?,
            // A snapshot; `Presence` events keep it current from here.
            online,
            // Always true here: every path into a session enrols the device,
            // which is what makes the next connection passwordless.
            enrolled: true,
        })
    }

    async fn refuse<T>(
        &self,
        send: &mut SendStream,
        error: ProtocolError,
    ) -> std::result::Result<T, Refused> {
        send_final(send, &ServerFrame::Error(error)).await;
        Err(Refused)
    }

    /// Accepts request streams until the connection ends.
    async fn serve_requests(
        &self,
        conn: &Connection,
        session: &Session,
    ) -> std::result::Result<(), ConnectionFailed> {
        let in_flight = Arc::new(Semaphore::new(self.limits.max_requests_per_session));

        loop {
            let (send, recv) = match conn.accept_bi().await {
                Ok(streams) => streams,
                // Every way a connection ends arrives here, including the
                // ordinary one where the client closed it.
                Err(err) => {
                    tracing::debug!(connection = session.id, %err, "connection closed");
                    return Ok(());
                }
            };

            let Ok(permit) = in_flight.clone().acquire_owned().await else {
                return Ok(());
            };

            let this = self.clone();
            let session = session.clone();
            tokio::spawn(async move {
                this.serve_one_request(send, recv, &session).await;
                drop(permit);
            });
        }
    }

    async fn serve_one_request(
        &self,
        mut send: SendStream,
        mut recv: RecvStream,
        session: &Session,
    ) {
        let response = match read_frame::<_, Request>(&mut recv).await {
            Ok(request) => {
                // Throttled before it is handled, because a typing event costs
                // the *server* a broadcast to every member and costs the
                // client nothing to send. A client is the one thing that
                // cannot be trusted to rate-limit itself.
                if let Request::Typing { .. } = &request
                    && !self.presence.may_type(session.id)
                {
                    let _ = write_frame(&mut send, &Response::Ok).await;
                    let _ = send.finish();
                    return;
                }

                let handled = rpc::handle(&self.server, session, request).await;
                for event in handled.fan_out {
                    match event.into_fanout(&self.server, session).await {
                        Ok(fanout) => self.fan_out(fanout),
                        // The change itself succeeded; only the announcement
                        // of it failed. Refusing the request now would be a
                        // lie, so the client is told it worked and everyone
                        // else finds out on their next connect.
                        Err(err) => {
                            tracing::error!(%err, "could not announce a change");
                        }
                    }
                }
                handled.response
            }
            // The peer opened a stream and closed it without asking anything.
            Err(FrameError::Closed) => return,
            Err(err) => {
                tracing::debug!(connection = session.id, %err, "unreadable request");
                Response::Error(protocol_error_for(&err))
            }
        };

        if write_frame(&mut send, &response).await.is_ok() {
            let _ = send.finish();
        }
    }
}

impl ProtocolHandler for ChatProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        // A fault on one connection is that connection's problem. Returning
        // `Err` here would log a warning per dropped wifi packet.
        if let Err(err) = self.run(connection).await {
            tracing::debug!(error = %err.0, "connection ended");
        }
        Ok(())
    }
}

/// Writes broadcast events to one session's control stream until the stream
/// dies or the session ends.
async fn pump_events(
    conn: Connection,
    mut send: SendStream,
    mut events: broadcast::Receiver<Fanout>,
    connection_id: u64,
    user_id: String,
    endpoint_id: String,
) {
    let to = Recipient {
        connection_id,
        user_id: &user_id,
        endpoint_id: &endpoint_id,
    };

    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Closed) => return,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                // This client was too slow and events were dropped. Not a
                // reason to disconnect it: `resume` closes the gap on the next
                // reconnect, and a `backfill` closes it before then.
                tracing::warn!(connection = connection_id, missed, "session lagged");
                continue;
            }
        };

        // `None` means this event was never for this session — a typing
        // indicator going back to the person who is typing, or a revocation
        // aimed at somebody else.
        let Some(frame) = event.frame_for(to) else {
            continue;
        };

        if matches!(frame, ServerFrame::Revoked) {
            // The last frame this connection will carry, so it has to be
            // flushed before the connection is dropped — otherwise the QUIC
            // close discards it and being kicked arrives as "connection lost",
            // which is exactly the thing the client must not retry through.
            tracing::info!(connection = connection_id, %user_id, "session revoked");
            send_final(&mut send, &frame).await;
            conn.close(REVOKED_CODE.into(), b"revoked");
            return;
        }

        if write_frame(&mut send, &frame).await.is_err() {
            return;
        }
    }
}

/// Binds an iroh endpoint carrying this server's identity.
///
/// Relays and discovery come from `config` rather than a preset, because a
/// host runs a client out of the same process and both have to agree about
/// which relays exist (PLAN §4). The defaults are n0's — a documented,
/// replaceable convenience, never a dependency on us.
pub async fn bind_endpoint(server: &Server, config: &NetworkConfig) -> Result<Endpoint> {
    let builder = config
        .apply(Endpoint::builder(iroh::endpoint::presets::Empty))
        .map_err(|err| ServerError::Endpoint(err.to_string()))?;
    builder
        .secret_key(server.identity().secret_key().clone())
        .alpns(vec![he_proto::ALPN.to_vec()])
        .bind()
        .await
        .map_err(|err| ServerError::Endpoint(err.to_string()))
}

/// Serves `hit-enter/0` on an endpoint you already built.
///
/// Separate from [`serve`] because a test binds two endpoints on loopback with
/// no relay and no discovery, and because M4's hosting settings will want the
/// same seam.
pub fn serve_on(endpoint: Endpoint, server: Arc<Server>, limits: Limits) -> Router {
    Router::builder(endpoint)
        .accept(he_proto::ALPN, ChatProtocol::with_limits(server, limits))
        .spawn()
}

/// Binds an endpoint and serves the protocol on it. The common case.
pub async fn serve(server: Arc<Server>, config: &NetworkConfig) -> Result<Router> {
    let endpoint = bind_endpoint(&server, config).await?;
    Ok(serve_on(endpoint, server, Limits::default()))
}

/// The invite link for a space, minted from the address the endpoint knows it
/// has *right now*.
///
/// The hints — home relay, direct addresses — are what make a first connection
/// fast, and they go stale. The `EndpointId` inside never does, so a link with
/// stale hints still resolves through discovery; it just takes longer. Mint a
/// fresh one rather than storing this string anywhere.
///
/// Call [`Endpoint::online`] first if the link is going to a different
/// network: before the endpoint has reached a relay there is no relay in its
/// address, and a link without one only works on this LAN.
pub fn invite_link(endpoint: &Endpoint, code: Option<String>) -> InviteLink {
    InviteLink::new(endpoint.addr(), code)
}

/// A handshake that was answered with an `Error` frame. Carries nothing: the
/// client has already been told, and the reason was logged where it belongs.
struct Refused;

/// A connection that ended badly enough to mention.
#[derive(Debug)]
struct ConnectionFailed(String);

/// Writes the last frame a stream will carry, and waits for the peer to
/// acknowledge it.
///
/// Without the wait this is a race the server loses. Returning from the
/// handler drops the connection, and closing a QUIC connection discards stream
/// data the peer has not acknowledged yet — so "your password is wrong"
/// arrives at the client as "connection lost", and the user is told to check
/// their network instead of their password.
///
/// Bounded, because a peer that never acknowledges must not be able to pin a
/// connection slot by refusing to listen to its own rejection.
async fn send_final(send: &mut SendStream, frame: &ServerFrame) {
    if write_frame(send, frame).await.is_err() {
        return;
    }
    let _ = send.finish();
    let _ = tokio::time::timeout(FLUSH_TIMEOUT, send.stopped()).await;
}

/// QUIC application close codes. Arbitrary, and only ever read by a human
/// looking at why a connection went away.
const BUSY_CODE: u32 = 1;
const TIMEOUT_CODE: u32 = 2;
const REFUSED_CODE: u32 = 3;
const REVOKED_CODE: u32 = 4;

/// How long to wait for a peer to acknowledge a final frame before giving up
/// on it. See [`send_final`].
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// What to tell a client that arrived while every handshake slot was busy.
const HANDSHAKE_BACKOFF: Duration = Duration::from_secs(5);

/// Maps a framing failure onto a wire error.
fn protocol_error_for(err: &FrameError) -> ProtocolError {
    match err {
        // Asking for more than a megabyte is over the line, but it is the
        // client's line to have crossed and it should be told which one.
        FrameError::TooLarge { .. } => ProtocolError::new(ErrorCode::Invalid),
        _ => ProtocolError::new(ErrorCode::Protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session to render events for. The two ids are what a revocation is
    /// matched against, so tests that do not care still have to name them.
    fn to(connection_id: u64) -> Recipient<'static> {
        Recipient {
            connection_id,
            user_id: "u1",
            endpoint_id: "device-1",
        }
    }

    fn a_message() -> Message {
        Message {
            id: "m1".into(),
            channel_id: "c1".into(),
            author_id: "u1".into(),
            author_name: "justin".into(),
            content: "hi".into(),
            edited_at: None,
            deleted_at: None,
        }
    }

    #[test]
    fn only_the_sender_gets_its_nonce_back() {
        // The nonce is how *one* client reconciles its optimistic bubble.
        // Sending it to everybody would have every other client swap in a
        // message it never composed.
        let event = Fanout::Message {
            message: a_message(),
            origin: (7, "n1".into()),
        };

        let Some(ServerFrame::Message { nonce, .. }) = event.clone().frame_for(to(7)) else {
            panic!("the sender gets the message");
        };
        assert_eq!(nonce.as_deref(), Some("n1"));

        let Some(ServerFrame::Message { nonce, .. }) = event.frame_for(to(8)) else {
            panic!("everyone else gets the message too");
        };
        assert_eq!(nonce, None);
    }

    #[test]
    fn nobody_is_told_that_they_themselves_are_typing() {
        // The sender already knows, and an indicator for yourself is the kind
        // of bug that only shows up with one window open.
        let event = Fanout::Typing {
            channel_id: "c1".into(),
            user_id: "u1".into(),
            username: "justin".into(),
            origin: 7,
        };
        assert!(event.clone().frame_for(to(7)).is_none());
        assert!(matches!(
            event.frame_for(to(8)),
            Some(ServerFrame::Typing { .. })
        ));
    }

    #[test]
    fn a_session_is_not_told_that_it_itself_came_online() {
        // It knows. But a *second* device on the same account does not, and
        // greying out a member who is reading on their phone would be wrong.
        let event = Fanout::Presence {
            user_id: "u1".into(),
            online: true,
            origin: Some(7),
        };
        assert!(event.clone().frame_for(to(7)).is_none());
        assert!(matches!(
            event.frame_for(to(8)),
            Some(ServerFrame::Presence { online: true, .. })
        ));
    }

    #[test]
    fn a_revocation_reaches_the_sessions_it_is_about_and_no_others() {
        // Getting this wrong in either direction is bad in a different way:
        // too narrow and a kicked member keeps a live session, too wide and
        // revoking one laptop signs the whole space out.
        let kicked = Fanout::Revoked {
            target: Revoked::Account {
                user_id: "u1".into(),
            },
        };
        assert!(matches!(
            kicked.clone().frame_for(to(7)),
            Some(ServerFrame::Revoked)
        ));
        assert!(
            kicked
                .frame_for(Recipient {
                    connection_id: 8,
                    user_id: "u2",
                    endpoint_id: "device-1",
                })
                .is_none(),
            "banning one account must not disconnect another on the same machine"
        );

        let one_device = Fanout::Revoked {
            target: Revoked::Device {
                user_id: "u1".into(),
                endpoint_id: "device-1".into(),
            },
        };
        assert!(matches!(
            one_device.clone().frame_for(to(7)),
            Some(ServerFrame::Revoked)
        ));
        assert!(
            one_device
                .frame_for(Recipient {
                    connection_id: 9,
                    user_id: "u1",
                    endpoint_id: "device-2",
                })
                .is_none(),
            "revoking one machine must leave the member's other machines alone"
        );
    }

    #[test]
    fn presence_and_deletions_reach_every_session_alike() {
        // Neither has an origin to exclude: a member going offline is news to
        // everyone, and a withdrawn message has to leave every screen — the
        // sender's included, because its own optimistic state was the text.
        for event in [
            Fanout::Presence {
                user_id: "u1".into(),
                online: false,
                origin: None,
            },
            Fanout::Deleted {
                id: "m1".into(),
                channel_id: "c1".into(),
                deleted_at: 1_700_000_000,
            },
            Fanout::Edited {
                message: a_message(),
            },
        ] {
            assert!(event.clone().frame_for(to(7)).is_some());
            assert!(event.frame_for(to(8)).is_some());
        }
    }

    #[test]
    fn limits_are_small_enough_to_matter() {
        let limits = Limits::default();
        assert!(
            limits.max_handshakes < limits.max_connections,
            "a handshake can cost 100ms of Argon2id; queueing as many of \
             those as there are connections is a free way to pin the host's CPU"
        );
        assert!(limits.handshake_timeout >= Duration::from_secs(5));
    }
}
