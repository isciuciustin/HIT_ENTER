# Linux packaging

Tauri's stock `.desktop` template is wrong for this app in two ways, and both
of them are silent — the package builds, installs, and then misbehaves.

## `Exec=` has to set `WEBKIT_DISABLE_DMABUF_RENDERER=1`

On Wayland with the proprietary NVIDIA driver, WebKitGTK's dmabuf renderer
kills the window at startup:

```
Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
```

`.cargo/config.toml` sets the variable for `cargo run` and `cargo tauri dev`,
but that file is a *development* setting and is not compiled into anything. A
released build that relied on it would fail to open for a large share of Linux
desktops, which is how this was found during M0.

The binary now also sets it for itself at startup (`src-tauri/src/lib.rs`),
which is what covers AppImage, a raw `./hit-enter`, and any packaging format
that does not go through this template. Belt and braces on purpose: the line
here is the one a user can read, and the code is the one that cannot be left
out of a new bundle target.

## `Exec=` has to end in `%u`

Tauri's default template does not include it, but it *does* write
`MimeType=x-scheme-handler/hitenter` from `plugins.deep-link`. Together that
registers this app as the handler for invite links and then launches it
**without the link** — the user clicks an invite and gets an empty window.

Invite links are how anybody joins a space (PLAN §5), so this is not a
cosmetic bug.

`StartupWMClass` deliberately stays the bare binary name: it is matched against
the window's class, which `env` does not change.
