//! The HIT_ENTER desktop app without a window (M12-1).
//!
//! [`state`] is the logic — onboarding, the node's life, every question a
//! window asks — moved here unchanged from `src-tauri` (M4a). [`events`] is
//! what a window is told, as typed [`events::UiEvent`]s handed to any
//! [`events::Sink`]. The Tauri shell is one sink; the native window
//! (`hit-desktop`) is another. Neither holds logic of its own.

pub mod browse;
pub mod events;

/// The scheme of join and contact links (`hit-enter:join?…`).
pub use hit_proto::invite::URL_SCHEME;
pub mod state;

use std::{net::SocketAddr, sync::Arc};

use hit_event::Keychain;
use hit_node::Relays;

use state::{App, Net};

/// Opens the app over the OS keychain, with the environment every window
/// honours (`docs/TESTING.md`): `HIT_KEYCHAIN_SERVICE`, `HIT_PORT`,
/// `HIT_RELAYS=none`, `HIT_PEERS`, `HIT_PUBLIC_RELAYS` (`none`, or a
/// comma-separated list in place of the defaults). The error is a sentence for the window —
/// shown there rather than a process that dies before drawing anything.
pub async fn open_from_env(data_dir: &std::path::Path) -> Result<Arc<App>, String> {
    let service =
        std::env::var("HIT_KEYCHAIN_SERVICE").unwrap_or_else(|_| Keychain::APP_SERVICE.into());
    let keychain = Keychain::new(service).map_err(|e| {
        format!("{e}. HIT_ENTER keeps your key in the OS keychain and will not run without it.")
    })?;
    let port = std::env::var("HIT_PORT").ok().and_then(|p| p.parse().ok());
    let relays = match std::env::var("HIT_RELAYS").as_deref() {
        Ok("none") => Relays::Disabled,
        _ => Relays::N0,
    };
    let net = Net {
        invite_addr: port.map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
        relays,
        port,
        peers: peers_from_env()?,
        public_relays: match std::env::var("HIT_PUBLIC_RELAYS").as_deref() {
            Ok("none") => None,
            Ok(list) => Some(list.split(',').map(|r| r.trim().to_owned()).collect()),
            Err(_) => Some(
                hit_zap::PUBLIC_RELAYS
                    .iter()
                    .map(|r| (*r).to_owned())
                    .collect(),
            ),
        },
    };
    App::open(data_dir, Arc::new(keychain), net)
        .await
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

/// `HIT_PEERS`: `<device id>@<ip:port>`, comma-separated.
fn peers_from_env() -> Result<Vec<(iroh::EndpointId, SocketAddr)>, String> {
    let Ok(list) = std::env::var("HIT_PEERS") else {
        return Ok(Vec::new());
    };
    list.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (id, addr) = p
                .trim()
                .split_once('@')
                .ok_or_else(|| format!("HIT_PEERS: {p} is not <device>@<ip:port>"))?;
            let id = id
                .parse()
                .map_err(|e| format!("HIT_PEERS: bad device id {id}: {e}"))?;
            let addr = addr
                .parse()
                .map_err(|e| format!("HIT_PEERS: bad address {addr}: {e}"))?;
            Ok((id, addr))
        })
        .collect()
}
