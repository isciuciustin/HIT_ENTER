//! The HIT_ENTER desktop shell.
//!
//! This crate owns the window and the bridge to the frontend, and nothing
//! else. Chat logic lives in `he-client`; hosting lives in `he-server`.
//! Keeping them out of here is what lets both be tested without a GUI — and
//! the M2 protocol tests are the proof that it worked.

mod commands;
mod events;
mod host;
mod session;
mod settings;
mod state;

use std::sync::Arc;

use tauri::Manager;

pub use commands::{AppInfo, CommandError, HostStatus, ServerSummary};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    disable_dmabuf_renderer_on_linux();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hit_enter=info,he_client=info,he_server=info".into()),
        )
        .init();

    tauri::Builder::default()
        // On Linux and Windows a clicked `hitenter://` link starts a *second*
        // copy of the app with the URL as an argument. Single-instance catches
        // that, hands the arguments to the window that is already open, and
        // exits — without it, clicking an invite would open a second app with
        // a second mirror and no idea it was the second.
        //
        // Scoped to the data directory rather than to the machine, because
        // that is where the real invariant is: two processes sharing a data
        // directory share a device key and a SQLite file, while two processes
        // with *different* data directories are two devices and must be able
        // to run side by side. That is the two-window recipe in
        // `docs/TESTING.md`, and a machine-wide lock silently breaks it.
        .plugin(
            tauri_plugin_single_instance::Builder::new()
                .callback(|app, argv, _cwd| {
                    forward_links(app, argv.iter().map(String::as_str));
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.set_focus();
                    }
                })
                .dbus_id(instance_id())
                .build(),
        )
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // The data directory holds the device key, the mirror and — if
            // this machine hosts — the space. The three things that make this
            // installation *this device*, give it a history, and make it a
            // host. `HE_DATA_DIR` overrides it, which is how two instances run
            // on one machine without sharing a device key.
            let fallback = app.path().app_data_dir()?;
            let data_dir = state::data_dir(fallback);

            let handle = app.handle().clone();
            let started = tauri::async_runtime::block_on(state::App::start(&data_dir))?;
            handle.manage(Arc::new(started));

            register_url_scheme(app);
            listen_for_links(app);
            // The link that started this process, if one did.
            forward_links(
                app.handle(),
                std::env::args()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .iter()
                    .map(String::as_str),
            );

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
            commands::edit_message,
            commands::delete_message,
            commands::typing,
            commands::members,
            commands::online,
            commands::unread,
            commands::mark_read,
            commands::create_channel,
            commands::delete_channel,
            commands::kick_member,
            commands::set_member_banned,
            commands::revoke_device,
            commands::devices,
            commands::invites,
            commands::revoke_invite,
            commands::create_invite,
            commands::pending_messages,
            commands::parse_link,
            commands::host_status,
            commands::create_space,
            commands::start_hosting,
            commands::stop_hosting,
            commands::network_settings,
            commands::set_network_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building HIT_ENTER")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Hang up rather than letting the process vanish: on the other
                // end an abandoned connection looks like a crash until QUIC's
                // idle timeout notices. The same goes for anyone connected to
                // a space this machine is hosting.
                if let Some(state) = app.try_state::<Arc<state::App>>() {
                    let state = state.inner().clone();
                    tauri::async_runtime::block_on(state.disconnect_all());
                }
            }
        });
}

/// Stops WebKitGTK from killing the window before it opens.
///
/// On Wayland with the proprietary NVIDIA driver, WebKitGTK's dmabuf renderer
/// dies at startup with `Gdk-Message: Error 71 (Protocol error) dispatching to
/// Wayland display` — no window, no error a user can act on. This was found
/// the hard way during M0 and is a large share of Linux desktops.
///
/// `.cargo/config.toml` sets the variable for `cargo run` and `cargo tauri
/// dev`, and the packaged `.desktop` file sets it in `Exec=`. Neither covers
/// an AppImage, a raw `./hit-enter`, or a bundle target added later — so the
/// binary sets it for itself, which covers every way it can be started.
///
/// Only when it is unset: `WEBKIT_DISABLE_DMABUF_RENDERER=0` in the
/// environment is somebody deliberately asking for the accelerated path, and
/// overriding that would make the escape hatch a lie.
fn disable_dmabuf_renderer_on_linux() {
    #[cfg(target_os = "linux")]
    {
        const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
        if std::env::var_os(VAR).is_some() {
            return;
        }
        // SAFETY: `set_var` is unsafe because another thread reading the
        // environment concurrently is undefined behaviour. This is the first
        // statement of `run`, which is the first statement of `main`: no
        // runtime has been started, no thread has been spawned, and the
        // webview process that reads this variable is not forked until later.
        unsafe { std::env::set_var(VAR, "1") };
    }
}

/// Which "single instance" this process belongs to.
///
/// The default data directory is the overwhelmingly common case and gets the
/// bundle identifier unchanged, so a released app behaves exactly as the
/// plugin intends. An explicit `HE_DATA_DIR` names a *different device* — its
/// own key, its own mirror, possibly its own space — so it gets its own lock
/// and can run alongside the others.
///
/// Hashed rather than embedded: a D-Bus name element may only contain
/// `[A-Za-z0-9_-]`, and a path contains neither only those nor a bounded
/// length.
fn instance_id() -> String {
    const BUNDLE: &str = "computer.hitenter.app";
    match std::env::var_os("HE_DATA_DIR") {
        None => BUNDLE.to_string(),
        Some(dir) => format!("{BUNDLE}.d{:016x}", fnv1a(dir.as_encoded_bytes())),
    }
}

/// FNV-1a, 64-bit. Not a security boundary — a collision means two data
/// directories share a lock, which costs a refused second window and nothing
/// else — so it is here rather than as a dependency.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Claims `hitenter://` with the desktop, at runtime.
///
/// On Linux this writes a `.desktop` entry pointing at the running binary, and
/// on Windows a registry key; on macOS the association comes from the bundle
/// and this does nothing. Best effort in every case — a user who cannot
/// register a URL scheme can still paste a link into the join dialog, which is
/// the path that always works.
///
/// The released Linux build needs its own `.desktop` file anyway, for
/// `WEBKIT_DISABLE_DMABUF_RENDERER=1` in `Exec=` (PLAN M6).
fn register_url_scheme(app: &tauri::App) {
    #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        if let Err(err) = app.deep_link().register_all() {
            tracing::warn!(%err, "could not register the hitenter:// scheme with the desktop");
        }
    }
    #[cfg(not(any(target_os = "linux", all(debug_assertions, windows))))]
    let _ = app;
}

/// On macOS, iOS and Android the OS delivers the URL to the running process.
fn listen_for_links(app: &tauri::App) {
    use tauri_plugin_deep_link::DeepLinkExt;
    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        forward_links(&handle, event.urls().iter().map(|url| url.as_str()));
    });
}

/// Passes anything that looks like one of our links to the window.
///
/// Filtered on the scheme rather than taken as "the first argument", because
/// these are process arguments: `--flag`, a file path, and a URL all arrive
/// the same way, and only one of them is an invite.
fn forward_links<'a>(app: &tauri::AppHandle, candidates: impl Iterator<Item = &'a str>) {
    for candidate in candidates {
        if candidate
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(he_proto::ticket::SCHEME))
        {
            tracing::info!("a join link arrived from the desktop");
            events::emit_link(app, candidate);
        }
    }
}
