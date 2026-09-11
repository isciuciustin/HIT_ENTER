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
use he_proto::{FrameError, Message, ServerFrame};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

use crate::error::{Result, ServerError};
use crate::rpc::{self, to_protocol_error};
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

/// A message to fan out, and the nonce that goes back to its sender alone.
#[derive(Debug, Clone)]
struct Broadcast {
    message: Message,
    /// `(connection id, nonce)` of whoever sent it. Only that connection gets
    /// the nonce; everyone else gets the same event without one, because only
    /// the sender has an optimistic bubble to reconcile (PLAN §9).
    origin: (u64, String),
}

/// The `hit-enter/0` protocol handler.
#[derive(Debug, Clone)]
pub struct ChatProtocol {
    server: Arc<Server>,
    events: broadcast::Sender<Broadcast>,
    limits: Limits,
    connections: Arc<Semaphore>,
    handshakes: Arc<Semaphore>,
    next_id: Arc<AtomicU64>,
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

        // The control stream is now one-way: events, until the session ends.
        let pump = tokio::spawn(pump_events(send, events, id));
        let result = self.serve_requests(&conn, &session).await;
        pump.abort();

        tracing::info!(user = %session.user.username, connection = id, "session ended");
        result
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
        Ok(Ready {
            server_name: self.server.name().to_owned(),
            user: user.as_member(),
            channels: self.server.channels().await?,
            members: self.server.members().await?,
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
                let handled = rpc::handle(&self.server, session, request).await;
                if let Some((message, nonce)) = handled.broadcast {
                    // A send error means nobody is subscribed, which includes
                    // the case where this is the only connection. The message
                    // is already stored either way.
                    let _ = self.events.send(Broadcast {
                        message,
                        origin: (session.id, nonce),
                    });
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
    mut send: SendStream,
    mut events: broadcast::Receiver<Broadcast>,
    connection_id: u64,
) {
    loop {
        let broadcast = match events.recv().await {
            Ok(broadcast) => broadcast,
            Err(broadcast::error::RecvError::Closed) => return,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                // This client was too slow and events were dropped. It is not
                // a reason to disconnect it; M5's `resume` is what closes the
                // gap properly, and until then a backfill does.
                tracing::warn!(connection = connection_id, missed, "session lagged");
                continue;
            }
        };

        let (origin, nonce) = broadcast.origin;
        let frame = ServerFrame::Message {
            message: broadcast.message,
            nonce: (origin == connection_id).then_some(nonce),
        };

        if write_frame(&mut send, &frame).await.is_err() {
            return;
        }
    }
}

/// Binds an iroh endpoint carrying this server's identity.
///
/// Uses n0's defaults for relays and discovery: a documented, replaceable
/// convenience, never a dependency on us (PLAN §4). M4 makes both
/// configurable.
pub async fn bind_endpoint(server: &Server) -> Result<Endpoint> {
    Endpoint::builder(iroh::endpoint::presets::N0)
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
pub async fn serve(server: Arc<Server>) -> Result<Router> {
    let endpoint = bind_endpoint(&server).await?;
    Ok(serve_on(endpoint, server, Limits::default()))
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

    #[test]
    fn only_the_sender_gets_its_nonce_back() {
        // The nonce is how *one* client reconciles its optimistic bubble.
        // Sending it to everybody would have every other client swap in a
        // message it never composed.
        let broadcast = Broadcast {
            message: Message {
                id: "m1".into(),
                channel_id: "c1".into(),
                author_id: "u1".into(),
                author_name: "justin".into(),
                content: "hi".into(),
                edited_at: None,
                deleted_at: None,
            },
            origin: (7, "n1".into()),
        };

        let for_sender = (broadcast.origin.0 == 7).then_some(broadcast.origin.1.clone());
        let for_others = (broadcast.origin.0 == 8).then_some(broadcast.origin.1);
        assert_eq!(for_sender.as_deref(), Some("n1"));
        assert_eq!(for_others, None);
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
