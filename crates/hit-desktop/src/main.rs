//! HIT_ENTER's native window (M12): Slint over `hit-app-core`.
//!
//! Two threads of work: Slint's event loop owns the main thread and every
//! UI object; a small tokio runtime runs the node. They meet only through
//! `slint::Weak::upgrade_in_event_loop` — the runtime never touches the
//! window directly, and the window never waits on the runtime.
//!
//! The environment is the Tauri window's (`HIT_DATA_DIR`,
//! `HIT_KEYCHAIN_SERVICE`, `HIT_RELAYS`, `HIT_PORT`, `HIT_PEERS`), and so is
//! the data directory: the same device, whichever window opens it.

// A release on Windows opens no console beside the window; a debug build
// keeps one for the logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod art;
mod chat;
mod instance;
mod pictures;
mod sink;
mod twemoji;

use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, atomic::AtomicBool},
};

use hit_app_core::state::{App, Stage, Status};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::runtime::Handle;

/// The `.slint` files, compiled by `build.rs`. Slint's generated code
/// unwraps internally; the allowance stops at this module.
mod ui {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    slint::include_modules!();
}
use ui::{MainWindow, StatusUi};

/// The Tauri window's directory: `<data dir>/<bundle identifier>`.
const BUNDLE: &str = "app.hitenter.desktop";

/// What every callback shares once the app is open.
struct Ctl {
    app: Arc<App>,
    rt: Handle,
    ui: slint::Weak<MainWindow>,
    /// Whether the event pump runs (`hit_app_core::events::start`).
    pumping: AtomicBool,
    /// The main window's model, once the node runs.
    chat: OnceLock<Arc<chat::Chat>>,
    /// Kept alive: on Linux the clipboard's contents are served by whoever
    /// owns them, and dropping this would empty it.
    clipboard: Mutex<Option<arboard::Clipboard>>,
}

/// glibc keeps what a thread frees in that thread's arena, and raises its
/// mmap threshold to the size of the last large free — so after a few
/// 24 MB photo decodes on the blocking pool, every later one lands in an
/// arena and stays there: 506 MB of freed pixels held after opening 50
/// photos (M12-9's benchmark). A fixed threshold sends every large buffer
/// straight to the kernel and back; two arenas bound the rest.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    // SAFETY: `mallopt` only sets allocator parameters; called once, first
    // thing in `main`, before any other thread exists.
    #[allow(unsafe_code)]
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 256 * 1024);
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tune_allocator();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hit_desktop=info,hit_app_core=info,hit_node=info".into()),
        )
        .init();

    // Two workers: the node is mostly waiting on the network, and every
    // idle thread is memory (PLAN.md M12).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    // One window per data directory: a second copy (a clicked link) hands
    // its links to the first and goes.
    let links = instance::links_in(std::env::args().skip(1));
    let dir = data_dir();
    let listener = match &dir {
        Ok(d) => match instance::claim(d, &links) {
            Ok(instance::Claim::Handed) => return Ok(()),
            Ok(instance::Claim::First(l)) => Some(l),
            Err(e) => {
                tracing::warn!("single instance: {e}");
                None
            }
        },
        Err(_) => None,
    };
    // The desktop's `hit-enter:` handler — only for a person's own data, not
    // the test windows' (`HIT_DATA_DIR`), which must not take it over.
    if std::env::var_os("HIT_DATA_DIR").is_none() {
        instance::register_scheme();
    }

    let ui = MainWindow::new()?;
    art::install(&ui);
    // The window's class on Wayland and X11: what the desktop file's
    // StartupWMClass names, so the dock shows our icon for it. After the
    // window exists (the platform does then), before it is shown.
    if let Err(e) = slint::set_xdg_app_id("hit-enter") {
        tracing::warn!("app id: {e}");
    }
    let ctl: Arc<OnceLock<Arc<Ctl>>> = Arc::default();
    wire(&ui, &ctl);
    watch_focus(&ui, &ctl);
    forward_links(&ui.as_weak(), links);
    if let Some(l) = listener {
        let weak = ui.as_weak();
        l.listen(move |links| {
            raise(&weak);
            forward_links(&weak, links);
        });
    }
    #[cfg(target_os = "macos")]
    {
        let weak = ui.as_weak();
        instance::on_apple_event_links(move |links| {
            raise(&weak);
            forward_links(&weak, links);
        });
    }

    let (weak, slot, handle) = (ui.as_weak(), ctl.clone(), rt.handle().clone());
    rt.spawn(async move {
        let opened = match dir {
            Err(why) => Err(why),
            Ok(dir) => hit_app_core::open_from_env(&dir).await,
        };
        match opened {
            Ok(app) => {
                let c = Arc::new(Ctl {
                    app,
                    rt: handle,
                    ui: weak,
                    pumping: AtomicBool::new(false),
                    chat: OnceLock::new(),
                    clipboard: Mutex::new(None),
                });
                let _ = slot.set(c.clone());
                c.refresh().await;
            }
            Err(why) => {
                tracing::error!("could not open the app: {why}");
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_fatal(why.into()));
            }
        }
    });

    ui.run()?;

    // Hang up rather than vanish: an abandoned connection looks like a
    // crash to the other side until QUIC's idle timeout.
    if let Some(c) = ctl.get() {
        rt.block_on(c.app.shutdown());
    }
    Ok(())
}

/// Hands links to the window: the join or contact dialog opens with each,
/// once the main window is up (`take-link` in main.slint).
fn forward_links(ui: &slint::Weak<MainWindow>, links: Vec<String>) {
    // The last one wins: two links at once is a double click.
    let Some(link) = links.into_iter().last() else { return };
    tracing::info!("a link arrived from the desktop");
    let contact = link
        .split_once(':')
        .is_some_and(|(_, rest)| rest.starts_with("contact"));
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_pending_kind(if contact { "contact" } else { "join" }.into());
        ui.set_pending_link(link.into());
        ui.invoke_take_link();
    });
}

/// Brings the window forward — or, where the compositor will not let an
/// app take focus, asks for attention.
fn raise(ui: &slint::Weak<MainWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        use slint::winit_030::{WinitWindowAccessor, winit::window::UserAttentionType};
        let _ = ui.show();
        ui.window().with_winit_window(|w| {
            w.focus_window();
            w.request_user_attention(Some(UserAttentionType::Informational));
        });
    });
}

/// Whether the window has the desktop's focus: a channel open in a window
/// in the background is not read (M12-4, owed until now).
fn watch_focus(ui: &MainWindow, ctl: &Arc<OnceLock<Arc<Ctl>>>) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
    let ctl = ctl.clone();
    ui.window().on_winit_window_event(move |_, event| {
        if let WindowEvent::Focused(on) = event
            && let Some(c) = ctl.get()
        {
            let on = *on;
            c.with_chat(async move |chat, _| chat.window_focused(on).await);
        }
        EventResult::Propagate
    });
}

/// Connects the window's callbacks. Each one runs only once the app is
/// open; before that the window shows "opening…" and has no buttons.
fn wire(ui: &MainWindow, ctl: &Arc<OnceLock<Arc<Ctl>>>) {
    let get = {
        let ctl = ctl.clone();
        move || ctl.get().cloned()
    };

    ui.on_is_encrypted(|secret| secret.trim().starts_with("ncryptsec"));

    let g = get.clone();
    ui.on_create(move || {
        if let Some(c) = g() {
            c.run(async move |app: Arc<App>| app.create_identity().map(drop));
        }
    });

    let g = get.clone();
    ui.on_import_key(move |secret, password| {
        if let Some(c) = g() {
            let (secret, password) = (secret.to_string(), password.to_string());
            c.run(async move |app: Arc<App>| {
                let password = (!password.is_empty()).then_some(password);
                app.import_identity(&secret, password.as_deref())
                    .await
                    .map(drop)
            });
        }
    });

    let g = get.clone();
    ui.on_save_file(move |password, again| {
        let Some(c) = g() else { return };
        // Before the dialog, not after it: a refusal once a file has been
        // chosen wastes the person's choice.
        let min = c.ui.upgrade().map_or(0, |ui| ui.get_status().min_password);
        if i32::try_from(password.chars().count()).unwrap_or(i32::MAX) < min {
            c.show_error(format!("the password needs at least {min} characters"));
            return;
        }
        if password != again {
            c.show_error("the two passwords differ".into());
            return;
        }
        let password = password.to_string();
        let c2 = c.clone();
        c.rt.spawn(async move {
            let start = dirs::document_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            let picked = rfd::AsyncFileDialog::new()
                .set_directory(start)
                .set_file_name("hit-enter-key.ncryptsec")
                .save_file()
                .await;
            let Some(file) = picked else { return };
            let path = file.path().to_path_buf();
            c2.run(async move |app: Arc<App>| app.backup_to_file(&path, &password).await);
        });
    });

    let g = get.clone();
    ui.on_reveal(move || {
        let Some(c) = g() else { return };
        match c.app.reveal_key() {
            Ok(key) => {
                if let Some(ui) = c.ui.upgrade() {
                    ui.set_shown_key(key.as_str().into());
                    ui.set_error("".into());
                }
            }
            Err(e) => c.show_error(e.to_string()),
        }
    });

    let g = get.clone();
    ui.on_copy_key(move || {
        let Some(c) = g() else { return };
        let Some(ui) = c.ui.upgrade() else { return };
        let key = ui.get_shown_key().to_string();
        let copied = c.copy(&key);
        ui.set_copied(copied);
    });

    let g = get.clone();
    ui.on_confirm(move |tail| {
        if let Some(c) = g() {
            let tail = tail.to_string();
            c.run(async move |app: Arc<App>| app.confirm_backup(&tail).await);
        }
    });

    let g = get.clone();
    ui.on_set_name(move |name| {
        if let Some(c) = g() {
            let name = name.to_string();
            c.run(async move |app: Arc<App>| app.set_name(&name).await);
        }
    });

    wire_chat(ui, get);
}

/// The main window's callbacks (M12-4).
fn wire_chat(ui: &MainWindow, get: impl Fn() -> Option<Arc<Ctl>> + Clone + 'static) {
    let g = get.clone();
    ui.on_select_group(move |id| {
        if let Some(c) = g() {
            let id = id.to_string();
            c.with_chat(async move |chat, _| chat.select_group(id).await);
        }
    });

    let g = get.clone();
    ui.on_select_channel(move |id| {
        if let Some(c) = g() {
            let id = id.to_string();
            c.with_chat(async move |chat, _| chat.select_channel(id).await);
        }
    });

    let g = get.clone();
    ui.on_send(move |text| {
        if let Some(c) = g() {
            let text = text.to_string();
            c.with_chat(async move |chat, ui| {
                let sent = text.clone();
                if chat.send(text).await {
                    // Cleared only if it is still what was sent: anything
                    // typed meanwhile stays.
                    let _ = ui.upgrade_in_event_loop(move |ui| {
                        if ui.get_draft() == sent.as_str() {
                            ui.set_draft("".into());
                        }
                    });
                }
            });
        }
    });

    let g = get.clone();
    ui.on_create_group(move |name, public| {
        if let Some(c) = g() {
            let name = name.to_string();
            c.dialog_step(async move |chat| chat.create_group(name, public).await);
        }
    });

    let g = get.clone();
    ui.on_join(move |link| {
        if let Some(c) = g() {
            let link = link.to_string();
            c.dialog_step(async move |chat| chat.join(link).await);
        }
    });

    let g = get.clone();
    ui.on_browse(move |query, fresh| browse(&g, query.to_string(), fresh));

    let g = get.clone();
    ui.on_make_invite(move |hours| {
        let Some(c) = g() else { return };
        if let Some(ui) = c.ui.upgrade() {
            ui.set_dialog_busy(true);
            ui.set_dialog_error("".into());
        }
        let c2 = c.clone();
        c.with_chat(async move |chat, ui| {
            let made = chat.invite(u32::try_from(hours).ok()).await;
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_dialog_busy(false);
                match made {
                    Ok(link) => {
                        ui.set_invite_copied(c2.copy(&link));
                        ui.set_invite_link(link.into());
                    }
                    Err(e) => ui.set_dialog_error(e.into()),
                }
            });
        });
    });

    let g = get.clone();
    ui.on_react(move |id, emoji| {
        if let Some(c) = g() {
            let (id, emoji) = (id.to_string(), emoji.to_string());
            c.with_chat(async move |chat, _| chat.react(id, emoji).await);
        }
    });

    let g = get.clone();
    ui.on_withdraw(move |id| {
        if let Some(c) = g() {
            let id = id.to_string();
            c.with_chat(async move |chat, _| chat.take_back(id, false).await);
        }
    });

    let g = get.clone();
    ui.on_remove_message(move |id| {
        if let Some(c) = g() {
            let id = id.to_string();
            c.with_chat(async move |chat, _| chat.take_back(id, true).await);
        }
    });

    ui.on_typed(|text| typed(&text));

    wire_moderation(ui, get.clone());
    wire_files_and_dms(ui, get.clone());
    wire_voice(ui, get.clone());
    wire_you_and_zaps(ui, get.clone());

    let g = get;
    ui.on_copy_invite(move || {
        let Some(c) = g() else { return };
        let Some(ui) = c.ui.upgrade() else { return };
        let link = ui.get_invite_link().to_string();
        ui.set_invite_copied(c.copy(&link));
    });
}

/// The settings and member dialogs (M12-5). Each action is one signed
/// event; what it did, or why this node refused it, shows in the dialog.
fn wire_moderation(ui: &MainWindow, get: impl Fn() -> Option<Arc<Ctl>> + Clone + 'static) {
    let (g, weak) = (get.clone(), ui.as_weak());
    ui.on_open_dialog(move |kind| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_dialog_error("".into());
        ui.set_dialog_done("".into());
        ui.set_dialog_busy(false);
        ui.set_invite_link("".into());
        ui.set_invite_copied(false);
        if kind == "settings"
            && let Some(c) = g()
        {
            c.with_chat(async move |chat, _| chat.open_settings().await);
        }
        if kind == "profile"
            && let Some(c) = g()
        {
            ui.set_profile(ui::ProfileUi::default());
            c.with_chat(async move |chat, ui| {
                let p = chat.profile().await;
                let _ = ui.upgrade_in_event_loop(move |ui| ui.set_profile(p));
            });
        }
        if kind == "browse" {
            ui.set_browse_query("".into());
            browse(&g, String::new(), false);
        }
        if kind == "audio"
            && let Some(c) = g()
        {
            ui.set_audio_loaded(false);
            c.with_chat(async move |chat, ui| {
                let got = chat.audio_settings().await;
                let _ = ui.upgrade_in_event_loop(move |ui| match got {
                    Ok(a) => a.apply(&ui),
                    Err(e) => ui.set_dialog_error(e.into()),
                });
            });
        }
        ui.set_dialog(kind);
    });

    let (g, weak) = (get.clone(), ui.as_weak());
    ui.on_open_member(move |npub| {
        let Some(c) = g() else { return };
        let Some(ui) = weak.upgrade() else { return };
        ui.set_dialog_error("".into());
        ui.set_dialog_done("".into());
        ui.set_dialog_busy(false);
        ui.set_roles(ModelRc::default());
        let npub = npub.to_string();
        c.with_chat(async move |chat, ui| {
            let roles = chat.open_member(npub).await;
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_roles(ModelRc::new(VecModel::from(roles)));
                ui.set_dialog("member".into());
            });
        });
    });

    let g = get.clone();
    ui.on_rename(move |name| {
        let name = name.to_string();
        moderate(&g, "renamed", false, async move |app, group, _| {
            app.rename_group(&group, &name).await
        });
    });

    let g = get.clone();
    ui.on_set_about(move |about| {
        let about = about.to_string();
        moderate(&g, "saved", false, async move |app, group, _| {
            app.set_about(&group, &about).await
        });
    });

    let g = get.clone();
    ui.on_set_public(move |public| {
        let label = if public { "now public" } else { "now private" };
        moderate(&g, label, false, async move |app, group, _| {
            app.set_public(&group, public).await
        });
    });

    let g = get.clone();
    ui.on_join_reading(move || {
        if let Some(c) = g() {
            // The banner and its button go away: the keyboard moves to the
            // composer, or the window would take no keys at all.
            c.with_chat(async move |chat, ui| {
                chat.join_reading().await;
                refocus(&ui);
            });
        }
    });

    let g = get.clone();
    ui.on_set_newcomer_wait(move |secs| {
        let secs = u32::try_from(secs).unwrap_or(0);
        // What was set: in a public group the 5-minute floor may still
        // apply, and the setting's own line says so.
        let label = if secs == 0 {
            "newcomer wait set to off".to_owned()
        } else {
            format!("newcomer wait set to {} min", secs / 60)
        };
        moderate(&g, &label, false, async move |app, group, _| {
            app.set_newcomer_wait(&group, secs).await
        });
    });

    let g = get.clone();
    ui.on_save_channel(move |id, name, slowmode, voice| {
        let (id, name) = (id.to_string(), name.to_string());
        let slowmode = u32::try_from(slowmode).unwrap_or(0);
        let label = if id.is_empty() {
            format!("{}{name} created", if voice { "🔊 " } else { "#" })
        } else {
            format!("#{name}: slowmode saved")
        };
        moderate(&g, &label, false, async move |app, group, _| {
            let id = Some(id.as_str()).filter(|i| !i.is_empty());
            app.save_channel(&group, id, &name, slowmode, voice)
                .await
                .map(|_| ())
        });
    });

    let g = get.clone();
    ui.on_set_private(move |id, private| {
        let id = id.to_string();
        let label = if private { "mods only" } else { "open to every member" };
        moderate(&g, label, false, async move |app, group, _| {
            app.set_channel_private(&group, &id, private).await
        });
    });

    let g = get.clone();
    ui.on_delete_channel(move |id| {
        let id = id.to_string();
        moderate(&g, "channel deleted", false, async move |app, group, _| {
            app.delete_channel(&group, &id).await
        });
    });

    let g = get.clone();
    ui.on_unban(move |npub, name| {
        let npub = npub.to_string();
        let label = format!("{name} unbanned — they can join again with an invite");
        moderate(&g, &label, false, async move |app, group, _| {
            app.unban(&group, &npub).await
        });
    });

    let g = get.clone();
    ui.on_compact(move || {
        moderate(&g, "compacted", false, async move |app, group, _| {
            app.compact(&group).await.map(|_| ())
        });
    });

    let (g, weak) = (get.clone(), ui.as_weak());
    ui.on_save_roles(move || {
        let Some(ui) = weak.upgrade() else { return };
        let chosen: Vec<String> = ui
            .get_roles()
            .iter()
            .filter(|r| r.chosen)
            .map(|r| r.id.to_string())
            .collect();
        moderate(&g, "roles saved", false, async move |app, group, who| {
            app.set_roles(&group, &who.unwrap_or_default(), &chosen).await
        });
    });

    let g = get.clone();
    ui.on_mute(move |minutes| {
        let minutes = u32::try_from(minutes).ok();
        let label = match minutes {
            None => "unmuted".to_owned(),
            Some(10) => "muted for 10 min".into(),
            Some(60) => "muted for an hour".into(),
            Some(m) => format!("muted for {} h", m / 60),
        };
        moderate(&g, &label, false, async move |app, group, who| {
            app.mute(&group, &who.unwrap_or_default(), minutes).await
        });
    });

    let g = get.clone();
    ui.on_kick(move || {
        moderate(&g, "kicked", true, async move |app, group, who| {
            app.kick(&group, &who.unwrap_or_default()).await
        });
    });

    let g = get;
    ui.on_ban(move || {
        moderate(&g, "banned", true, async move |app, group, who| {
            app.ban(&group, &who.unwrap_or_default()).await
        });
    });
}

/// Files and direct messages (M12-6).
fn wire_files_and_dms(ui: &MainWindow, get: impl Fn() -> Option<Arc<Ctl>> + Clone + 'static) {
    let g = get.clone();
    ui.on_attach(move || {
        let Some(c) = g() else { return };
        c.with_chat(async move |chat, ui| {
            let paths = pick_files().await;
            if !paths.is_empty() {
                chat.attach(paths).await;
            }
            refocus(&ui);
        });
    });

    let g = get.clone();
    ui.on_unattach(move |hash| {
        if let Some(c) = g() {
            let hash = hash.to_string();
            c.with_chat(async move |chat, _| chat.unattach(hash).await);
        }
    });

    let g = get.clone();
    ui.on_show_file(move |f| {
        if let Some(c) = g() {
            let key = f.key.to_string();
            c.with_chat(async move |chat, _| chat.show_file(key).await);
        }
    });

    let g = get.clone();
    ui.on_save_attachment(move |f| {
        let Some(c) = g() else { return };
        let (key, dm, name) = (f.key.to_string(), f.dm, f.name.to_string());
        c.with_chat(async move |chat, ui| {
            // The person chooses where it lands; downloads to start with.
            let to = match std::env::var_os("HIT_TEST_SAVE_DIR") {
                // Debug builds only, as `HIT_TEST_PICK`.
                Some(dir) if cfg!(debug_assertions) => {
                    Some(std::path::PathBuf::from(dir).join(&name))
                }
                _ => {
                    let start = dirs::download_dir()
                        .or_else(dirs::home_dir)
                        .unwrap_or_default();
                    rfd::AsyncFileDialog::new()
                        .set_directory(start)
                        .set_file_name(name)
                        .save_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                }
            };
            if let Some(to) = to {
                chat.save_file(key, dm, to).await;
            }
            refocus(&ui);
        });
    });

    let g = get.clone();
    ui.on_show_dms(move || {
        if let Some(c) = g() {
            c.with_chat(async move |chat, _| chat.show_dms().await);
        }
    });

    let g = get.clone();
    ui.on_open_dm(move |npub| {
        if let Some(c) = g() {
            let npub = npub.to_string();
            c.with_chat(async move |chat, _| chat.open_dm(npub).await);
        }
    });

    let g = get.clone();
    ui.on_send_dm(move |text| {
        if let Some(c) = g() {
            let text = text.to_string();
            c.with_chat(async move |chat, ui| {
                let sent = text.clone();
                if chat.send_dm(text).await {
                    let _ = ui.upgrade_in_event_loop(move |ui| {
                        if ui.get_dm_draft() == sent.as_str() {
                            ui.set_dm_draft("".into());
                        }
                    });
                }
            });
        }
    });

    let g = get.clone();
    ui.on_attach_dm(move || {
        let Some(c) = g() else { return };
        c.with_chat(async move |chat, ui| {
            let paths = pick_files().await;
            if !paths.is_empty() {
                chat.send_dm_files(paths).await;
            }
            refocus(&ui);
        });
    });

    let g = get;
    ui.on_add_contact(move |link| {
        if let Some(c) = g() {
            let link = link.to_string();
            c.dialog_step(async move |chat| chat.add_contact(link).await);
        }
    });
}

/// Voice (M12-7): joining, calling, answering, the call bar's controls,
/// the talk key and the audio settings.
fn wire_voice(ui: &MainWindow, get: impl Fn() -> Option<Arc<Ctl>> + Clone + 'static) {
    use chat::voice::Toggle;

    let g = get.clone();
    ui.on_join_voice(move |channel| {
        if let Some(c) = g() {
            let channel = channel.to_string();
            c.with_chat(async move |chat, _| chat.join_voice(channel).await);
        }
    });

    let g = get.clone();
    ui.on_ring(move |npub| {
        if let Some(c) = g() {
            let npub = npub.to_string();
            c.with_chat(async move |chat, _| chat.ring(npub).await);
        }
    });

    let g = get.clone();
    ui.on_answer(move || {
        if let Some(c) = g() {
            c.with_chat(async move |chat, _| chat.answer().await);
        }
    });

    let g = get.clone();
    ui.on_decline(move || {
        if let Some(c) = g() {
            c.with_chat(async move |chat, _| chat.decline().await);
        }
    });

    let g = get.clone();
    ui.on_leave_call(move || {
        if let Some(c) = g() {
            c.with_chat(async move |chat, _| chat.leave_call().await);
        }
    });

    let toggle = |what: Toggle| {
        let g = get.clone();
        move || {
            if let Some(c) = g() {
                c.with_chat(async move |chat, _| chat.toggle(what).await);
            }
        }
    };
    ui.on_mute_mic(toggle(Toggle::Mute));
    ui.on_deafen(toggle(Toggle::Deafen));
    ui.on_ptt(toggle(Toggle::PushToTalk));

    let g = get.clone();
    ui.on_hold(move |down| {
        if let Some(c) = g() {
            c.with_chat(async move |chat, _| chat.hold(down).await);
        }
    });

    let g = get;
    ui.on_save_audio(move || {
        let Some(c) = g() else { return };
        let Some(ui) = c.ui.upgrade() else { return };
        let (i, o, e) = (
            ui.get_audio_input().to_string(),
            ui.get_audio_output().to_string(),
            ui.get_audio_echo(),
        );
        c.with_chat(async move |chat, ui| {
            let saved = chat.save_audio(i, o, e);
            let _ = ui.upgrade_in_event_loop(move |ui| match saved {
                Ok(()) => ui.set_dialog_done("saved".into()),
                Err(e) => ui.set_dialog_error(e.into()),
            });
        });
    });
}

/// The profile dialog's steps and the zap dialog (M12-8).
fn wire_you_and_zaps(ui: &MainWindow, get: impl Fn() -> Option<Arc<Ctl>> + Clone + 'static) {
    let g = get.clone();
    ui.on_copy_text(move |text| {
        if let Some(c) = g()
            && let Some(ui) = c.ui.upgrade()
        {
            if c.copy(&text) {
                ui.set_dialog_done("Copied.".into());
            } else {
                ui.set_dialog_error("The clipboard did not take it.".into());
            }
        }
    });

    let g = get.clone();
    ui.on_set_my_name(move |name| {
        let name = name.to_string();
        profile_step(&g, async move |chat| chat.set_my_name(name).await);
    });
    let g = get.clone();
    ui.on_set_address(move |a| {
        let a = a.to_string();
        profile_step(&g, async move |chat| chat.set_address(a).await);
    });
    let g = get.clone();
    ui.on_connect_wallet(move |conn| {
        let conn = conn.to_string();
        profile_step(&g, async move |chat| chat.connect_wallet(conn).await);
    });
    let g = get.clone();
    ui.on_disconnect_wallet(move || {
        profile_step(&g, async move |chat| chat.disconnect_wallet().await);
    });
    let g = get.clone();
    ui.on_toggle_inbox(move || {
        profile_step(&g, async move |chat| chat.toggle_inbox().await);
    });
    let g = get.clone();
    ui.on_set_public_relays(move |r| {
        let r = r.to_string();
        profile_step(&g, async move |chat| chat.set_public_relays(r).await);
    });
    let g = get.clone();
    ui.on_backup_again(move |password| {
        let password = password.to_string();
        profile_step(&g, async move |chat| {
            const NAME: &str = "hit-enter-key.ncryptsec";
            let to = match std::env::var_os("HIT_TEST_SAVE_DIR") {
                // Debug builds only, as the file dialogs' other stand-ins.
                Some(dir) if cfg!(debug_assertions) => Some(std::path::PathBuf::from(dir).join(NAME)),
                _ => {
                    let start = dirs::document_dir()
                        .or_else(dirs::home_dir)
                        .unwrap_or_default();
                    rfd::AsyncFileDialog::new()
                        .set_directory(start)
                        .set_file_name(NAME)
                        .save_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                }
            };
            let Some(to) = to else { return Ok(String::new()) };
            chat.backup_again(&to, password).await
        });
    });

    // ---- the zap dialog ----
    let g = get.clone();
    ui.on_open_zap(move |id| {
        let Some(c) = g() else { return };
        let id = id.to_string();
        c.with_chat(async move |chat, ui| {
            let Some(who) = chat.open_zap(id).await else { return };
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_zap(ui::ZapUi {
                    step: "pick".into(),
                    who: who.into(),
                    ..ui::ZapUi::default()
                });
                ui.set_zap_amount("1000".into());
                ui.set_zap_comment("".into());
                ui.set_zap_anon(false);
                ui.set_dialog_error("".into());
                ui.set_dialog_busy(false);
                ui.set_dialog("zap".into());
                ui.invoke_zap_quote();
            });
        });
    });

    let g = get.clone();
    ui.on_zap_quote(move || {
        let Some(c) = g() else { return };
        let Some(ui) = c.ui.upgrade() else { return };
        let mut z = ui.get_zap();
        z.has_quote = false;
        ui.set_zap(z);
        ui.set_dialog_error("".into());
        let Some(amount) = zap_amount(&ui) else {
            ui.set_dialog_error("a whole number of sats, above zero".into());
            return;
        };
        c.with_chat(async move |chat, ui| {
            let q = chat.zap_quote(amount).await;
            let _ = ui.upgrade_in_event_loop(move |ui| {
                // Stale: the amount changed while this was asked.
                if zap_amount(&ui) != Some(amount) {
                    return;
                }
                let mut z = ui.get_zap();
                match q {
                    Ok(q) => {
                        z.has_quote = true;
                        z.quote = q.quote.into();
                        z.fee_note = q.fee_note.into();
                        z.privacy = q.privacy.into();
                        z.total = q.total.into();
                        z.wallet = q.wallet;
                    }
                    Err(e) => ui.set_dialog_error(e.into()),
                }
                ui.set_zap(z);
            });
        });
    });

    let g = get.clone();
    ui.on_zap_wallet(move || {
        let Some(c) = g() else { return };
        let Some(ui) = c.ui.upgrade() else { return };
        let Some(amount) = zap_amount(&ui) else { return };
        let (comment, anon) = (ui.get_zap_comment().to_string(), ui.get_zap_anon());
        ui.set_dialog_busy(true);
        ui.set_dialog_error("".into());
        c.with_chat(async move |chat, ui| {
            let sent = chat.zap_from_wallet(amount, comment, anon).await;
            let waiting = match &sent {
                Ok(z) if !z.counted => Some((z.result.clone(), z.fee)),
                _ => None,
            };
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_dialog_busy(false);
                match sent {
                    Ok(sent) => {
                        let mut z = ui.get_zap();
                        z.step = "done".into();
                        z.result = sent.result.into();
                        ui.set_zap(z);
                    }
                    // A zap that failed after the quote: its reason shown.
                    Err(e) => ui.set_dialog_error(e.into()),
                }
            });
            // Paid, not counted yet: the receipt usually comes through the
            // anchor's carrier, seconds later. The answer replaces the
            // "waiting" line only if the dialog still shows it — not a
            // dialog opened since for another zap.
            if let Some((interim, fee)) = waiting {
                let result = chat.wallet_zap_counted(amount, fee).await;
                let _ = ui.upgrade_in_event_loop(move |ui| {
                    let mut z = ui.get_zap();
                    if z.step == "done" && z.result == interim.as_str() {
                        z.result = result.into();
                        ui.set_zap(z);
                    }
                });
            }
        });
    });

    let g = get.clone();
    ui.on_zap_qr(move || {
        let Some(c) = g() else { return };
        let Some(ui) = c.ui.upgrade() else { return };
        let Some(amount) = zap_amount(&ui) else { return };
        let (comment, anon) = (ui.get_zap_comment().to_string(), ui.get_zap_anon());
        ui.set_dialog_busy(true);
        ui.set_dialog_error("".into());
        c.with_chat(async move |chat, ui| {
            let shown = match chat.zap_invoice(amount, comment, anon).await {
                Ok(inv) => show_invoice(&ui, "qr", inv, None),
                Err(e) => {
                    let _ = ui.upgrade_in_event_loop(move |ui| {
                        ui.set_dialog_busy(false);
                        ui.set_dialog_error(e.into());
                    });
                    return;
                }
            };
            if !shown {
                return;
            }
            let result = chat.zap_wait(amount).await;
            match chat.zap_fee_invoice().await {
                Ok(Some(fee)) => {
                    show_invoice(&ui, "fee", fee, Some(result));
                }
                Ok(None) => done(&ui, result),
                Err(e) => done(&ui, format!("{result} The fee could not be asked for: {e}")),
            }
        });
    });

    let g = get.clone();
    ui.on_zap_copy(move || {
        if let Some(c) = g()
            && let Some(ui) = c.ui.upgrade()
        {
            let inv = ui.get_zap().invoice.to_string();
            if !c.copy(&inv) {
                ui.set_dialog_error("The clipboard did not take it.".into());
            }
        }
    });

    ui.on_zap_fee_paid({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                let mut z = ui.get_zap();
                z.step = "done".into();
                ui.set_zap(z);
            }
        }
    });
}

/// The amount typed, if it is a whole number of sats above zero.
fn zap_amount(ui: &MainWindow) -> Option<u64> {
    ui.get_zap_amount()
        .trim()
        .replace(',', "")
        .parse::<u64>()
        .ok()
        .filter(|&a| a > 0)
}

/// Puts an invoice on screen at `step` ("qr" or "fee"); false if the
/// window is gone.
fn show_invoice(
    ui: &slint::Weak<MainWindow>,
    step: &'static str,
    inv: chat::zaps::Invoice,
    result: Option<String>,
) -> bool {
    ui.upgrade_in_event_loop(move |ui| {
        ui.set_dialog_busy(false);
        let mut z = ui.get_zap();
        z.step = step.into();
        z.invoice = inv.bolt11.into();
        z.invoice_sats = inv.sats.into();
        z.qr = slint::Image::from_rgba8(inv.qr);
        if let Some(r) = result {
            z.result = r.into();
        }
        ui.set_zap(z);
    })
    .is_ok()
}

fn done(ui: &slint::Weak<MainWindow>, result: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let mut z = ui.get_zap();
        z.step = "done".into();
        z.result = result.into();
        ui.set_zap(z);
    });
}

/// One of the profile's steps: busy meanwhile, then what it did or why
/// not, and the profile read again.
/// The browse list for `query`: the public relays asked again when
/// `fresh`, else their last answer searched. An answer for a query no
/// longer typed is dropped — they can arrive out of order.
fn browse(get: &impl Fn() -> Option<Arc<Ctl>>, query: String, fresh: bool) {
    let Some(c) = get() else { return };
    if let Some(ui) = c.ui.upgrade() {
        if fresh || ui.get_communities().row_count() == 0 {
            ui.set_browsed(false);
        }
        ui.set_dialog_error("".into());
    }
    c.with_chat(async move |chat, ui| {
        let found = chat.browse(&query, fresh).await;
        let _ = ui.upgrade_in_event_loop(move |ui| {
            if ui.get_browse_query().as_str() != query {
                return;
            }
            ui.set_browsed(true);
            match found {
                Ok(list) => ui.set_communities(ModelRc::new(VecModel::from(list))),
                Err(e) => {
                    ui.set_communities(ModelRc::default());
                    ui.set_dialog_error(e.into());
                }
            }
        });
    });
}

fn profile_step<F, Fut>(get: &impl Fn() -> Option<Arc<Ctl>>, step: F)
where
    F: FnOnce(Arc<chat::Chat>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<String, String>> + Send,
{
    let Some(c) = get() else { return };
    if let Some(ui) = c.ui.upgrade() {
        ui.set_dialog_busy(true);
        ui.set_dialog_error("".into());
        ui.set_dialog_done("".into());
    }
    c.with_chat(async move |chat, ui| {
        let outcome = step(chat.clone()).await;
        let p = chat.profile().await;
        let _ = ui.upgrade_in_event_loop(move |ui| {
            ui.set_dialog_busy(false);
            ui.set_profile(p);
            match outcome {
                Ok(done) => ui.set_dialog_done(done.into()),
                Err(e) => ui.set_dialog_error(e.into()),
            }
        });
    });
}

/// The portal's open dialog, several files allowed. Debug builds take
/// `HIT_TEST_PICK` (paths, `:`-separated) instead: the agent driving two
/// windows must never see — or screenshot — a file dialog
/// (`scripts/gui/README.md`).
async fn pick_files() -> Vec<std::path::PathBuf> {
    if cfg!(debug_assertions)
        && let Some(paths) = std::env::var_os("HIT_TEST_PICK")
    {
        return std::env::split_paths(&paths).collect();
    }
    rfd::AsyncFileDialog::new()
        .pick_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.path().to_path_buf())
        .collect()
}

/// After a native dialog the window may hold no focus at all, and then it
/// gets no keys (WebKitGTK's lesson, true of any toolkit): the composer on
/// screen takes it.
fn refocus(ui: &slint::Weak<MainWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_refocus());
}

/// Runs one moderation step: the dialog busy meanwhile, then `label` or the
/// refusal shown — or, for a kick or a ban, the dialog closed.
fn moderate<F, Fut>(
    get: &impl Fn() -> Option<Arc<Ctl>>,
    label: &str,
    close: bool,
    step: F,
) where
    F: FnOnce(Arc<App>, String, Option<String>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), hit_app_core::state::AppError>> + Send,
{
    let Some(c) = get() else { return };
    if let Some(ui) = c.ui.upgrade() {
        ui.set_dialog_busy(true);
        ui.set_dialog_error("".into());
        ui.set_dialog_done("".into());
    }
    let label = label.to_owned();
    c.with_chat(async move |chat, ui| {
        let outcome = chat.moderate(step).await;
        let _ = ui.upgrade_in_event_loop(move |ui| {
            ui.set_dialog_busy(false);
            match outcome {
                Ok(()) if close => ui.set_dialog("".into()),
                Ok(()) => ui.set_dialog_done(label.into()),
                Err(e) => ui.set_dialog_error(e.into()),
            }
        });
    });
}

/// Whether a key's text types something. Slint names keys that type
/// nothing — arrows, Tab, F-keys — with characters in Unicode's private
/// use area; Enter, Backspace, Escape and Delete are control characters.
fn typed(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_control() && !('\u{e000}'..='\u{f8ff}').contains(&c))
}

impl Ctl {
    /// Runs one of the app's steps on the runtime: busy while it runs, its
    /// error shown if it fails, and the status read again either way.
    fn run<F, Fut>(self: &Arc<Self>, step: F)
    where
        F: FnOnce(Arc<App>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), hit_app_core::state::AppError>> + Send,
    {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_busy(true);
            ui.set_error("".into());
        }
        let c = self.clone();
        self.rt.spawn(async move {
            let outcome = step(c.app.clone()).await;
            if let Err(e) = &outcome {
                c.show_error(e.to_string());
            }
            c.refresh().await;
        });
    }

    /// Reads the status and hands it to the window; starts the event pump
    /// once the node runs.
    async fn refresh(self: &Arc<Self>) {
        let status = self.app.status().await;
        if matches!(&status, Ok(s) if s.stage == Stage::Ready) {
            self.start_chat().await;
        }
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.set_busy(false);
            match status {
                Ok(s) => {
                    if s.stage != Stage::Backup {
                        // Past the backup step the key is never on screen.
                        ui.set_shown_key("".into());
                    }
                    ui.set_status(status_ui(s));
                }
                Err(e) => ui.set_error(e.to_string().into()),
            }
        });
    }

    /// Once the node runs: the chat model, and the pump that feeds it.
    async fn start_chat(self: &Arc<Self>) {
        if let Some(chat) = self.chat.get() {
            // A later refresh (a name just set): read everything again.
            return chat.refresh().await;
        }
        let chat = chat::Chat::new(self.app.clone(), self.ui.clone());
        if self.chat.set(chat.clone()).is_err() {
            return;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        hit_app_core::events::start(sink::Native::sink(tx), &self.pumping, &self.app).await;
        let c = chat.clone();
        self.app
            .attach(self.rt.spawn(async move {
                while let Some(ev) = rx.recv().await {
                    c.on_event(ev).await;
                }
            }))
            .await;
        chat.refresh().await;
    }

    /// Runs `f` against the chat model on the runtime.
    fn with_chat<F, Fut>(self: &Arc<Self>, f: F)
    where
        F: FnOnce(Arc<chat::Chat>, slint::Weak<MainWindow>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        if let Some(chat) = self.chat.get().cloned() {
            let ui = self.ui.clone();
            self.rt.spawn(async move { f(chat, ui).await });
        }
    }

    /// A dialog's step: busy while it runs; closed on success, its error
    /// shown on failure.
    fn dialog_step<F, Fut>(self: &Arc<Self>, f: F)
    where
        F: FnOnce(Arc<chat::Chat>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send,
    {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_dialog_busy(true);
            ui.set_dialog_error("".into());
        }
        self.with_chat(async move |chat, ui| {
            let outcome = f(chat).await;
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_dialog_busy(false);
                match outcome {
                    Ok(()) => ui.set_dialog("".into()),
                    Err(e) => ui.set_dialog_error(e.into()),
                }
            });
        });
    }

    /// From any thread.
    fn show_error(&self, why: String) {
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.set_busy(false);
            ui.set_error(why.into());
        });
    }

    fn copy(&self, text: &str) -> bool {
        let Ok(mut slot) = self.clipboard.lock() else {
            return false;
        };
        if slot.is_none() {
            *slot = arboard::Clipboard::new()
                .inspect_err(|e| tracing::warn!("no clipboard: {e}"))
                .ok();
        }
        slot.as_mut().is_some_and(|c| {
            c.set_text(text)
                .inspect_err(|e| tracing::warn!("copy: {e}"))
                .is_ok()
        })
    }
}

fn status_ui(s: Status) -> StatusUi {
    StatusUi {
        stage: stage_name(s.stage).into(),
        npub: s.npub.unwrap_or_default().into(),
        lost: s.lost.unwrap_or_default().into(),
        name: s.name.unwrap_or_default().into(),
        min_password: i32::try_from(s.min_password).unwrap_or(i32::MAX),
        confirm_chars: i32::try_from(s.confirm_chars).unwrap_or(i32::MAX),
    }
}

fn data_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("HIT_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    dirs::data_dir()
        .map(|d| d.join(BUNDLE))
        .ok_or_else(|| "this system names no data directory for HIT_ENTER".into())
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::New => "new",
        Stage::Backup => "backup",
        Stage::Ready => "ready",
    }
}
