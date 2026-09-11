//! What the desktop app owns: one device key, one mirror, and a live session
//! per connected server.
//!
//! There is deliberately no chat logic here. This crate holds the window and
//! the bridge; `he-client` holds the protocol. What lives in this file is the
//! part that genuinely belongs to a *desktop app* — where the data directory
//! is, which servers are currently connected, and how a background task gets
//! an event onto the screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use he_client::{Client, DeviceIdentity, Mirror, Session};
use tauri::async_runtime::JoinHandle;
use tokio::sync::RwLock;

/// One connected server.
pub struct Live {
    pub session: Arc<Session>,
    /// The task forwarding this session's events into the UI. Spawned on
    /// Tauri's runtime rather than tokio's directly, because that is the one
    /// the app is actually running on. Aborted when the server is
    /// disconnected, so a stale pump cannot keep writing to a mirror for a
    /// space the user has left.
    pub pump: JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// The whole application, as the Tauri commands see it.
pub struct App {
    /// This machine's identity. A server enrols the public half, which is why
    /// it has to live on disk and survive a restart (PLAN §3).
    identity_id: String,
    client: Client,
    mirror: Mirror,
    sessions: RwLock<HashMap<String, Live>>,
}

impl App {
    /// Loads (or creates) everything in `data_dir` and binds an iroh endpoint.
    pub async fn start(data_dir: &Path) -> he_client::Result<Self> {
        std::fs::create_dir_all(data_dir)?;

        let identity = DeviceIdentity::load_or_create(&data_dir.join("device.key"))?;
        let identity_id = identity.endpoint_id().to_string();
        let mirror = Mirror::open(&data_dir.join("mirror.db")).await?;
        let client = Client::bind(&identity).await?;

        tracing::info!(device_id = %identity_id, data_dir = %data_dir.display(), "client ready");

        Ok(Self {
            identity_id,
            client,
            mirror,
            sessions: RwLock::new(HashMap::new()),
        })
    }

    /// This device's public identity. Safe to show: it is what a server owner
    /// sees in their device list, and what they revoke by.
    pub fn device_id(&self) -> &str {
        &self.identity_id
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// The live session for a server, if it is connected.
    pub async fn session(&self, endpoint_id: &str) -> Option<Arc<Session>> {
        self.sessions
            .read()
            .await
            .get(endpoint_id)
            .map(|live| live.session.clone())
    }

    /// Registers a live session, replacing and tearing down any previous one
    /// for the same server.
    pub async fn insert_session(&self, endpoint_id: &str, live: Live) {
        // The replaced `Live` aborts its own pump on drop, which is what stops
        // two pumps from writing the same events into the mirror twice.
        self.sessions
            .write()
            .await
            .insert(endpoint_id.to_owned(), live);
    }

    /// Hangs up on a server, if it was connected.
    pub async fn remove_session(&self, endpoint_id: &str) -> bool {
        match self.sessions.write().await.remove(endpoint_id) {
            Some(live) => {
                live.session.disconnect();
                true
            }
            None => false,
        }
    }

    /// Closes everything, for shutdown.
    pub async fn disconnect_all(&self) {
        for (_, live) in self.sessions.write().await.drain() {
            live.session.disconnect();
        }
        self.client.shutdown().await;
    }
}

/// Where the app keeps its device key and its mirror.
///
/// `HE_DATA_DIR` overrides it, which is the only way to run two instances on
/// one machine — and running two is how you watch a conversation happen
/// without owning two computers. Two instances sharing a directory would share
/// a device key, which means the server would see one device, not two.
pub fn data_dir(fallback: PathBuf) -> PathBuf {
    match std::env::var_os("HE_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => fallback,
    }
}
