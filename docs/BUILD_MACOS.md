# Building HIT_ENTER for macOS

Written 2026-10-17 on the Linux machine, for a Claude Code session on the
owner's **MacBook Air (M1, Apple silicon)**. Nothing here has run on a Mac
yet: every step says what to check, and what was found goes back into this
file in the same commit.

**The goal:** `HIT_ENTER_0.1.0-beta.2_aarch64.dmg`, built and run on the
M1, with every feature the Linux beta has, then uploaded to the public
release (§7). Intel Macs are a later step (§8).

## 0. Since 0.1.0-beta.3: the Slint app (start here)

Tauri and the web UI left the tree; §1–§9 below are the Tauri story, kept
for what it found (the libopus trap, the quarantine). Now:

1. `cargo build -p hit-desktop` (debug) — first, does it compile.
   Likeliest breaks: `slint` with `unstable-winit-030` on macOS, `rfd`
   (AppKit dialogs), `arboard`, and `#[cfg(target_os = "linux")]` code
   paths (`instance::register_scheme` is a no-op off Linux; the allocator
   tuning is glibc-only).
2. **Links by Apple Event — code to write here.** macOS does not start a
   second copy with the URL (Linux's way, `src/instance.rs`); it sends the
   *running* app a `kAEGetURL` Apple Event, and winit does not pass it on.
   Register a handler with `NSAppleEventManager` (`objc2`/`objc2-foundation`,
   MIT) before `ui.run()`, and route its URL to `forward_links` — the same
   path the Linux socket uses. Test: `open 'hit-enter:join?…'` with the app
   running, and with it closed (then the event arrives at launch).
3. `scripts/release-macos.sh 0.1.0-beta.3` — builds, checks `otool -L`
   (only `/usr/lib` and `/System`) and that no `$HOME` is in the binary,
   assembles `HIT_ENTER.app` (Info.plist: `app.hitenter.desktop`, the
   microphone text, `CFBundleURLTypes` for `hit-enter`), ad-hoc signs it,
   makes the `.dmg`, appends its SHA-256 to `target/release-upload/SHA256SUMS`.
4. Run the `.app` (not the bare binary: only the bundle carries the
   microphone text). The owner's data opens unchanged —
   `~/Library/Application Support/app.hitenter.desktop`, the same keychain
   service. Check: onboarding or the existing identity, a group, the
   window's memory (`footprint <pid>` or Activity Monitor; M12-9 wants the
   M1 numbers beside Linux's), a call only with the owner.
5. The release: the Linux files are built on the Linux machine
   (`target/release-upload/` there); the `.dmg` and its line join them
   before upload — one release, on the owner's go.

Read `CLAUDE.md` first. Its rules hold on the Mac too, above all: **the
code is public (MIT)** — never commit a secret, a wallet string or a key; and **never open the microphone or speakers
without asking the owner** (§5).

---

## 1. Tools

```bash
xcode-select --install            # Apple's compilers and SDK (a dialog; the owner clicks)
# Homebrew, if missing: https://brew.sh — the owner runs its installer (it asks for the password)
brew install rustup node pkg-config opus autoconf automake libtool cmake
rustup-init -y && source "$HOME/.cargo/env"
rustup default stable
cargo install tauri-cli --version "^2" --locked
cargo install sqlx-cli --no-default-features --features sqlite --locked   # only if SQL changes
```

- `opus` + `pkg-config`: `hit-voice` uses `opus` → `audiopus_sys`, which
  links the system libopus when pkg-config finds one (§3, the trap).
- `autoconf automake libtool`: `webrtc-audio-processing` is built from
  source (`features = ["bundled"]`), as on Linux.
- On the Linux machine Rust came from `mise`; on the Mac `rustup` is fine.
  Ignore `CLAUDE.md`'s `eval "$(mise env -s bash)"`.
- **`sudo` / passwords:** the agent has no TTY; anything that asks for the
  Mac's password, the owner runs in their own terminal.

## 2. First build: does it compile

```bash
git pull
cargo build -p hit-app            # debug first: errors are faster to read
cargo clippy --workspace -- -D warnings
cargo test --workspace            # the keychain test is #[ignore]d; see §4
(cd web && npm ci && npm run check && npm run build)
```

Expected trouble, in order of likelihood:

1. **`webrtc-audio-processing` (bundled, C++)** — the likeliest failure.
   If its build script can't find tools, check `autoreconf`/`libtool` are
   Homebrew's (`brew --prefix libtool`). If it fails on the Apple SDK, read
   the crate's issues for macOS/arm64 before patching anything.
2. **`cpal`** uses CoreAudio on macOS: no extra libraries, should just build.
3. **`keyring` 4** uses the macOS Keychain on its own (`Cargo.toml`
   comments). Nothing to change.
4. Code behind `#[cfg(target_os = "linux")]` (`src-tauri/src/lib.rs`:
   WebKit's dmabuf workaround, registering the `hit-enter:` scheme at run
   time) is skipped on macOS by design: the scheme is declared in the
   bundle and delivered by `on_open_url`.

If something else breaks, fix it in the code **with `cfg` only where the
platform really differs**, and keep clippy clean on both systems.

## 3. The release build — and the libopus trap

The Linux script (`scripts/release-linux.sh`) is the model. Write
`scripts/release-macos.sh` from it:

```bash
#!/usr/bin/env bash
# Builds the macOS release (.dmg, Apple silicon) for upload, with SHA256SUMS.
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTFLAGS="--remap-path-prefix=$HOME/.cargo/registry/src=/cargo --remap-path-prefix=$HOME/.cargo/git/checkouts=/cargo-git --remap-path-prefix=$PWD=/hit-enter --remap-path-prefix=$HOME=/home"
export CFLAGS="-ffile-prefix-map=$PWD=/hit-enter -ffile-prefix-map=$HOME=/home"
export CXXFLAGS="$CFLAGS"
rm -rf target/release/bundle/dmg target/release/bundle/macos
(cd web && npm ci --silent)
(cd src-tauri && cargo tauri build --bundles app,dmg)
bin=target/release/bundle/macos/HIT_ENTER.app/Contents/MacOS/hit-enter
if grep -aqF "$HOME" "$bin"; then echo "the binary still names $HOME — not releasable" >&2; exit 1; fi
if otool -L "$bin" | grep -qE "/opt/homebrew|/usr/local"; then
  echo "links a Homebrew library — users do not have it:" >&2; otool -L "$bin" >&2; exit 1
fi
out=target/release-upload
rm -rf "$out" && mkdir -p "$out"
cp target/release/bundle/dmg/*.dmg "$out/"
cp THIRD_PARTY_NOTICES.md THIRD_PARTY_NOTICES_WEB.md "$out/"
(cd "$out" && shasum -a 256 -- * > SHA256SUMS)
ls -lh "$out"
```

**The trap: `otool -L` must show only `/usr/lib` and `/System` paths.**
On Linux the `.deb` depends on `libopus0`; a Mac has no such package. If
the binary links `/opt/homebrew/opt/opus/lib/libopus.0.dylib`, the app
runs on this Mac and **crashes on every other one**. Fixes, best first:

- Make `audiopus_sys` link opus **statically**. Read its `build.rs` for
  the switch (it has static linking and a bundled-source build; check the
  exact environment variable or feature in the version in `Cargo.lock`),
  set it in the script, and confirm with `otool -L`.
- Or hide pkg-config's opus so it builds from source.
- Last resort: copy the dylib into the bundle (`bundle.macOS.frameworks`)
  and fix its install name. Don't, if static linking works.

Same check for anything else `otool -L` shows from Homebrew.

## 4. What the bundle needs — check each

- **Microphone permission:** `src-tauri/Info.plist` (added 2026-10-17)
  carries `NSMicrophoneUsageDescription`; Tauri merges it into the app's
  Info.plist. Check:
  `plutil -p target/release/bundle/macos/HIT_ENTER.app/Contents/Info.plist | grep Microphone`.
  Without it macOS refuses the mic and voice is silent.
- **The `hit-enter:` scheme:** `plugins.deep-link.desktop.schemes` in
  `tauri.conf.json` should put `CFBundleURLTypes` in the same Info.plist.
- **The notices:** `bundle.resources` copies `THIRD_PARTY_NOTICES*.md` into
  `Contents/Resources`; check they are there.
- **The icon:** `icons/icon.icns` exists.
- **The keychain test** (writes and deletes one throwaway entry; macOS may
  ask the owner to allow it):
  `cargo test -p hit-event --test m1_done_when -- --ignored`.
- **Version:** `tauri.conf.json` says `0.1.0-beta.2`, the same as the Linux
  release. The macOS build joins that release; it is not a new version.

## 5. Running it — the owner's mic and keychain are real

- Run the built app with `open target/release/bundle/macos/HIT_ENTER.app`.
- **The owner onboards themselves.** The agent never types a key into the
  window and never reads the keychain.
- **Never start a call without asking.** On Linux, tests used null sinks;
  macOS has no built-in equivalent (BlackHole exists; installing it needs
  the owner's password). Simplest: the owner makes the voice test
  themselves, with the agent watching logs.
- Data is in `~/Library/Application Support/app.hitenter.desktop`.
- Driving the window from the agent (`scripts/gui/`) is Linux-only
  (Hyprland, `wtype`, `grim`). On the Mac the owner clicks.

**The test**, against the Linux PC or the gaming laptop:

1. Onboard; the backup is suggested in **Documents** and saves (the beta.1
   AppImage bug, fixed in beta.2 — check it stays fixed on macOS).
2. Join a group from an invite link made on Linux (paste it, and try
   clicking a `hit-enter:` link too).
3. Messages both ways; a file both ways; a picture shows inline.
4. A voice call with someone on Linux: the macOS microphone prompt appears
   once; both hear each other; echo and the call bar's glitch count.
5. Memory, for M12: `ps -o rss= -p <pid>` for `hit-enter` and the WebKit
   processes (`WebContent`, `Networking`), against Linux's ≈ 470 MB.

Write what happened into §9 of this file.

## 6. Unsigned: what users will see — say it plainly

`tauri.conf.json` has `"signingIdentity": "-"`: the app is **ad-hoc
signed**, not signed by a developer and not notarized. A downloaded copy
gets the quarantine flag, and macOS says **"HIT_ENTER is damaged and can't
be opened"** — on an M1, an ad-hoc signature is the minimum to run at all,
and it is not enough for a downloaded app.

For the beta, the download page tells Mac users, before they download:

```bash
xattr -dr com.apple.quarantine /Applications/HIT_ENTER.app
```

The real fix is an **Apple Developer account ($99/year)**, a Developer ID
certificate and notarization — `tauri.conf.json`'s `signingIdentity` plus
`APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID` for `cargo tauri build`.
That is the owner's decision and money; don't start it without asking.

## 7. Publishing

The public repo is `isciuciustin/hitenter` (website + binaries, **no
source**), checked out on Linux at `~/Work/hitenter-site`. From the Mac:

```bash
gh release upload v0.1.0-beta.2 --repo isciuciustin/hitenter \
  target/release-upload/HIT_ENTER_0.1.0-beta.2_aarch64.dmg
```

- **`SHA256SUMS` on the release already lists the Linux files.** Download
  it, append the `.dmg`'s line, and upload with `--clobber`. Don't replace
  it with the Mac's own list.
- **Only on the owner's go.** Uploading is publishing.
- The landing page's macOS button and the quarantine note are edited in
  the site repo, not here; tell the owner what to add, or clone
  `isciuciustin/hitenter`, edit `public/index.html` and deploy with
  `npx wrangler@4 deploy` (needs `npx wrangler login` on the Mac first).

Commit `scripts/release-macos.sh`, any code fixes and this file's §9 to
this repo, and push after checking the diff for secrets.

## 8. Later

- **Intel Macs:** `rustup target add x86_64-apple-darwin`, then
  `cargo tauri build --target universal-apple-darwin` for one app that runs
  on both. Every C dependency (opus, webrtc) must build for both
  architectures. Not for the first Mac beta.
- **Signing and notarization** (§6).
- **Auto-update** (Tauri's updater, signed): every platform needs it.

## 9. What happened on the Mac

### 2026-10-17 — first build on the M1: it built, the `.dmg` is on the release

**The machine:** MacBook Air M1, macOS 26.6.2, Command Line Tools (no
Xcode). Everything in §1 was already installed: rustup stable (cargo
1.94.1), Node 25.8.1, Homebrew `opus` 1.6.1, `pkgconf` 2.5.1, autoconf
2.73, automake 1.19, libtool 2.6.2, cmake 3.31.5, `tauri-cli` 2.11.4.
Nothing new was installed.

**What built first time — everything. Nothing broke; no code changed.**

- `(cd web && npm ci && npm run build)`: clean (`svelte-check` runs inside
  `build`).
- `cargo build -p hit-app --features custom-protocol` (debug): 7 m 19 s
  from cold. `webrtc-audio-processing` (bundled, C++), the likeliest
  failure in §2, built without help. So did `cpal`/CoreAudio and `keyring`.
- `scripts/release-macos.sh` (§3's script, as written, committed in
  `7de8578`): release build 6 m 00 s, then the `.app` and the `.dmg`. Tauri
  ad-hoc signs (`Signature=adhoc`, no team) and skips notarization, as §6
  expects.

**The libopus trap did not fire, and why.** `audiopus_sys` 0.2.2 found
Homebrew's opus through pkg-config and linked it **statically** on its own
(`cargo:rustc-link-lib=static=opus`, from
`/opt/homebrew/Cellar/opus/1.6.1/lib`): with no feature or environment
variable set, it links static by default on macOS. No switch needed. **It
depends on Homebrew shipping `libopus.a`** — if a future `opus` bottle
drops it, the link goes dynamic and the script's `otool` check (made
reliable under `pipefail` in `0f19650`) is what
catches it. Keep that check.

**`otool -L`** of `Contents/MacOS/hit-enter`: only system paths, nothing
from Homebrew —

```
/System/Library/Frameworks/{AppKit, AudioToolbox, CoreAudio,
  SystemConfiguration, Security, WebKit, ApplicationServices,
  CoreGraphics, Carbon, CoreVideo, CoreFoundation, QuartzCore,
  Foundation, ColorSync, CoreServices}.framework
/usr/lib/libc++.1.dylib  /usr/lib/libSystem.B.dylib
/usr/lib/libiconv.2.dylib  /usr/lib/libobjc.A.dylib
```

The script's other check passed too: no `$HOME` in the binary.

**§4, the bundle:**

- `NSMicrophoneUsageDescription` is in the app's Info.plist (merged from
  `src-tauri/Info.plist`). ✓
- `CFBundleURLTypes` names the `hit-enter` scheme. ✓
- `Contents/Resources`: `icon.icns`, `THIRD_PARTY_NOTICES.md`,
  `THIRD_PARTY_NOTICES_WEB.md`. ✓ Both notices are byte-identical to the
  Linux release's (same SHA-256), so the release keeps one copy of each.
- `CFBundleShortVersionString` `0.1.0-beta.2`. ✓
- The keychain test (`m1_done_when -- --ignored`): **not run.**

**The upload (§7), on the owner's go:**
`HIT_ENTER_0.1.0-beta.2_aarch64.dmg`, 16 720 271 bytes,
sha256 `3616e71627001bb2d5b5d72b5f800522a7eb8516dbaa4049a998704b8b98ea33`,
added to the `v0.1.0-beta.2` release of `isciuciustin/hitenter`. The
release's `SHA256SUMS` was downloaded, the `.dmg`'s line appended, and
re-uploaded with `--clobber`; the four Linux lines are unchanged. The
release is still titled "HIT_ENTER 0.1.0-beta.2 (Linux)".

**The site:** `public/index.html` got a "macOS · Apple silicon" button,
the §6 quarantine note with the `xattr` command, and the `.dmg`'s
checksum (`a4dfdcd` in `isciuciustin/hitenter`, pushed). **Not deployed**
— `wrangler` is not logged in on the Mac; the owner deploys from Linux.

**Run (§5) — only the debug build, and only briefly.** The owner ran
`target/debug/hit-enter` twice:

- The first run homed on the `euc1-1` iroh relay and synced
  with two peers (`a36129cc7f`, `94f132fa74`: pushed 1 each, fetched 0).
  One peer went away right after; `iroh_gossip` then logged "failed to
  send … send loop terminated" every 3 s for it until exit. Noisy, not
  fatal. `hit_voice::device` logged "default output device changed" and
  "microphone: Device disconnected" — a device change, not a call.
- Both runs exited with code 0 after one to two minutes, with no error
  logged. Most likely the window was closed; not confirmed.

**Still owed on the Mac:**

1. **Run the release `.app`** — nobody has opened it. Then §5's test
   against Linux: the backup in Documents, joining by an invite link and
   by a clicked `hit-enter:` link, messages and files both ways, a picture
   inline, a voice call (the microphone prompt; only with the owner),
   and the memory numbers for M12. A bare debug binary is not a fair voice
   test: it is not a bundle, so it carries no microphone permission text.
2. A downloaded copy, quarantined, to check the "damaged" message and that
   the `xattr` step clears it.
3. `cargo clippy --workspace -- -D warnings` and `cargo test --workspace`
   on macOS — **not run**; and the keychain test.
4. ~~Deploy the site; rename the release~~ — done from Linux, 2026-10-17:
   `a4dfdcd` is live on hitenter.app, and the release is titled "(Linux,
   macOS Apple silicon)" with macOS install notes.

### 2026-10-18 — 0.1.0-beta.3, the Slint app: built, run, a call with Linux

**Built first time.** `cargo build -p hit-desktop` compiled on macOS with
no change (`slint` + `unstable-winit-030`, `rfd`, `arboard`, the
Linux-only `cfg` paths). Clippy (`-D warnings`) and the crate's 7 tests
pass. The one piece of new code is §0 step 2: `instance::on_apple_event_links`
(`crates/hit-desktop/src/instance.rs`) registers a `kAEGetURL` handler with
`NSAppleEventManager` (`objc2` 0.6 / `objc2-foundation` 0.3, the versions
winit and Slint already pull in) after the window exists and before
`ui.run()`, and hands the URL to `raise` + `forward_links`.

**`scripts/release-macos.sh 0.1.0-beta.3`:** release build 4 m 47 s.
`HIT_ENTER_0.1.0-beta.3_aarch64.dmg`, 24 MB, sha256
`db32f3dd4abfc551c580d0cfd5f482290ee9044d267e0ae523b98946baccfae4`;
binary 51 MB, ad-hoc signed, no `$HOME`. `otool -L`: only `/System` and
`/usr/lib` (opus still static, §9 2026-10-17). **Fixed:** the script's
`cp -n` kept an older build's `THIRD_PARTY_NOTICES.md` in
`target/release-upload/`; it now always copies the current one. Clear that
folder of an older version's files before building a new one, or its
`SHA256SUMS` mixes both.

**The keychain trap — every ad-hoc build is a stranger to the keychain.**
The identity saved by beta.2 (Tauri) is readable only by the binary that
wrote it, and an ad-hoc signature changes with each build: beta.3 asked
for the *login keychain password*, which the owner does not know (it has
drifted from the Mac's login password). Users will see this on **every
update** until the app has a stable signature (§6: Developer ID; for a
developer's own Mac, a self-signed certificate). Tested instead on a fresh
identity, which needs no prompt (an entry this build writes, it may read):

```bash
open -n /Applications/HIT_ENTER.app \
  --env HIT_DATA_DIR="$HOME/Library/Application Support/app.hitenter.desktop-mactest" \
  --env HIT_KEYCHAIN_SERVICE=hit-enter-mac-test
```

**The run (the owner, against the Linux PC):** the `.app` from the `.dmg`
installed over beta.2 in `/Applications`, opened without the quarantine
step (built here, not downloaded). Onboarding with a new identity, a new
group, the PC joined it: **messages both ways ✓, a voice call both ways —
the owner: "the sound is ok" ✓.** Memory (`footprint`): 84 MB after
onboarding (peak 96), **81 MB after the call, 160 MB peak during it** —
against beta.2's ≈ 470 MB.

**Links by Apple Event:** `open 'hit-enter:join?…'` (an invite link the
app made) with the app running: **the join dialog opened ✓** — the handler
works. Not yet tried: the same with the app closed (macOS launches it and
sends the event at launch).

**Still owed:** the owner's real identity on this Mac (the keychain
password, or a reset of the login keychain and the
`~/Documents/hit-enter-key.ncryptsec` backup); a downloaded, quarantined
copy; files and pictures between Mac and Linux; the keychain test;
`cargo test --workspace` on macOS; the upload, on the owner's go.

*(Later builds: the date, what broke and the fix, the checksum, results.)*
