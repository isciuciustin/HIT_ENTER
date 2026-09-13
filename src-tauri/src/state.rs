//! What the desktop app owns: one device key, one mirror, a live session per
//! connected server, and — when this machine hosts — one space.
//!
//! There is deliberately no chat logic here. This crate holds the window and
//! the bridge; `he-client` holds the protocol and `he-server` holds hosting.
//! What lives in this file is the part that genuinely belongs to a *desktop
//! app* — where the data directory is, which servers are currently connected,
//! and how a background task gets an event onto the screen.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use he_client::{Client, DeviceIdentity, Mirror, Session};
use tauri::async_runtime::JoinHandle;
use tokio::sync::RwLock;

use crate::host::Host;
use crate::settings::Settings;

/// One server the app is *supposed* to be connected to.
///
/// Not one connection: the session inside is replaced every time the
/// supervisor reconnects, and is `None` while it is between attempts. That
/// distinction is the whole point — "connected to this space" is a thing the
/// app is responsible for having, and a dropped session is a gap in it rather
/// than the end of it (see `session.rs`).
pub struct Live {
    /// The current connection, or `None` while reconnecting.
    session: Mutex<Option<Arc<Session>>>,
    /// Who has a live session on that space, by user id.
    ///
    /// Seeded from `Ready.online` on every connect and kept current by the
    /// event pump. Held here rather than in the window because a reconnect
    /// replaces the whole set, and because the window may be told about a
    /// space it is not currently looking at (PLAN §6).
    online: Mutex<HashSet<String>>,
    /// The supervisor: dials, pumps events into the UI, and redials. Spawned
    /// on Tauri's runtime rather than tokio's directly, because that is the
    /// one the app is actually running on. Aborted when the server is
    /// disconnected, so a stale supervisor cannot keep writing to the mirror
    /// for a space the user has left.
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl Live {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            online: Mutex::new(HashSet::new()),
            supervisor: Mutex::new(None),
        }
    }

    pub fn session(&self) -> Option<Arc<Session>> {
        self.lock(&self.session).clone()
    }

    fn set_session(&self, session: Option<Arc<Session>>) {
        *self.lock(&self.session) = session;
    }

    pub fn online(&self) -> Vec<String> {
        self.lock(&self.online).iter().cloned().collect()
    }

    /// Replaces the whole set. A reconnect is the only thing that knows who
    /// is there *now*, and merging would keep whoever left while we were away.
    fn seed_online(&self, ids: impl IntoIterator<Item = String>) {
        *self.lock(&self.online) = ids.into_iter().collect();
    }

    fn set_online(&self, user_id: &str, online: bool) {
        let mut set = self.lock(&self.online);
        if online {
            set.insert(user_id.to_owned());
        } else {
            set.remove(user_id);
        }
    }

    pub fn attach(&self, supervisor: JoinHandle<()>) {
        if let Some(previous) = self.lock(&self.supervisor).replace(supervisor) {
            previous.abort();
        }
    }

    /// Hangs up and stops the supervisor **without aborting it**.
    ///
    /// For the one caller that is running *inside* the supervisor's own task:
    /// the event pump, when the server says this device is revoked. Aborting
    /// there would cancel the task at its next await, which is somewhere in
    /// the middle of telling the window what happened.
    ///
    /// Taking the handle rather than aborting it also disarms `Drop`, so the
    /// supervisor is left to notice on its own that nobody wants it any more
    /// and to wind itself down in order.
    fn abandon(&self) {
        let _ = self.lock(&self.supervisor).take();
        if let Some(session) = self.lock(&self.session).take() {
            session.disconnect();
        }
        self.lock(&self.online).clear();
    }

    /// Stops supervising and hangs up. Idempotent.
    fn shutdown(&self) {
        if let Some(supervisor) = self.lock(&self.supervisor).take() {
            supervisor.abort();
        }
        if let Some(session) = self.lock(&self.session).take() {
            session.disconnect();
        }
        // Nobody is reachable through a connection that is gone. Leaving the
        // set behind would show green dots next to a space that is offline.
        self.lock(&self.online).clear();
    }

    /// A poisoned lock means another thread panicked while holding it. These
    /// hold a handle and an `Option`, not an invariant anybody reasons about,
    /// so carrying on beats taking the window down.
    fn lock<'a, T>(&self, what: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        what.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.shutdown();
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
    sessions: RwLock<HashMap<String, Arc<Live>>>,
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

    /// The live session for a server, if one is connected *right now*.
    ///
    /// `None` covers two cases a caller should treat the same way: never
    /// connected, and between reconnect attempts. Both mean "write it to the
    /// outbox and let the supervisor deliver it".
    pub async fn session(&self, endpoint_id: &str) -> Option<Arc<Session>> {
        self.sessions.read().await.get(endpoint_id)?.session()
    }

    /// Whether the app is supposed to be connected to a server — which is not
    /// the same as whether it currently is. The supervisor reads this to know
    /// whether a dropped session is a gap to close or a goodbye.
    pub async fn is_connected(&self, endpoint_id: &str) -> bool {
        self.sessions.read().await.contains_key(endpoint_id)
    }

    /// Claims the slot for a server, replacing and tearing down whatever was
    /// there. The caller attaches its supervisor to the returned handle.
    pub async fn register(&self, endpoint_id: &str) -> Arc<Live> {
        let live = Arc::new(Live::new());
        // The replaced `Live` aborts its own supervisor on drop, which is what
        // stops two of them from writing the same events into the mirror.
        self.sessions
            .write()
            .await
            .insert(endpoint_id.to_owned(), live.clone());
        live
    }

    /// Points a server's slot at a freshly connected session.
    pub async fn set_live_session(&self, endpoint_id: &str, session: Arc<Session>) {
        if let Some(live) = self.sessions.read().await.get(endpoint_id) {
            live.set_session(Some(session));
        }
    }

    /// Marks a server as between connections, without giving up on it.
    pub async fn clear_live_session(&self, endpoint_id: &str) {
        if let Some(live) = self.sessions.read().await.get(endpoint_id) {
            live.set_session(None);
            // Presence is a property of an open connection. Between attempts
            // we do not know who is there, which is not the same as knowing
            // that nobody is — and the UI says exactly that.
            live.seed_online([]);
        }
    }

    /// Who is online on a server, as far as this process has been told.
    pub async fn online(&self, endpoint_id: &str) -> Vec<String> {
        self.sessions
            .read()
            .await
            .get(endpoint_id)
            .map(|live| live.online())
            .unwrap_or_default()
    }

    /// Replaces a server's online set, from a fresh `Ready`.
    pub async fn seed_online(&self, endpoint_id: &str, ids: impl IntoIterator<Item = String>) {
        if let Some(live) = self.sessions.read().await.get(endpoint_id) {
            live.seed_online(ids);
        }
    }

    /// Applies one `presence` event.
    pub async fn set_online(&self, endpoint_id: &str, user_id: &str, online: bool) {
        if let Some(live) = self.sessions.read().await.get(endpoint_id) {
            live.set_online(user_id, online);
        }
    }

    /// Hangs up on a server and stops trying to reach it.
    pub async fn remove_session(&self, endpoint_id: &str) -> bool {
        match self.sessions.write().await.remove(endpoint_id) {
            Some(live) => {
                live.shutdown();
                true
            }
            None => false,
        }
    }

    /// Gives up on a server, from inside the task that is supervising it.
    ///
    /// Used when the space says this device is revoked: retrying cannot change
    /// that answer, so the slot is released and the supervisor's next check
    /// tells it to stop. [`Self::remove_session`] is the version for callers
    /// that are *not* the supervisor.
    pub async fn stop_connecting(&self, endpoint_id: &str) {
        if let Some(live) = self.sessions.write().await.remove(endpoint_id) {
            live.abandon();
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
            live.shutdown();
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
