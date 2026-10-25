# Building HIT_ENTER for Windows

The Slint window (`crates/hit-desktop`, binary `hit-enter`) for Windows
x64, as a portable `.zip`. First built on 2026-10-18 on a Windows 11 Pro
laptop; `0.1.0-beta.3`'s zip is on the release.

**Making a release? Start with `docs/RELEASING.md`** — the current one
(0.1.0-beta.5), what to smoke-test, and where the zip goes.

## 0. Short version

```powershell
powershell -ExecutionPolicy Bypass -File scripts\release-windows.ps1 -Version 0.1.0-beta.3
# -> target\release-upload\HIT_ENTER_<version>_x64.zip, its line in SHA256SUMS
```

The script refuses a binary that names the home directory or needs a DLL
that is not Windows' own (the C runtime is linked in: `+crt-static`).

## 1. The machine

All with `winget` (each asks for admin once):

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Kitware.CMake -e
```

- **MSVC**, not GNU: `rust-toolchain.toml` picks `stable`, which is
  `x86_64-pc-windows-msvc` here.
- **CMake** builds libopus (`audiopus_sys` from source; no pkg-config, no
  vcpkg). CMake 4 refuses opus's `cmake_minimum_required` without
  `CMAKE_POLICY_VERSION_MINIMUM=3.5` — the script sets it; for a plain
  `cargo build`/`clippy`/`test`, set it in the shell first
  (`$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'`), or `audiopus_sys` fails.
- **Clone with `core.autocrlf false`**, or every file differs from the
  Linux checkouts by its line endings.

## 2. What is different on Windows

- **Single instance** (`instance.rs`): a named pipe,
  `\\.\pipe\hit-enter-<FNV-1a of the lower-cased data dir>`, created with
  `FILE_FLAG_FIRST_PIPE_INSTANCE`. A second copy opens it as a file,
  writes its links and exits; a busy pipe is retried for a second. The unit
  test runs on Windows too.
- **`hit-enter:` links**: `HKCU\Software\Classes\hit-enter` (no admin),
  `shell\open\command` = `"<exe>" "%1"`, rewritten at every start so a
  moved folder keeps working. Not for test windows (`HIT_DATA_DIR`).
- **Keychain**: keyring 4's Windows Credential Manager store — no change.
- **No console window**: `windows_subsystem = "windows"` in release
  builds; debug builds keep the console for the logs.
- **Icon**: `packaging/windows/hit-enter.rc` → `icons/icon.ico`, compiled
  by `embed-resource` (a Windows-only build dependency).
- **Data**: `%APPDATA%\app.hitenter.desktop`. `device.key` has no
  `0600` (no Unix modes); `%APPDATA%` is the user's own.

## 3. Echo cancellation — not on Windows yet

`webrtc-audio-processing` 0.5 `bundled` builds with autotools
(`libtoolize`), which MSVC has no answer to. 2.x is no better: its bundled
build shells out to `cp -a`, `nm` and meson/ninja, expects `lib*.a`, and
needs LLVM for bindgen. So on Windows `hit-voice` has no canceller
(`device.rs`: `wap` is an empty stub, the setting is ignored). Calls work;
**laptop speakers + mic echo — use a headset**, as the download page says.

Ways to get it back, roughly cheapest first: WASAPI's own communications
processing (the Voice Capture DSP / `AUDCLNT_STREAMOPTIONS` raw off),
patching the 2.x crate's build script for MSVC (`[patch.crates-io]`), or a
prebuilt static library through its non-bundled path
(`WEBRTC_AUDIO_PROCESSING_INCLUDE` / `_LIB`).

## 4. Home paths in the binary

As the other scripts: `--remap-path-prefix` for Rust, and for C (libopus's
assertions carry `__FILE__`) MSVC's undocumented `/d1trimfile:` through
`CFLAGS`. **`audiopus_sys`'s build script does not rerun when CFLAGS
change** — after changing them, `cargo clean --release -p audiopus_sys`.

## 5. Unsigned

No code-signing certificate: SmartScreen says "Windows protected your PC"
until a download has reputation — *More info* → *Run anyway*. A
certificate (OV or EV, or Azure Trusted Signing) is the owner's decision
and money.

## 6. Publishing

As `BUILD_MACOS.md` §7: download the release's `SHA256SUMS`, append the
zip's line, upload both with `--clobber`; the download page's button and
note are in `isciuciustin/hitenter`, deployed with `npx wrangler@4 deploy`
from a machine logged in to Cloudflare (not the Windows laptop — the
owner's choice).

## 7. Not tested yet

- Creating an identity (the Credential Manager), a call, files, zaps — the
  first run only reached the sign-up screen (29 MB working set, 13
  threads) and the single-instance handoff (266 ms).
- A clicked `hit-enter:` link from a browser.
- A second Windows PC, and Windows 10.
