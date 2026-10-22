//! Files (M8): the bytes, content-addressed by BLAKE3, on `/iroh-bytes/4`.
//!
//! HIT_ENTER_2 M7's attachment path, moved onto the mesh. What carries over,
//! and why (`../HIT_ENTER_2/spikes/blobs/README.md`):
//!
//! - **`iroh-blobs` is pull-only.** Posting a file is adding it to this
//!   node's store and putting its hash in a message; whoever may read the
//!   message fetches it — from the uploader, or from anyone who already has.
//! - **Never serve the store as `iroh-blobs` ships it**: its handler serves
//!   any blob to anyone, which is content access *and* an existence oracle
//!   ("do you hold the file with this hash?"). Two gates instead, below.
//! - **Fetch with `execute_get` and a full range**, never `Remote::fetch`,
//!   which trusts a partial local copy and short-circuits.
//! - **Nothing deletes a blob directly.** The store's garbage collector asks
//!   [`protect`] what to keep before every sweep, and the answer is read
//!   fresh from the event store: the files some held message carries, within
//!   the cache's size — so a withdrawn, removed or silenced message's files
//!   go with it, eventually, and nothing a member can still see is swept.
//!
//! What is new on a mesh: a file has no one host. Any member who can read
//! the message may be asked for it, and serves it only to those who can read
//! it too — so the done-when is a file downloaded from a member's *cache*
//! after its uploader has gone.

use std::{
    collections::HashSet,
    path::Path,
    str::FromStr,
    sync::{Arc, RwLock},
    time::Duration,
};

use hit_store::Store;
use iroh::{Endpoint, EndpointAddr, endpoint::Connection};
use iroh_blobs::{
    Hash,
    api::blobs::BlobStatus,
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventResult, EventSender, ObserveMode,
        ProviderMessage, RequestMode, ThrottleMode,
    },
    store::{GcConfig, ProtectCb, ProtectOutcome, fs::FsStore},
};
use nostr::prelude::{PublicKey, Timestamp};
use sha2::Digest;

use crate::{Inner, NodeError};

/// How often the collector runs.
const GC_EVERY: Duration = Duration::from_secs(600);
/// A file added (attached) but not yet in a sent message is kept this long.
const PENDING_KEEP: u64 = 3_600;
/// QUIC close code for a blob connection from a device that speaks for
/// nobody we know.
const UNKNOWN_DEVICE: u32 = 0x10;
/// How long a dial to a provider may take before the next one is tried — the
/// connection only: a big file's transfer takes as long as it takes.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How many provider events may wait for an answer.
const EVENT_QUEUE: usize = 32;

/// What this node keeps (M8). A member's app caches; an anchor pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePolicy {
    /// Keep every file any held message carries — an anchor.
    pub pin_all: bool,
    /// Otherwise: files of ours, then the most recently used, up to this.
    pub cache_bytes: u64,
}

impl Default for FilePolicy {
    fn default() -> Self {
        Self {
            pin_all: false,
            cache_bytes: 1024 * 1024 * 1024,
        }
    }
}

pub(crate) struct Blobs {
    store: FsStore,
    pub(crate) policy: Arc<RwLock<FilePolicy>>,
}

pub(crate) fn parse_hash(hash: &str) -> Result<Hash, NodeError> {
    Hash::from_str(hash).map_err(|_| NodeError::Protocol("not a file hash".into()))
}

fn blob_err(e: impl std::fmt::Display) -> NodeError {
    NodeError::Blob(e.to_string())
}

impl Blobs {
    pub(crate) async fn open(dir: &Path, events: Store) -> Result<Self, NodeError> {
        tokio::fs::create_dir_all(dir).await.map_err(blob_err)?;
        let policy = Arc::new(RwLock::new(FilePolicy::default()));
        let mut options = iroh_blobs::store::fs::options::Options::new(dir);
        options.gc = Some(GcConfig {
            interval: GC_EVERY,
            add_protected: Some(protect(events, policy.clone())),
        });
        let store = FsStore::load_with_opts(dir.join("blobs.db"), options)
            .await
            .map_err(blob_err)?;
        Ok(Self { store, policy })
    }

    /// Adds a file from disk: its BLAKE3 (from the store) and SHA-256.
    pub(crate) async fn add_path(&self, path: &Path) -> Result<(String, String, u64), NodeError> {
        let path = std::path::absolute(path).map_err(blob_err)?;
        let size = tokio::fs::metadata(&path).await.map_err(blob_err)?.len();
        if size == 0 || size > hit_event::group::MAX_ATTACHMENT_BYTES {
            return Err(NodeError::Blob(format!(
                "a file must be 1 byte to {} MB",
                hit_event::group::MAX_ATTACHMENT_BYTES / 1_000_000
            )));
        }
        let tag = self.store.add_path(&path).await.map_err(blob_err)?;
        let sha = {
            let p = path.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<String> {
                let mut f = std::fs::File::open(p)?;
                let mut h = sha2::Sha256::new();
                std::io::copy(&mut f, &mut h)?;
                Ok(hex::encode(h.finalize()))
            })
            .await
            .map_err(blob_err)?
            .map_err(blob_err)?
        };
        Ok((tag.hash.to_hex().to_string(), sha, size))
    }

    /// Adds bytes already in memory — a DM file's ciphertext (M8b).
    pub(crate) async fn add_bytes(&self, bytes: Vec<u8>) -> Result<String, NodeError> {
        let tag = self.store.add_bytes(bytes).await.map_err(blob_err)?;
        Ok(tag.hash.to_hex().to_string())
    }

    pub(crate) async fn complete(&self, hash: &Hash) -> Result<bool, NodeError> {
        Ok(matches!(
            self.store.status(*hash).await.map_err(blob_err)?,
            BlobStatus::Complete { .. }
        ))
    }

    /// Fetches `hash` from `from`: the whole blob, verified by hash on the
    /// way in, whatever is here already.
    pub(crate) async fn fetch(
        &self,
        endpoint: &Endpoint,
        from: EndpointAddr,
        hash: &Hash,
    ) -> Result<(), NodeError> {
        let conn = tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(from, iroh_blobs::ALPN))
            .await
            .map_err(|_| NodeError::Connect("timed out".into()))?
            .map_err(|e| NodeError::Connect(e.to_string()))?;
        self.store
            .remote()
            .execute_get(conn, iroh_blobs::protocol::GetRequest::blob(*hash))
            .await
            .map_err(blob_err)?;
        if self.complete(hash).await? {
            Ok(())
        } else {
            Err(NodeError::Blob("incomplete after fetching".into()))
        }
    }

    /// Fetches `hash` from the first of `from` that answers and serves it:
    /// every dial at once, so a provider that has gone — often the uploader
    /// — costs nothing while one that is up answers in milliseconds. The
    /// transfer itself runs over one connection at a time.
    pub(crate) async fn fetch_first(
        &self,
        endpoint: &Endpoint,
        from: Vec<EndpointAddr>,
        hash: &Hash,
    ) -> Result<(), NodeError> {
        let mut dials = tokio::task::JoinSet::new();
        for addr in from {
            let ep = endpoint.clone();
            dials.spawn(async move {
                let id = addr.id;
                let conn =
                    tokio::time::timeout(DIAL_TIMEOUT, ep.connect(addr, iroh_blobs::ALPN)).await;
                (id, conn)
            });
        }
        while let Some(done) = dials.join_next().await {
            let Ok((id, Ok(Ok(conn)))) = done else {
                continue;
            };
            let got = self
                .store
                .remote()
                .execute_get(conn, iroh_blobs::protocol::GetRequest::blob(*hash))
                .await;
            match got {
                Ok(_) if self.complete(hash).await? => {
                    dials.abort_all();
                    return Ok(());
                }
                Ok(_) => tracing::debug!("{} sent an incomplete file", id.fmt_short()),
                Err(e) => tracing::debug!("{} did not serve the file: {e}", id.fmt_short()),
            }
        }
        Err(NodeError::NoProvider)
    }

    pub(crate) async fn bytes(&self, hash: &Hash) -> Result<Vec<u8>, NodeError> {
        Ok(self
            .store
            .get_bytes(*hash)
            .await
            .map_err(blob_err)?
            .to_vec())
    }

    pub(crate) async fn export(&self, hash: &Hash, to: &Path) -> Result<(), NodeError> {
        let to = std::path::absolute(to).map_err(blob_err)?;
        self.store.export(*hash, &to).await.map_err(blob_err)?;
        Ok(())
    }

    /// Serves one incoming `/iroh-bytes/4` connection, behind two gates:
    ///
    /// - **the device must speak for someone** — a person whose binding
    ///   names it (read off the connection, never a body). A stranger's
    ///   probe never reaches the store.
    /// - **each hash must be attached to a message one of those persons may
    ///   read**, by `hit-group`'s one rule. A file in a private channel is as
    ///   private as the channel. A pushed blob is refused: our store takes
    ///   only what we chose to fetch.
    pub(crate) async fn serve(&self, inner: Arc<Inner>, conn: Connection) {
        let device = conn.remote_id();
        let persons = inner.book().persons_of(device, Timestamp::now());
        if persons.is_empty() {
            tracing::debug!(
                "blob connection from {} refused: nobody",
                device.fmt_short()
            );
            conn.close(UNKNOWN_DEVICE.into(), b"unknown device");
            return;
        }
        let (events, mut asks) = EventSender::channel(EVENT_QUEUE, MASK);
        let gate = tokio::spawn(async move {
            while let Some(message) = asks.recv().await {
                answer(&inner, &persons, message).await;
            }
        });
        iroh_blobs::provider::handle_connection(conn, (*self.store).clone(), events).await;
        gate.abort();
    }

    pub(crate) async fn shutdown(&self) {
        if let Err(e) = self.store.shutdown().await {
            tracing::warn!("the blob store did not shut down cleanly: {e}");
        }
    }
}

/// Every read is asked about; pushes are refused outright.
const MASK: EventMask = EventMask {
    connected: ConnectMode::None,
    get: RequestMode::Intercept,
    get_many: RequestMode::Intercept,
    push: RequestMode::Disabled,
    observe: ObserveMode::Intercept,
    throttle: ThrottleMode::None,
};

async fn answer(inner: &Inner, persons: &[PublicKey], message: ProviderMessage) {
    match message {
        ProviderMessage::GetRequestReceived(msg) => {
            let ok = permitted(inner, persons, &msg.inner.request.hash).await;
            msg.tx.send(ok).await.ok();
        }
        ProviderMessage::GetManyRequestReceived(msg) => {
            // All or nothing: a partial answer says which hashes we hold.
            let mut ok = Ok(());
            for h in &msg.inner.request.hashes {
                if let Err(e) = permitted(inner, persons, h).await {
                    ok = Err(e);
                    break;
                }
            }
            msg.tx.send(ok).await.ok();
        }
        // Size and presence: the existence question in another costume.
        ProviderMessage::ObserveRequestReceived(msg) => {
            let ok = permitted(inner, persons, &msg.inner.request.hash).await;
            msg.tx.send(ok).await.ok();
        }
        ProviderMessage::PushRequestReceived(msg) => {
            msg.tx.send(Err(AbortReason::Permission)).await.ok();
        }
        _ => {}
    }
}

/// May any of `persons` read a message carrying `hash` — or were they
/// granted it, as one of the two people of a DM file (M8b)? A store error is
/// a refusal: "could not check" must never mean "served".
async fn permitted(inner: &Inner, persons: &[PublicKey], hash: &Hash) -> EventResult {
    let hex = hash.to_hex();
    let (carrying, granted) = match (
        inner.store.carrying(&hex).await,
        inner.store.granted(&hex).await,
    ) {
        (Ok(c), Ok(g)) => (c, g),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!("refusing a file request: {e}");
            return Err(AbortReason::Permission);
        }
    };
    if persons.iter().any(|p| granted.contains(p)) {
        return Ok(());
    }
    let now = Timestamp::now();
    let groups = inner.groups();
    let ok = carrying
        .iter()
        .any(|ev| persons.iter().any(|p| groups.may_read(p, ev, now)));
    if ok {
        Ok(())
    } else {
        Err(AbortReason::Permission)
    }
}

/// What the collector keeps, read fresh before every sweep: with
/// `pin_all`, every file a held message carries; otherwise ours, then the
/// most recently used within the cache — and anything attached in the last
/// hour that is not in a sent message yet. **A failed read aborts the
/// sweep**: sweeping with half a live set deletes what someone can still
/// see. (`tokio::spawn`, because the callback's future must be `Sync` and a
/// `sqlx` one is not — HIT_ENTER_2's lesson.)
fn protect(events: Store, policy: Arc<RwLock<FilePolicy>>) -> ProtectCb {
    Arc::new(move |live: &mut HashSet<Hash>| {
        let (events, policy) = (events.clone(), policy.clone());
        Box::pin(async move {
            let p = *policy.read().unwrap_or_else(|e| e.into_inner());
            let read = tokio::spawn(async move {
                let mut keep = if p.pin_all {
                    events
                        .attached()
                        .await?
                        .into_iter()
                        .map(|(h, _)| h)
                        .collect()
                } else {
                    events.blobs_to_keep(p.cache_bytes).await?
                };
                let since = Timestamp::now().as_secs().saturating_sub(PENDING_KEEP);
                keep.extend(events.mine_since(since).await?);
                // DM files: kept while their grant is.
                keep.extend(events.granted_hashes().await?);
                Ok::<_, hit_store::StoreError>(keep)
            });
            let hashes = match read.await {
                Ok(Ok(h)) => h,
                Ok(Err(e)) => {
                    tracing::error!("skipping file collection: {e}");
                    return ProtectOutcome::Abort;
                }
                Err(e) => {
                    tracing::error!("skipping file collection: {e}");
                    return ProtectOutcome::Abort;
                }
            };
            for h in hashes {
                match Hash::from_str(&h) {
                    Ok(h) => {
                        live.insert(h);
                    }
                    Err(_) => {
                        tracing::error!("skipping file collection: an unreadable hash");
                        return ProtectOutcome::Abort;
                    }
                }
            }
            ProtectOutcome::Continue
        })
    })
}
