//! Dialing a server and holding the session open.
//!
//! A server is addressed by its `EndpointId` — a public key, not an IP and not
//! a name. Its address can change underneath an open connection and iroh will
//! re-path rather than drop it (PLAN §4), which is why nothing here caches a
//! socket address or retries on one.
//!
//! **The host's own client uses this same code.** There is no loopback
//! shortcut and there must never be one: one network path means one code path
//! to debug (PLAN §2.1).

use he_proto::io::{read_frame, write_frame};
use he_proto::rpc::{Auth, Hello, Ready, Request, Response};
use he_proto::{FrameError, Message, NetworkConfig, ServerFrame, limits};
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::ConnectionPath;
use crate::error::{ClientError, Result};
use crate::keys::DeviceIdentity;

/// How many events may queue for a caller that is not draining them before the
/// reader task starts applying back-pressure to the control stream.
const EVENT_BUFFER: usize = 256;

/// This machine's iroh endpoint. One per process, shared by every session.
#[derive(Debug, Clone)]
pub struct Client {
    endpoint: Endpoint,
}

impl Client {
    /// Binds an endpoint with this device's key and these relay and discovery
    /// settings.
    ///
    /// The settings come from `he_proto::NetworkConfig` rather than from a
    /// preset because a host configures *both* of its endpoints from one
    /// settings file, and the two must not be able to disagree about which
    /// relays exist (PLAN §4).
    pub async fn bind(identity: &DeviceIdentity, config: &NetworkConfig) -> Result<Self> {
        let builder = config
            .apply(Endpoint::builder(iroh::endpoint::presets::Empty))
            .map_err(|err| ClientError::Connect(err.to_string()))?;
        let endpoint = builder
            .secret_key(identity.secret_key().clone())
            .bind()
            .await
            .map_err(|err| ClientError::Connect(err.to_string()))?;
        Ok(Self { endpoint })
    }

    /// Wraps an endpoint the caller built. Used by tests, which bind two
    /// endpoints on loopback with no relay and no discovery.
    pub fn from_endpoint(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// This device's public identity — what a server enrols.
    pub fn endpoint_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// This endpoint's own address, hints included.
    ///
    /// For a client that is only ever dialling out this is a curiosity; for
    /// the same process hosting a space it is what goes into an invite link
    /// (PLAN §5), and it changes as relays and interfaces come and go.
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Waits until this endpoint has reached a relay, or gives up after
    /// `timeout`.
    ///
    /// An [`Self::addr`] read before this has no relay in it, and an invite
    /// link minted from that address can only be joined from the same LAN —
    /// which is the difference between "a friend in another city joins" and
    /// "it worked on my machine".
    pub async fn wait_online(&self, timeout: std::time::Duration) -> bool {
        tokio::time::timeout(timeout, self.endpoint.online())
            .await
            .is_ok()
    }

    /// Dials a server and completes the handshake.
    ///
    /// `addr` may be a bare `EndpointId`, in which case discovery resolves it,
    /// or an [`EndpointAddr`] carrying relay and direct-address hints from an
    /// invite ticket. The hints may be stale; the `EndpointId` never is.
    pub async fn connect(&self, addr: impl Into<EndpointAddr>, auth: Auth) -> Result<Session> {
        let conn = self
            .endpoint
            .connect(addr, he_proto::ALPN)
            .await
            .map_err(|err| ClientError::Connect(err.to_string()))?;
        Session::start(conn, auth).await
    }

    pub async fn shutdown(&self) {
        self.endpoint.close().await;
    }
}

/// One logged-in connection to one server.
///
/// Dropping it drops the connection and the task reading its control stream.
#[derive(Debug)]
pub struct Session {
    conn: Connection,
    ready: Ready,
    /// `None` once [`Session::take_events`] has handed the stream to someone
    /// else — the app shell pumps events into the UI from its own task while
    /// the session itself stays shared and `&self`-only for requests.
    events: Option<mpsc::Receiver<ServerFrame>>,
    reader: JoinHandle<()>,
}

impl Session {
    /// Opens the control stream, sends `Hello`, and waits for `Ready`.
    async fn start(conn: Connection, auth: Auth) -> Result<Self> {
        // The client opens the control stream; the server accepts it first,
        // before any request stream, and that ordering is the handshake.
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|err| ClientError::Transport(err.to_string()))?;

        write_frame(&mut send, &Hello::new(auth)).await?;

        let ready = match read_frame::<_, ServerFrame>(&mut recv).await? {
            ServerFrame::Ready(ready) => ready,
            ServerFrame::Error(err) => return Err(ClientError::Refused(err)),
            ServerFrame::Message { .. } => return Err(ClientError::Unexpected("message")),
        };

        // From here the control stream is one-way: events, until it ends.
        let (tx, events) = mpsc::channel(EVENT_BUFFER);
        let reader = tokio::spawn(read_events(recv, tx));

        Ok(Self {
            conn,
            ready,
            events: Some(events),
            reader,
        })
    }

    /// Everything the server said when it let us in: the space's name, our own
    /// account, its channels and its members.
    pub fn ready(&self) -> &Ready {
        &self.ready
    }

    /// Whether this connection is peer-to-peer or going through a relay.
    ///
    /// Surfaced in the UI rather than hidden (PLAN §6): relayed is slower, not
    /// broken, and a user who can see why understands their network instead of
    /// concluding the app is bad.
    pub fn path(&self) -> ConnectionPath {
        crate::path_of(&self.conn)
    }

    /// The next event from the server, or `None` once the session has ended
    /// or the stream has been taken by [`Session::take_events`].
    pub async fn next_event(&mut self) -> Option<ServerFrame> {
        match self.events.as_mut() {
            Some(events) => events.recv().await,
            None => None,
        }
    }

    /// Takes the event stream out of the session, once.
    ///
    /// Reading events needs `&mut`, and running requests needs only `&`. A UI
    /// wants both at the same time from different tasks, so it takes the
    /// receiver into a pump and shares the rest of the session. Returns `None`
    /// on a second call.
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<ServerFrame>> {
        self.events.take()
    }

    /// Runs one RPC on its own bi-stream.
    ///
    /// Streams are cheap, so there is no request id and nothing to correlate:
    /// the answer to this request is the only thing that will ever be written
    /// on this stream.
    pub async fn request(&self, request: Request) -> Result<Response> {
        request.validate()?;

        let (mut send, mut recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|err| ClientError::Transport(err.to_string()))?;

        write_frame(&mut send, &request).await?;
        let _ = send.finish();

        match read_frame::<_, Response>(&mut recv).await? {
            Response::Error(err) => Err(ClientError::Refused(err)),
            response => Ok(response),
        }
    }

    /// Posts a message and returns the nonce it was tagged with.
    ///
    /// The message itself comes back as a [`ServerFrame::Message`] event
    /// carrying that nonce, which is how the client swaps its optimistic
    /// bubble for the authoritative row (PLAN §9).
    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<String> {
        let nonce = Uuid::now_v7().to_string();
        self.request(Request::Send {
            channel_id: channel_id.to_owned(),
            content: content.to_owned(),
            nonce: nonce.clone(),
        })
        .await?;
        Ok(nonce)
    }

    /// A page of history, newest first, ending just before `before`.
    pub async fn backfill(
        &self,
        channel_id: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>> {
        match self
            .request(Request::Backfill {
                channel_id: channel_id.to_owned(),
                before: before.map(str::to_owned),
                limit,
            })
            .await?
        {
            Response::Messages { messages } => Ok(messages),
            _ => Err(ClientError::Unexpected("response")),
        }
    }

    /// Mints an invite code.
    pub async fn create_invite(
        &self,
        expires_in: Option<u64>,
        max_uses: Option<i64>,
    ) -> Result<String> {
        match self
            .request(Request::Invite {
                expires_in,
                max_uses,
            })
            .await?
        {
            Response::Invite { code, .. } => Ok(code),
            _ => Err(ClientError::Unexpected("response")),
        }
    }

    /// The default page size, so a caller does not have to reach into
    /// `he-proto` for it.
    pub const BACKFILL_LIMIT: u32 = limits::BACKFILL_DEFAULT_LIMIT;

    /// Closes the connection and stops reading events, without consuming the
    /// session — so a shared `Arc<Session>` can be hung up on.
    pub fn disconnect(&self) {
        self.conn.close(0u32.into(), b"bye");
        self.reader.abort();
    }

    /// Closes the connection and stops reading events.
    pub async fn close(self) {
        self.disconnect();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Without this the reader task outlives the session and keeps the
        // connection alive, which on the server side looks like a member who
        // never logs out.
        self.reader.abort();
    }
}

/// Reads the control stream until it ends, forwarding events to the session.
async fn read_events(mut recv: iroh::endpoint::RecvStream, tx: mpsc::Sender<ServerFrame>) {
    loop {
        match read_frame::<_, ServerFrame>(&mut recv).await {
            Ok(frame) => {
                if tx.send(frame).await.is_err() {
                    return; // the session was dropped
                }
            }
            // The ordinary end of a session: the server closed the stream.
            Err(FrameError::Closed) => return,
            Err(err) => {
                tracing::debug!(%err, "control stream ended");
                return;
            }
        }
    }
}
