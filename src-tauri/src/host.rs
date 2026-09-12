//! Hosting a space from the desktop app.
//!
//! This is the M4 switch: the same `he-server` library `he-serverd` runs,
//! opened on a `server.db` in the app's data directory and served on its own
//! iroh endpoint.
//!
//! **Two endpoints, one process, and that is deliberate.** The space has its
//! own `EndpointId` — the one invite links point at, the one that must survive
//! reinstalling the app — and the host's client keeps the device key it had
//! before it ever hosted anything. The host's client then dials the space by
//! its `EndpointId` exactly as a stranger does (PLAN §2.1). There is no
//! loopback shortcut here and there must never be one: iroh resolves a local
//! endpoint to a local path by itself, and one network path means one code
//! path to debug.
//!
//! What that buys, concretely: the owner's own client exercises the handshake,
//! the enrolment, the fan-out and the mirror on every single run. A bug in any
//! of them is the host's bug first.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use he_proto::{InviteLink, NetworkConfig};
use he_server::{Server, ServerError};
use iroh::Endpoint;
use iroh::protocol::Router;

/// The database file inside the app's data directory.
pub const DB_FILE: &str = "server.db";

/// How long to wait for the space's endpoint to reach a relay.
///
/// An invite link minted before this has no relay in it, and can then only be
/// joined from the same LAN — which is exactly the failure that looks like
/// "it works on my machine". Bounded, because hosting on a box with no
/// internet is a supported setup, not an error.
pub const ONLINE_WAIT: Duration = Duration::from_secs(8);

/// A space this machine is serving right now.
pub struct Host {
    server: Arc<Server>,
    endpoint: Endpoint,
    router: Router,
}

impl Host {
    /// Opens `server.db` and serves `hit-enter/0` on the space's endpoint.
    ///
    /// Creates the database on a first run, which also generates the space's
    /// identity — `server_meta.secret_key`, the most security-critical value
    /// in the system. Losing it kills every invite ever issued (PLAN §11).
    pub async fn start(
        data_dir: &Path,
        name_if_new: &str,
        network: &NetworkConfig,
    ) -> he_server::Result<Self> {
        let server = Arc::new(Server::open(&db_path(data_dir), name_if_new).await?);
        let endpoint = he_server::bind_endpoint(&server, network).await?;
        let router = he_server::serve_on(
            endpoint.clone(),
            server.clone(),
            he_server::Limits::default(),
        );

        tracing::info!(
            endpoint_id = %server.endpoint_id(),
            space = %server.name(),
            "hosting"
        );

        Ok(Self {
            server,
            endpoint,
            router,
        })
    }

    pub fn server(&self) -> &Arc<Server> {
        &self.server
    }

    /// The space's address. Not this device's — see the module comment.
    pub fn endpoint_id(&self) -> String {
        self.server.endpoint_id().to_string()
    }

    pub fn space_name(&self) -> &str {
        self.server.name()
    }

    /// The space's endpoint. Cloned rather than borrowed so that a caller can
    /// await on it — [`wait_online`] takes eight seconds, and holding the
    /// lock on the hosting slot for that long would make "close space" hang.
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    /// A join link for this space, with the hints the endpoint knows about
    /// itself *now*.
    ///
    /// Minted fresh every time rather than stored: a relay can change and an
    /// interface can come and go, and a link is only as good as its
    /// `EndpointId` plus whatever hints were true when it was made.
    pub fn invite_link(&self, code: Option<String>) -> InviteLink {
        he_server::invite_link(&self.endpoint, code)
    }

    /// True once the endpoint has a relay to be reached through. A space with
    /// no relay and no public address is reachable on the LAN and nowhere
    /// else, which the UI has to be able to say out loud.
    pub fn is_reachable_remotely(&self) -> bool {
        !self.endpoint.addr().addrs.is_empty()
    }

    /// Stops serving and lets in-flight sessions finish.
    ///
    /// Dropping the endpoint instead looks like a crash on every connected
    /// member until QUIC's idle timeout notices.
    pub async fn shutdown(self) {
        if let Err(err) = self.router.shutdown().await {
            tracing::warn!(%err, "the space did not shut down cleanly");
        }
        self.endpoint.close().await;
        tracing::info!("stopped hosting");
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the server, which owns the secret key (PLAN §11).
        f.debug_struct("Host")
            .field("endpoint_id", &self.endpoint_id())
            .field("space", &self.space_name())
            .finish_non_exhaustive()
    }
}

/// Waits until the space is reachable from outside this network, or gives up.
/// Returns whether it got there.
pub async fn wait_online(endpoint: &Endpoint) -> bool {
    tokio::time::timeout(ONLINE_WAIT, endpoint.online())
        .await
        .is_ok()
}

/// The owner account of a space, if one has been created.
///
/// Every space has exactly one, made locally by whoever created it — an invite
/// cannot gate the first account, so it never crosses the network (PLAN §11).
pub async fn owner_of(server: &Server) -> he_server::Result<Option<he_server::User>> {
    for member in server.members().await? {
        if member.is_owner {
            return server.user_by_id(&member.id).await;
        }
    }
    Ok(None)
}

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_FILE)
}

/// Whether this machine has a space at all — the database exists, whether or
/// not it is currently being served.
pub fn space_exists(data_dir: &Path) -> bool {
    db_path(data_dir).exists()
}

/// Translates a server error for the frontend.
///
/// `OwnerExists` is the one worth naming: it is what "create a space" hits
/// when the database is already there, and the answer is to log in, not to
/// try again.
pub fn describe(err: &ServerError) -> String {
    match err {
        ServerError::OwnerExists => {
            "this machine already hosts a space — open it instead of creating one".to_string()
        }
        other => other.to_string(),
    }
}
