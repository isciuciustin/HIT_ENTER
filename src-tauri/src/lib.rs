//! The HIT_ENTER desktop shell.
//!
//! This crate owns the window and the bridge to the frontend, and nothing else.
//! Chat logic lives in `he-client`; hosting lives in `he-server`. Keeping them
//! out of here is what lets both be tested without a GUI.

use serde::Serialize;

/// What the frontend needs to render its status line.
#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub protocol: String,
    pub protocol_version: u32,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        protocol: String::from_utf8_lossy(he_proto::ALPN).into_owned(),
        protocol_version: he_proto::PROTOCOL_VERSION,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hit_enter=info,he_client=info,he_server=info".into()),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("error while running HIT_ENTER");
}
