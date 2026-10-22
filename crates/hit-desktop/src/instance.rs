//! One window per data directory — what Tauri's single-instance plugin did.
//!
//! A clicked `hit-enter:` link starts a *second* copy of the app with the
//! URL as an argument. That copy finds the first by a Unix socket in the
//! data directory, hands it the links, and exits; the first raises its
//! window and opens the join or contact dialog. Scoped to the data
//! directory: two processes with different ones are two devices, and run
//! side by side (the two-window recipe in `scripts/gui`).
//!
//! On Windows the socket is a named pipe whose name is a hash of the data
//! directory (pipes live in their own namespace, not in the directory).

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// The links among the process's arguments — by scheme, because arguments
/// mix flags, paths and URLs.
pub fn links_in(args: impl Iterator<Item = String>) -> Vec<String> {
    args.filter(|a| {
        a.split_once(':')
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(hit_app_core::URL_SCHEME))
    })
    .collect()
}

#[cfg(unix)]
fn socket(dir: &Path) -> PathBuf {
    dir.join("instance.sock")
}

/// `\\.\pipe\hit-enter-<hash>`: FNV-1a of the directory, lower-cased,
/// because Windows paths are not case-sensitive and the pipe must be the
/// same for `C:\Users` and `c:\users`.
#[cfg(windows)]
fn socket(dir: &Path) -> PathBuf {
    let path = dir.to_string_lossy().to_lowercase();
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    PathBuf::from(format!(r"\\.\pipe\hit-enter-{hash:016x}"))
}

/// What `claim` found.
pub enum Claim {
    /// This is the only window: links that reach it arrive at `listen`.
    First(Listener),
    /// Another window has this data directory; it has the links now.
    Handed,
}

#[cfg(unix)]
pub struct Listener(std::os::unix::net::UnixListener);

/// The pipe's one instance; each other copy connects, writes, and closes.
#[cfg(windows)]
pub struct Listener(std::fs::File);

#[cfg(not(any(unix, windows)))]
pub struct Listener;

/// Becomes the window for `dir`, or hands `links` to the one that is.
#[cfg(unix)]
pub fn claim(dir: &Path, links: &[String]) -> std::io::Result<Claim> {
    use std::os::unix::net::{UnixListener, UnixStream};
    std::fs::create_dir_all(dir)?;
    let path = socket(dir);
    if let Ok(mut other) = UnixStream::connect(&path) {
        // An empty message still says "raise your window".
        other.write_all(links.join("\n").as_bytes())?;
        return Ok(Claim::Handed);
    }
    // Nobody answered: a socket left by a window that crashed.
    let _ = std::fs::remove_file(&path);
    Ok(Claim::First(Listener(UnixListener::bind(&path)?)))
}

#[cfg(windows)]
pub fn claim(dir: &Path, links: &[String]) -> std::io::Result<Claim> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_PIPE_BUSY, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND},
        System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_WAIT,
        },
    };
    let path = socket(dir);
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // A few rounds: the first window serves one copy at a time, and a pipe
    // between two of them answers "busy".
    for _ in 0..20 {
        match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(mut other) => {
                // An empty message still says "raise your window".
                other.write_all(links.join("\n").as_bytes())?;
                return Ok(Claim::Handed);
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            Err(_) => {}
        }
        // Nobody answered. `FIRST_PIPE_INSTANCE` fails if another copy
        // created it meanwhile — then try handing again.
        // SAFETY: `wide` is a NUL-terminated name that outlives the call;
        // no security attributes (the default: this user only).
        let pipe = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if pipe != INVALID_HANDLE_VALUE {
            // SAFETY: a handle just created and owned by nothing else.
            return Ok(Claim::First(Listener(unsafe { std::fs::File::from_raw_handle(pipe) })));
        }
    }
    Err(std::io::Error::other("the other window's pipe stayed busy"))
}

#[cfg(not(any(unix, windows)))]
pub fn claim(_dir: &Path, _links: &[String]) -> std::io::Result<Claim> {
    Ok(Claim::First(Listener))
}

impl Listener {
    /// Calls `arrived` with each other copy's links, on a thread of its own.
    #[cfg(unix)]
    pub fn listen(self, arrived: impl Fn(Vec<String>) + Send + 'static) {
        std::thread::spawn(move || {
            for stream in self.0.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut text = String::new();
                // A link is short; a peer that sends more is not one of us.
                if Read::take(&mut s, 64 * 1024).read_to_string(&mut text).is_ok() {
                    arrived(links_in(text.lines().map(str::to_owned)));
                }
            }
        });
    }

    #[cfg(windows)]
    pub fn listen(self, arrived: impl Fn(Vec<String>) + Send + 'static) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{ERROR_NO_DATA, ERROR_PIPE_CONNECTED},
            System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe},
        };
        std::thread::spawn(move || {
            let pipe = self.0;
            loop {
                // SAFETY: `pipe` is our own pipe handle, open for the whole
                // loop; no OVERLAPPED, so the call blocks until a copy comes.
                let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut()) };
                // A copy that came before this call: still connected, or
                // already gone with its links written (`NO_DATA`) — the
                // pipe holds them either way.
                let early = matches!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(e) if e == ERROR_PIPE_CONNECTED as i32 || e == ERROR_NO_DATA as i32
                );
                if ok != 0 || early {
                    let mut text = String::new();
                    // A link is short; a peer that sends more is not one of us.
                    if Read::take(&mut &pipe, 64 * 1024).read_to_string(&mut text).is_ok() {
                        arrived(links_in(text.lines().map(str::to_owned)));
                    }
                }
                // SAFETY: as above; frees the one instance for the next copy.
                if unsafe { DisconnectNamedPipe(pipe.as_raw_handle()) } == 0 {
                    tracing::warn!("single instance: {}", std::io::Error::last_os_error());
                    return;
                }
            }
        });
    }

    #[cfg(not(any(unix, windows)))]
    pub fn listen(self, _arrived: impl Fn(Vec<String>) + Send + 'static) {}
}

/// Claims `hit-enter:` with the desktop (Linux): a hidden desktop entry
/// that runs this binary with the URL, made the scheme's default. Best
/// effort — pasting a link into the join dialog always works. An AppImage
/// names itself by `$APPIMAGE`: its own path is a mount that goes away.
#[cfg(target_os = "linux")]
pub fn register_scheme() {
    let Some(apps) = dirs::data_dir().map(|d| d.join("applications")) else { return };
    let exe = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let Some(exe) = exe else { return };
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=HIT_ENTER\nTerminal=false\nNoDisplay=true\n\
         MimeType=x-scheme-handler/{0}\nExec=\"{1}\" %u\n",
        hit_app_core::URL_SCHEME,
        exe.display()
    );
    let file = apps.join("hit-enter-handler.desktop");
    if std::fs::read_to_string(&file).is_ok_and(|old| old == entry) {
        return;
    }
    if std::fs::create_dir_all(&apps).and_then(|()| std::fs::write(&file, entry)).is_err() {
        tracing::warn!("could not write {}", file.display());
        return;
    }
    let scheme = format!("x-scheme-handler/{}", hit_app_core::URL_SCHEME);
    if let Err(e) = std::process::Command::new("xdg-mime")
        .args(["default", "hit-enter-handler.desktop", &scheme])
        .status()
    {
        tracing::warn!("xdg-mime: {e}");
    }
}

/// Claims `hit-enter:` for this user (Windows): `HKCU\Software\Classes\
/// hit-enter`, opened with this binary and the URL. No admin rights, and
/// rewritten on every start so a moved install keeps working. Best effort,
/// as on Linux.
#[cfg(windows)]
pub fn register_scheme() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let key = format!(r"Software\Classes\{}", hit_app_core::URL_SCHEME);
    let command = format!("\"{}\" \"%1\"", exe.display());
    let values: [(String, Option<&str>, String); 3] = [
        (key.clone(), None, "URL:HIT_ENTER link".into()),
        (key.clone(), Some("URL Protocol"), String::new()),
        (format!(r"{key}\shell\open\command"), None, command),
    ];
    for (sub, name, data) in values {
        let (sub, data) = (wide(sub.as_ref()), wide(data.as_ref()));
        let name = name.map(|n| wide(n.as_ref()));
        let bytes = u32::try_from(data.len() * 2).unwrap_or(u32::MAX);
        // SAFETY: every pointer is a NUL-terminated UTF-16 string that
        // outlives the call, and `bytes` is `data`'s length with its NUL.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                REG_SZ,
                data.as_ptr().cast(),
                bytes,
            )
        };
        if status != 0 {
            tracing::warn!("could not claim hit-enter: links (registry error {status})");
            return;
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn register_scheme() {}

/// Links on macOS: the system does not start a second copy with the URL;
/// it sends the *running* app a `kAEGetURL` Apple Event — and, when the
/// app is closed, launches it and sends the event then. winit passes
/// neither on, so a handler of our own takes them. Registered after the
/// window exists (winit has made `NSApp`) and before `run` (AppKit
/// finishes launching there, and delivers a launch URL after that).
#[cfg(target_os = "macos")]
pub fn on_apple_event_links(arrived: impl Fn(Vec<String>) + 'static) {
    mac::register(Box::new(arrived));
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
        rc::Retained,
        runtime::{AnyObject, NSObject},
        sel,
    };
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

    /// Four-character codes from the Carbon headers: `'GURL'` is both the
    /// event class (`kInternetEventClass`) and its id (`kAEGetURL`); the
    /// URL is the direct object (`keyDirectObject`, `'----'`).
    const GURL: u32 = u32::from_be_bytes(*b"GURL");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    pub struct Ivars {
        arrived: Box<dyn Fn(Vec<String>)>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "HitEnterLinkHandler"]
        #[ivars = Ivars]
        struct LinkHandler;

        impl LinkHandler {
            #[unsafe(method(handleGetURL:withReplyEvent:))]
            fn handle_get_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                // SAFETY: `paramDescriptorForKeyword:` takes an AEKeyword
                // (a u32) and returns a descriptor or nil.
                let url: Option<Retained<NSAppleEventDescriptor>> =
                    unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
                let Some(url) = url.and_then(|d| d.stringValue()) else { return };
                (self.ivars().arrived)(super::links_in(std::iter::once(url.to_string())));
            }
        }
    );

    pub fn register(arrived: Box<dyn Fn(Vec<String>)>) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("links: not on the main thread; hit-enter: links will not open");
            return;
        };
        let this = LinkHandler::alloc(mtm).set_ivars(Ivars { arrived });
        // SAFETY: NSObject's `init` on a freshly allocated object.
        let handler: Retained<LinkHandler> = unsafe { msg_send![super(this), init] };
        let manager = NSAppleEventManager::sharedAppleEventManager();
        let target: &AnyObject = &handler;
        // SAFETY: the selector is the method defined above, with the
        // signature AppKit calls; the class and id are u32 four-char codes.
        unsafe {
            let _: () = msg_send![
                &*manager,
                setEventHandler: target,
                andSelector: sel!(handleGetURL:withReplyEvent:),
                forEventClass: GURL,
                andEventID: GURL
            ];
        }
        // The manager does not retain its handler: it lives as long as
        // the process.
        std::mem::forget(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_among_arguments() {
        let args = ["--flag", "/some/path", "hit-enter:join?g=x", "HIT-ENTER:contact?p=y", "http://x"]
            .map(String::from);
        assert_eq!(
            links_in(args.into_iter()),
            vec!["hit-enter:join?g=x".to_owned(), "HIT-ENTER:contact?p=y".to_owned()]
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_second_copy_hands_its_links_to_the_first() {
        let dir = std::env::temp_dir().join(format!("hit-instance-{}", std::process::id()));
        let Ok(Claim::First(l)) = claim(&dir, &[]) else { panic!("first") };
        let (tx, rx) = std::sync::mpsc::channel();
        l.listen(move |links| { let _ = tx.send(links); });
        assert!(matches!(claim(&dir, &["hit-enter:join?g=1".into()]), Ok(Claim::Handed)));
        let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(got, vec!["hit-enter:join?g=1".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
