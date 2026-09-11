//! The HIT_ENTER desktop shell.
//!
//! This crate owns the window and the bridge to the frontend, and nothing
//! else. Chat logic lives in `he-client`; hosting lives in `he-server`.
//! Keeping them out of here is what lets both be tested without a GUI — and
//! the M2 protocol tests are the proof that it worked.

mod commands;
mod events;
mod state;

use std::sync::Arc;

use tauri::Manager;

pub use commands::{AppInfo, CommandError, ServerSummary};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hit_enter=info,he_client=info,he_server=info".into()),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            // The data directory holds the device key and the mirror — the two
            // things that make this installation *this device* and give it a
            // history. `HE_DATA_DIR` overrides it, which is how two instances
            // run on one machine without sharing a device key.
            let fallback = app.path().app_data_dir()?;
            let data_dir = state::data_dir(fallback);

            let handle = app.handle().clone();
            let started = tauri::async_runtime::block_on(state::App::start(&data_dir))?;
            handle.manage(Arc::new(started));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_servers,
            commands::join_server,
            commands::connect_server,
            commands::disconnect_server,
            commands::forget_server,
            commands::channels,
            commands::history,
            commands::sync_channel,
            commands::send_message,
            commands::create_invite,
            commands::pending_messages,
        ])
        .build(tauri::generate_context!())
        .expect("error while building HIT_ENTER")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Hang up rather than letting the process vanish: on the other
                // end an abandoned connection looks like a crash until QUIC's
                // idle timeout notices.
                if let Some(state) = app.try_state::<Arc<state::App>>() {
                    let state = state.inner().clone();
                    tauri::async_runtime::block_on(state.disconnect_all());
                }
            }
        });
}
