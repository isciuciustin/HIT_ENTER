//! What the desktop app owns: one device key, one mirror, a live session per
//! connected server, and — when this machine hosts — one space.
//!
//! There is deliberately no chat logic here. This crate holds the window and
//! the bridge; `he-client` holds the protocol and `he-server` holds hosting.
//! What lives in this file is the part that genuinely belongs to a *desktop
//! app* — where the data directory is, which servers are currently connected,
//! and how a background task gets an event onto the screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use he_client::{Client, DeviceIdentity, Mirror, Session};
use tauri::async_runtime::JoinHandle;
use tokio::sync::RwLock;

use crate::host::Host;
use crate::settings::Settings;

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
    data_dir: PathBuf,
    client: Client,
    mirror: Mirror,
    sessions: RwLock<HashMap<String, Live>>,
    /// The space this machine serves, when it is serving one. A *different*
    /// endpoint from `client`, with a different key — see `host.rs`.
    host: RwLock<Option<Host>>,
    settings: RwLock<Settings>,
}

impl App {
    /// Loads (or creates) everything in `data_dir` and binds an iroh endpoint.
    ///
    /// Hosting is started here too when the settings ask for it, so that a
    /// machine which was hosting when it was last closed is hosting again
    /// before the window is on screen — a space that only opens once its owner
    /// clicks something is a space that is down every time they reboot.
    pub async fn start(data_dir: &Path) -> he_client::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let settings = Settings::load(data_dir);

        let identity = DeviceIdentity::load_or_create(&data_dir.join("device.key"))?;
        let identity_id = identity.endpoint_id().to_string();
        let mirror = Mirror::open(&data_dir.join("mirror.db")).await?;
        let client = Client::bind(&identity, &settings.network).await?;

        tracing::info!(
            device_id = %identity_id,
            data_dir = %data_dir.display(),
            network = %settings.network.describe(),
            "client ready"
        );

        let host = if settings.hosting.enabled && crate::host::space_exists(data_dir) {
            match Host::start(data_dir, &settings.hosting.space_name, &settings.network).await {
                Ok(host) => Some(host),
                Err(err) => {
                    // Not fatal. The app is a client first; failing to open
                    // the window because a space would not start would take
                    // away the offline history too.
                    tracing::error!(%err, "could not start hosting");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            identity_id,
            data_dir: data_dir.to_path_buf(),
            client,
            mirror,
            sessions: RwLock::new(HashMap::new()),
            host: RwLock::new(host),
            settings: RwLock::new(settings),
        })
    }

    /// This device's public identity. Safe to show: it is what a server owner
    /// sees in their device list, and what they revoke by.
    pub fn device_id(&self) -> &str {
        &self.identity_id
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
    }

    /// Replaces the settings and writes them to disk.
    ///
    /// Writing first would leave the running app disagreeing with its own
    /// settings file if the write failed; this way a failed write is a failed
    /// change, which is the honest outcome.
    pub async fn put_settings(&self, next: Settings) -> std::io::Result<()> {
        next.save(&self.data_dir)?;
        *self.settings.write().await = next;
        Ok(())
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

    // ---- hosting ----------------------------------------------------------

    /// Runs `f` against the space this machine serves, if it is serving one.
    ///
    /// Synchronous on purpose: the hosting slot is behind a lock that
    /// `start_hosting` and `stop_hosting` need, and awaiting while holding it
    /// would make closing a space wait on whatever the reader was doing. For
    /// anything that has to await, take an owned handle with
    /// [`Self::host_endpoint`] or [`Self::host_server`] and let the lock go.
    pub async fn with_host<T>(&self, f: impl FnOnce(&Host) -> T) -> Option<T> {
        self.host.read().await.as_ref().map(f)
    }

    /// The hosted space's endpoint, cloned so it can be awaited on freely.
    pub async fn host_endpoint(&self) -> Option<iroh::Endpoint> {
        self.with_host(Host::endpoint).await
    }

    /// The hosted space's database handle.
    pub async fn host_server(&self) -> Option<Arc<he_server::Server>> {
        self.with_host(|host| host.server().clone()).await
    }

    pub async fn is_hosting(&self) -> bool {
        self.host.read().await.is_some()
    }

    /// The `EndpointId` of the space this machine serves.
    ///
    /// Note that it is *not* [`Self::device_id`]: the space and the device
    /// have separate keys and separate lifetimes (PLAN §3).
    pub async fn hosted_endpoint_id(&self) -> Option<String> {
        self.with_host(Host::endpoint_id).await
    }

    /// Starts hosting, replacing anything already running.
    pub async fn start_hosting(&self) -> he_server::Result<String> {
        let settings = self.settings().await;
        let host = Host::start(
            &self.data_dir,
            &settings.hosting.space_name,
            &settings.network,
        )
        .await?;
        let endpoint_id = host.endpoint_id();

        if let Some(previous) = self.host.write().await.replace(host) {
            previous.shutdown().await;
        }
        Ok(endpoint_id)
    }

    /// Stops hosting. Returns whether anything was running.
    pub async fn stop_hosting(&self) -> bool {
        match self.host.write().await.take() {
            Some(host) => {
                host.shutdown().await;
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
        // The space goes down last: its members include this machine's own
        // client, and hanging up on them before closing their sessions would
        // show up as a dropped connection rather than a clean goodbye.
        if let Some(host) = self.host.write().await.take() {
            host.shutdown().await;
        }
        self.client.shutdown().await;
    }
}

/// Where the app keeps its device key, its mirror and — if it hosts — its
/// space.
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
