# HIT_ENTER 1.0

**The next Discord, for public communities**, on decentralized software
(open source, MIT): groups with channels, roles, DMs, text, files, voice, and
zaps. **The north star (owner, 2026-10-22): big public communities — the
Unturned Discord running on HIT_ENTER.** Privacy is no longer the headline
(DMs stay end-to-end encrypted); `DESIGN.md` §0 has what the star asks for. **Nostr is the identity and the data model; iroh is
the wire; every running app is a relay** for its user's groups.

The app is called **HIT_ENTER**. The directory is `HIT_ENTER`, the **public**
repo `isciuciustin/HIT_ENTER` (MIT, since 2026-10-22); work happens here
directly. (Until then it lived in the private `HIT_ENTER_0.1`, now retired.) Crates are `hit-*`, so they never collide with the original HIT_ENTER's `he-*` or
HIT_ENTER_2's `he2-*`.

## Where this is right now

**2026-10-19: v0.1.0-beta.4 is out — Linux, macOS Apple silicon, Windows.**
Public pre-release on `isciuciustin/hitenter` (AppImage, `.deb`, `.dmg`,
`.zip`, one `SHA256SUMS`). New: Twemoji colour reactions (`src/art.rs`,
`ui/art.slint`); the emoji picker's full list has a background (its
`Panel` was unsized); **the orange "your IP is visible to this group"
header line is removed** (owner's call, 2026-10-19 — the site's "Said
plainly" no longer promises a badge); DMs' "No conversations yet" sits
above the list. Leftover `web/` and `src-tauri/` folders deleted from disk
(untracked). Linux built here (`scripts/release-linux.sh`), macOS and
Windows by the owner and added to the same release.

**hitenter.app revamped (2026-10-19)**, layout after about.kick.com in our
own green: sticky nav, big display hero with floating SVG icons, mission,
values cards, a scrolling green ticker, "by the numbers" (32 MB / 62 MB /
0 servers / 8 in a call), features beside a mock window, per-OS download
cards with install notes and checksums in `<details>`, "Said plainly", an
ENTER key CTA, footer. Still one static `public/index.html`: no web fonts,
no scripts. Deployed with `npx wrangler deploy` from `~/Work/hitenter-site`.
**On a new release:** bump the links, versions and sha256 lines there.

**2026-10-18: v0.1.0-beta.3 was out — the Slint app, Linux + macOS.**
Public pre-release on `isciuciustin/hitenter` (`.deb`, AppImage, `.dmg`,
one `SHA256SUMS`), and hitenter.app links it (site `6e6a1c3`). The window
is `crates/hit-desktop` (Slint, binary `hit-enter`); Tauri and `web/` are
gone. Measured on this PC (release builds, PSS of the whole app): **32 MB
on the sign-up screen, 51 MB signed in, 62 MB with a 1,000-message channel
open, ~100 MB with 50 large photos, 55 MB in a call** — beta.2 (Tauri):
264 / 298 / 620 / 734 MB. Discord idle beside it: 533–745 MB, 161
threads (ours: 23–25). On the M1: 81 MB after a call (beta.2 ≈ 470).
Linux packages are built in Ubuntu 22.04 (`scripts/release-linux.sh`,
glibc 2.35); macOS on the M1 (`scripts/release-macos.sh`, ad-hoc signed
— **every update asks for the keychain password** until a stable
signature, `BUILD_MACOS.md` §9). The owner runs beta.3 installed in
`~/.local/bin/hit-enter` on this PC. The story is `PLAN.md` M12 and
`SESSION_LOG.md` "Session of 2026-10-18".

**M0–M10 built on one machine — v0.1.0 waits for a person in another city; M9 waits for people; M10 waits for real wallets.** `docs/DESIGN.md` and
`docs/PLAN.md` were written on 2026-09-27; `docs/SESSION_LOG.md` has what
was decided and why — **check it before reopening a settled question.**

**M0 — spike S3 (voice) is half done** (`spikes/voice/README.md`):

- **Part 1, local — done.** n0's `callme` (iroh 0.33, MIT OR Apache-2.0)
  builds with echo cancellation and connects two copies in 0.44 s, Opus
  both ways. It is built in `~/Work/tries/callme`, **outside this repo, and
  must never be committed here.**
- **Part 2 — done as far as it can be from here (2026-10-10).** Mac ↔ Linux
  on a Bluetooth headset was robotic (the headset, not callme). **Linux ↔
  Linux (laptop ↔ gaming PC), same LAN, headsets: clear, direct, low latency.**
  Laptop speakers + internal mic were robotic with ~12–20 xruns a minute in
  three rounds, cause not isolated. **Not tested:** a call across networks
  (slow village connection; iPhone hotspot unavailable), a clean two-person
  echo test, a busy machine. Provisional verdict and the "what M9 inherits"
  list are in the README.
- **S1 — Nostr over iroh: done, yes (2026-10-10), built twice.** Two
  independent spikes agree: NIP-01 + NIP-77 over iroh works, negentropy syncs
  by difference, own SQLite is a sound store. `spikes/nostr-iroh/` (iroh 1.3,
  persistent key, 5 k-event runs, CLI) and `spikes/nostr-iroh-b/` (iroh 1.1,
  SQL-side filters, forged/oversized-frame/peer-id assertions, two-process
  script). **Neither has crossed a real network or tested authz** — the
  laptop ↔ PC run in the first README is still open; read both "not tested"
  lists before citing S1.
- **S2 (gated gossip) — done, yes, by ALPN (2026-10-10)**:
  `spikes/gossip-gate/`. One gated gossip instance per set of readers; a
  list is not revocation, close live connections; **needs iroh ≥ 1.3**
  (HIT_ENTER_2 is on 1.1). Loopback only — see its "not tested" list.
- **S5 (two endpoints) — done, yes, with separate keys (2026-10-10)**:
  `spikes/two-endpoints/`. A relay-only endpoint beside a direct one leaks
  no IP (also on the real network). The second endpoint costs under 1 MB,
  2 sockets and 0 threads. **One key on two endpoints fails silently**,
  because the newest wins at the relay. Each group lives on one endpoint.
  Loopback paths only — see its "not tested" list.
- **S4 not started** — one real 21-sat zap, the spike nobody has touched
  before.

**Open source, MIT** (owner, 2026-10-22; reversed the 2026-09-27
closed-source decision, `DESIGN.md` §15 q1). Published to the public
`isciuciustin/HIT_ENTER` as a fresh history; every crate is still
`publish = false`. **Never commit secrets** — NWC strings, `nsec`s, wallet
addresses, `*.key`, `.env`: everything here can go public. The site repo
for `hitenter.app` is separate: **`isciuciustin/hitenter` (public)**, local at
`~/Work/hitenter-site`: the hitenter.app page, `fee@hitenter.app`, and
release binaries (`gh release create … --repo isciuciustin/hitenter`; first:
v0.1.0-beta.2, 2026-10-17). **The zap fee is 1%** (`DESIGN.md` §11).

**Workspace skeleton — done (2026-10-10).** Six crates under `crates/`
(`hit-event`, `hit-proto`, `hit-store`, `hit-group`, `hit-node`, `hit-cli`),
empty except for doc comments that name their rules. The rest join with their
milestone. `unwrap_used`/`expect_used` are denied workspace-wide (tests
exempt via `clippy.toml`). `about.toml` accepts **MPL-2.0** (owner's
decision 2026-10-10, for `attohttpc` under iroh's port mapper) and no
GPL-family licence. The spikes' trees (the planned stack) pass it.

**M1 — done (2026-10-10).** `hit-event`: `Person` (generate, `nsec` and
NIP-49 `ncryptsec` import/backup, no `Debug` of the secret), `SecretStore`
with `Keychain` (keyring 4, Secret Service over zbus) and `MemoryStore`,
the device binding (kind **30436**, provisional, *device-countersigned*),
`verify_binding(event, endpoint_id)` and `BindingBook` for revocations.
Wire format: `docs/PROTOCOL.md`.

**M2 — done on one machine (2026-10-10).** `hit-proto` (framing ported from
`he-proto`, the `HIT-BINDING` hello), `hit-store` (SQLite, compile-checked
SQL, replaceable/addressable slots), `hit-group`'s M2 `Roster`, `hit-node`
(relay + sync, one `accept_event` gate, binding renewal, refuses to start
if revoked) and `hit-cli`. Automated test, plus `scripts/m2-by-hand.sh`
with three processes and a `kill -9`. **Not yet run between two machines**
(`docs/TESTING.md` §2).

**M3 — done on one machine (2026-10-11).** Groups with nobody in charge:
`hit-event::group` (NIP-29 kinds, the group key, invite codes),
`hit-group` (`perm.rs` moved from HIT_ENTER_2; `GroupState::fold` — the
group key's snapshot by `seq`, actions on top, the rank rule; `Groups`
judging every event: store, read, gossip), `hit-node` (its own accept loop,
one gated `iroh-gossip` per group with the gate in an `after_handshake`
hook for both directions, joins by invite link, per-target sync tasks) and
the CLI's group commands. Automated test plus `scripts/m3-by-hand.sh` (five
processes, the owner `kill -9`ed). **Not yet run between two machines**.

**M4a — the app, done on one machine (2026-10-11).** M4 is split. `src-tauri`
(crate `hit-app`, binary `hit-enter`) is HIT_ENTER_2's shell over
`hit-node`; the logic is in `src-tauri/src/state.rs`, without Tauri, and
tested there. `web/` is a new, smaller Svelte store with HIT_ENTER_2's
look. Onboarding with a backup step that cannot be skipped (enforced in
Rust), names (kind 0, carried between people who share a group), groups,
channels, messages, members, invites, offline. Two windows driven from the
keyboard (`docs/TESTING.md` §3).

**M4b — done on one machine (2026-10-11).** Reactions (NIP-25) and
withdrawing your own (NIP-09, only kinds 9 and 7, enforced in the store),
presence (kind 20436, ephemeral, gossip only), an online-first member list
in pages, unread counts in `read.json`, `THIRD_PARTY_NOTICES_WEB.md`. S5's
second endpoint deferred; the IP badge stays.

**M5 — moderation, done on one machine (2026-10-11).** Ban/unban (`9001`
tags), mute (`9030`, ours), group edits (`9002`: name, channels and
slowmode, overrides, roles), moderator removal (`9005`), all in the fold
with HIT_ENTER_2's rules; **silences** drop a removed or muted person's
messages that beat the action to a peer. `hit-cli lie` is the lying client
the done-when needs (`m5_done_when.rs`, `scripts/m5-by-hand.sh`). The app
has a member dialog, group settings and a banned banner. Limits in
`PROTOCOL.md`, "Enforcement".

**M6 — anchors, done on one machine (2026-10-11).** `crates/hit-anchor`:
the same node headless, a member of each group it pins (profile `"bot":
true`), invite links name anchors first, compaction on a timer with a held
group key, keys from systemd credentials on a server, and a `ws://` bridge
for NIP-29 clients (NIP-11, NIP-42, read-only, group state translated and
signed by the anchor). `docs/SELF_HOSTING.md`. Still owed: Chachi/0xchat
against a real `wss://` (`TESTING.md` §5).

**M7 — DMs, done on one machine (2026-10-11).** NIP-17 via `nostr`'s
NIP-44/59 (`hit-event/src/dm.rs`); a wrap is kept only by its recipient and
the anchors their inbox list (10050, our `anchor` rows) names; the sender
keeps an outbox copy; never on gossip. `m7_done_when.rs` scans an anchor's
database bytes for the text and the sender. Opt-in inbox in the app.
Public-relay publishing (DMs with other Nostr apps) not built — the
owner's decision.

**M8 — files, done on one machine (2026-10-11).** NIP-92 `imeta` on kind 9
(BLAKE3 + SHA-256), `iroh-blobs` 0.103.1 on our accept loop behind two
gates (a device that speaks for someone; a hash in a message they may
read), fetched whole from whoever answers first, cached by members and
pinned by anchors; the collector's live set is the event store's
`attachments` index.

**M8b — DM files (2026-10-11).** NIP-17 kind 15, AES-256-GCM (`aes-gcm`),
ciphertext-only blobs served to the two people by `blob_grants`; the window
never holds a key. An anchor cannot serve a DM file.

**M9 — voice, built and driven on one machine (2026-10-11); its done-when
(people, two cities) is owed.** `hit-voice`: Opus (system libopus), a
jitter buffer with FEC, cpal devices with WebRTC echo cancellation
(bundled), and our own `hit/voice/1` (one Opus frame per QUIC datagram);
presence is kind 20437; mesh capped at 8; the listener gates the dialling
device and decides every second who may be heard. **1:1 calls from a DM**
on the same ALPN: the caller's binding in the join, every callee device
rings, only someone you share a group with or wrote to can ring you.
**Private voice channels**: presence handed directly to who may view the
room, never gossiped; the owner's *private* box. **No group in common**:
every DM carries the sender's device binding (sealed), and a contact link
(`hit-enter:contact?…`) makes first contact. **Drift compensation**: each
device buffer held at 40 ms by resampling within ±0.5 %, glitch counts in
the call bar. **Not done:** a Bluetooth HFP warning.

**M10 — zaps, built against a fake Lightning network (2026-10-11); S4 and
the done-when with real sats are owed.** `hit-zap` (the only crate with an
HTTP or WebSocket client: LNURL-pay, NIP-47, receipts, the 1% fee, and a
`fake` feature — address servers, NWC wallets and a relay in-process),
`hit-event::zap` (bolt11 fields, NIP-57 receipt checks, the **kind 9436
carrier** that brings a receipt into a group), totals counted only for
receipts signed by the key **the recipient's own profile names**
(`zap_pubkey`), receipts delivered only to the group's anchors' `wss://`
(the anchor takes a 9735 without NIP-42 and carries it in), and the app:
address and wallet in *you*, ⚡ on messages, the dialog showing the fee
before sending, the QR fallback. `m10_done_when.rs` is the done-when on
the fake. **The fee address is `fee@hitenter.app`** (2026-10-17; before, `None`
and nothing charged); debug builds take `HIT_ZAP_FEE_ADDRESS`.

**2026-10-17 — S4 done with real sats; a Linux beta built.** A 21-sat
zap Coinos → Primal was counted in all three nodes through
`wss://anchor.hitenter.app` (a named Cloudflare tunnel `hit-anchor` on
the laptop; the owner owns `hitenter.app`). Wallet of Satoshi publishes no
receipts for our zaps; Primal's invoices don't commit to the request, so
we no longer require it (`TESTING.md` §7b). `scripts/release-linux.sh`
builds `0.1.0-beta.1` (.deb + AppImage) with no home paths in the binary.
**The fee address is `fee@hitenter.app`** (`FEE_ADDRESS`): a static
LNURL-pay file served by `~/Work/hitenter-site` (its own git repo; a
Cloudflare Worker with static assets only, `npx wrangler@4 deploy`) that
forwards to the owner's Wallet of Satoshi. Still owed for zaps: the wallet
path waiting for the carrier, then M10's done-when.

**Session of 2026-10-11 ended with M10 against fakes.** M3–M10 were built
in that one session. `docs/SESSION_LOG.md` has an entry per piece.

**Next session starts with** one of (the owner picks):

0. **Windows — built (2026-10-18)**: `0.1.0-beta.3`'s x64 `.zip` is on
   the release (`scripts/release-windows.ps1`, `docs/BUILD_WINDOWS.md`).
   Single instance by named pipe, `hit-enter:` in `HKCU`, Credential
   Manager, static CRT, unsigned. **No echo cancellation on Windows**:
   `webrtc-audio-processing` does not build under MSVC (the owner chose
   to ship without it; the site says "use a headset"). Owed: the
   canceller (`BUILD_WINDOWS.md` §3), and identity/call/link tests on
   Windows (§7). The site's Windows button is pushed to
   `isciuciustin/hitenter` but deployed from the owner's main PC — the
   laptop has no Cloudflare login, by choice.
1. **`TESTING.md` §6 — a real call.** Two people first, one on laptop
   speakers, ten minutes: echo, drift (the call bar's *audio glitches*),
   Bluetooth. Then the done-when: four people, two cities, thirty minutes.
2. **`TESTING.md` §2, §4, §5 — the runs v0.1.0 waits for**: two machines
   (the laptop ↔ PC run also carries S1, S2 and S5's T1 over a real
   network), a person in another city, Chachi/0xchat against a real
   `wss://` anchor. Then tag **v0.1.0**.
3. **`TESTING.md` §7b — real zaps**: S4 (21 sats) and M10's done-when.
   **Wallets ready (2026-10-12):** Coinos (funded from Kraken) sends over
   NWC, Wallet of Satoshi receives (zap support checked); Alby Hub
   dropped. `cloudflared` is installed for a quick-tunnel `wss://` anchor
   (§7b has the commands). Left: start anchor + tunnel, the owner makes
   the NWC connection, then the zap — **only on the owner's go**.

4. **M11 — hosted anchors** (`PLAN.md`): the business, and what makes a
   `wss://` anchor — which zaps need — one click.
5. **M12 — 10× lighter than Discord, in Slint — switched (2026-10-18).**
   The app is `crates/hit-desktop` (binary `hit-enter`, Slint 1.18, winit
   + software renderer) over `crates/hit-app-core`; **the Tauri/web
   window (`web/`, `src-tauri`) left the tree** at `0.1.0-beta.3`
   (last commit with it: `5a32529`). Linux benchmark in `PLAN.md` M12-9
   (native 62 MB with 1,000 messages, ~100 MB with 50 photos; web 620 /
   734). **Next: beta.3, Linux + macOS in one release** — Linux built
   (`scripts/release-linux.sh`), then the owner's M1 (`docs/BUILD_MACOS.md`
   §0: `scripts/release-macos.sh`, and links by Apple Event), then publish
   on the owner's go.

**Open, waiting on the owner** (asked, not answered):

- M7: should the DM inbox (anchors keeping your DMs) be **on by default**?
  Should profile and inbox be published to **public relays** (DMs with
  other Nostr apps; an outbound WebSocket from the app)?
- M9: a **Bluetooth HFP warning** (S3: robotic) — not built.
- M10: **zaps in a group without a `wss://` anchor** — refused today;
  a public relay would announce the zap (`DESIGN.md` §15 q9).
- M9: between people with **no group in common, names are npubs** —
  profiles travel only within groups. Sealing the sender's name into DMs
  alongside their device binding would fix it; a protocol choice.

**Don't re-run speaker rounds on the laptop's internal audio** — three were
enough.

## This machine

- **Rust 1.98.1 via `mise`** (`mise use -g rust@stable`, 2026-09-27). The
  machine had no Rust at all. In a non-interactive shell, run
  `eval "$(mise env -s bash)"` before `cargo`.
- **Audio:** PipeWire 1.6.8, ALSA, Opus, `webrtc-audio-processing-1`,
  autotools — every C dependency voice needs is installed. `rtkit` installed
  and PipeWire restarted, so PipeWire itself runs real-time.
- **Processes started from the terminal cannot get real-time threads**:
  `foot` → tmux → Claude inherit `RLIMIT_RTTIME` 0/0 (Hyprland has
  unlimited), and a hard limit cannot be raised. `callme` warns and works
  anyway. Source of the zero not found; not worth chasing for a spike.
- **Testing audio without the owner's mic or speakers** — a silent device,
  and **never record the real microphone without asking**. Anything that
  opens audio with no device named opens the *system defaults*: the real
  mic and speakers. `hit-voice`'s examples refuse without `IN`/`OUT`; the
  app takes `HIT_AUDIO_IN`/`HIT_AUDIO_OUT` (`TESTING.md` §4d):

  ```bash
  MOD=$(pactl load-module module-null-sink sink_name=hit_spike)
  export PULSE_SINK=hit_spike PULSE_SOURCE=hit_spike.monitor
  ./target/release/callme-cli -i pulse -o pulse accept --auto   # etc.
  pactl unload-module $MOD                                      # always
  ```
- **`sudo` needs a password and has no TTY here**, not even through `!`.
  Anything that needs root, the owner runs in their own terminal.
- **`cargo-about` 0.9.2** is in `~/.cargo/bin`. Since 0.9 the binary needs
  `cargo install cargo-about --locked --features cli`; without the feature
  the install "succeeds" and installs nothing. Regenerate the notices after
  any dependency change:
  `cargo about generate about.hbs --workspace --all-features -o THIRD_PARTY_NOTICES.md`.
- **`nostr` 0.45.5 traps** (also in `docs/SESSION_LOG.md`): types are in
  `nostr::prelude`; `Tag::custom("name", values)` takes a plain string;
  sign with `builder.finalize(&keys)` (`FinalizeEvent`); events go to JSON
  with `serde_json`; and **`nip49` does not compile without `nip59`**.
- **The keychain test** writes one throwaway entry to the real keychain and
  deletes it: `cargo test -p hit-event --test m1_done_when -- --ignored`.
  Reading `~/.local/share/keyrings` is (rightly) blocked for the agent.
- **iroh 1.3 traps** (M3): `Router` fixes its ALPNs when spawned, so
  per-group gossip ALPNs need our own accept loop plus `set_alpns`
  (`hit-node/src/accept.rs`). An accept handler only sees *incoming*
  connections; to gate both directions use an `EndpointHooks::after_handshake`
  hook (`hit-node/src/gossip.rs`). `nostr`: `Kind::from_u16` is `const`;
  `SingleLetterTag::LOWERCASE_H` exists (no `lowercase()`).
- **sqlx:** queries are checked at compile time against
  `crates/hit-store/.sqlx/`, and `.cargo/config.toml` sets `SQLX_OFFLINE`.
  After adding or editing a `sqlx::query!` (sqlx-cli 0.9.0 is installed):

  ```bash
  export DATABASE_URL="sqlite://$PWD/target/hit-store-dev.db"
  rm -f target/hit-store-dev.db && sqlx database create
  sqlx migrate run --source crates/hit-store/migrations
  (cd crates/hit-store && cargo sqlx prepare)   # commit crates/hit-store/.sqlx
  unset DATABASE_URL
  ```
  NIP-01 filters are the exception: `QueryBuilder`, every value bound.
- **The app:** `cargo build --release -p hit-desktop` → `target/release/
  hit-enter` (Slint; no web build, no Node). The `.slint` files compile in
  `build.rs`; a Slint error fails the build with the file and line. Traps
  found building it (M12, also in `docs/PLAN.md`): an id inside an `if`
  cannot be reached from outside it; `changed` does not fire for a value
  set in `init`; a wrapped text needs its width set, or its height counts
  one line; a binding that reads the window's own width can loop — set it
  in `changed width` instead; a new model rebuilds its rows and drops the
  focus — patch models in place; the software renderer draws no colour
  emoji (Noto Emoji, monochrome, is bundled) and no SVG. A border on a rounded rectangle
  is drawn in stairs — every rounded box is `Panel` (`ui/panel.slint`), a
  `Path` (`software-renderer-path`) with `fit: preserve` (the default fit
  keeps the aspect ratio and shrinks a wide box). A bar of buttons over a
  hover area must sit *inside* its own TouchArea, or it flickers.
  Patching a ListView's rows after one is removed keeps the old heights
  (blank gaps): `patch` rebuilds on removal. Colour emoji in reactions are
  Twemoji PNGs (`src/art.rs`). Clicks for driving the window: a uinput
  mouse needs both REL_X and REL_Y, or Hyprland ignores it.
- **Driving the window: `scripts/gui/`** (README there) — launch, onboard
  with a throwaway nsec, focus-guarded typing, one-window screenshots, the
  null sinks, and `stop.sh` to clean up (keychain service
  `hit-enter-gui-test`, data in `target/gui/`). The details:
- **Driving the window** (`docs/TESTING.md` §3): `wtype` types into
  whatever has focus — check `hyprctl activewindow` is the app before every
  keystroke, or it types into the owner's terminal. A new window takes
  focus on spawn. `grim -g` screenshots a window; `hyprctl clients -j`
  has its geometry. Neither GTK file dialog (save, open) takes `wtype`'s
  Enter or Escape — and a screenshot of one shows the owner's files: don't.
  Focus follows the mouse here; refocus with
  `hyprctl dispatch 'hl.dsp.focus({ window = "address:0x…" })'` (this
  Hyprland takes Lua dispatchers).
  Clear `hit-enter-m4-test` after.
- **Focus, the recurring bug** (WebKitGTK M4–M9, and Slint the same): a
  window with nothing focused gets **no keys at all**. Focus is lost whenever the focused
  element disappears or is disabled — a button replaced by a status, a
  banner that closes, a control `disabled` while it saves. Every such
  place must hand focus on (`GroupSettingsDialog`'s `run` restores it
  after `tick()`; in Slint, move focus to *close* before a control that
  vanishes, and refocus the composer after a native dialog). And never
  autofocus something Enter would trigger by accident (the incoming-call
  banner).
- **Restarting an app window**: stop it and *wait for it to exit*
  (`scripts/gui/stop.sh`). One still hanging up takes the next launch's
  arguments through the single-instance socket (`instance.sock` in the
  data dir), and the new process quits.
- **Zaps against the fake network** (`TESTING.md` §7a):
  `examples/fake_network` prints addresses and NWC strings,
  `examples/nwc_pay` pays an invoice like a phone. Real sats only with the
  owner, and never without asking.
- **Audio in the app:** `HIT_AUDIO_IN` / `HIT_AUDIO_OUT` beat the audio
  settings (device ids, e.g. `pulse`), and the ALSA `pulse` device follows
  `PULSE_SOURCE` / `PULSE_SINK` — how each window gets its own null sinks.
  `hit-voice`'s examples refuse to open audio without `IN`/`OUT`.
- **The native window's Linux release**: `scripts/release-native-linux.sh
  <version>` builds inside Ubuntu 22.04 (Docker) so the binary needs glibc
  2.35, not this machine's 2.43 — a build made here runs on nothing but
  rolling distributions (true of the Tauri betas). Test the .deb with
  `docker run … debian:bookworm` and `xvfb-run`.
- **Before every push, check the diff for secrets** — the code is public
  (MIT) and `isciuciustin/HIT_ENTER` gets it.

## Read first

| | |
|---|---|
| `docs/DESIGN.md` | the system — identity, the wire, authority without a server (§6), encryption, voice, zaps, money, the rules |
| `docs/PLAN.md` | milestones M0–M11, what carries over from HIT_ENTER and HIT_ENTER_2, and what dies |
| `docs/PROTOCOL.md` | every event kind we emit and how a peer verifies it — changes in the same commit as the wire |
| `docs/TESTING.md` | the by-hand tests: three processes on one machine, and two machines (still owed) |
| `docs/SELF_HOSTING.md` | running an anchor on a Pi or a VPS: systemd, credentials, `wss://` |
| `docs/BUILD_MACOS.md` | building, checking and publishing the macOS app on the owner's M1 — written on Linux, not yet run |
| `docs/BUILD_WINDOWS.md` | building the Windows zip: the toolchain, the named pipe, the registry, why there is no echo canceller there, home paths in C |
| `docs/SESSION_LOG.md` | what each session decided, and why — settled questions stay settled |
| `spikes/voice/README.md` | S3: what `callme` proved locally, the two-city recipe, the verdict when it exists |
| `scripts/gui/README.md` | driving two app windows from the agent: the scripts and their rules |
| `../HIT_ENTER_2/CLAUDE.md` | the rules, gotchas and lessons most of this inherits — iroh, iroh-blobs, WebKitGTK focus, Tauri capabilities, sqlx caches |

## Relationship to the other two

- **The original HIT_ENTER** is MIT OR Apache-2.0. Its code may be used
  here; its copyright notice goes in `THIRD_PARTY_NOTICES.md`. Its old
  `v0.1.0`/`v0.1.1` tags were removed from GitHub on 2026-10-22; its full
  history is in `~/Work/HIT_ENTER_original.bundle` (`git clone` it).
  The public `isciuciustin/HIT_ENTER` repo *is* this code now.
- **HIT_ENTER_2** (`../HIT_ENTER_2`) is proprietary and the owner's. Its
  code may move here — which publishes it under MIT; record each piece that
  moves in `docs/SESSION_LOG.md`.

## Rules that are not negotiable

- **Read the peer's `EndpointId` from the iroh connection, never from a
  message body.** A device binding is valid only on the connection whose
  `EndpointId` it names. `verify_binding` has no signature-only entry point.
- **Verify every event before storing, forwarding or showing it** — signature
  first, then `hit-group`'s permissions for its author.
- **Every authorization decision goes through `hit-group`**, a pure function
  every peer runs on every event.
- **Person key (Nostr), group key, device key (iroh): three keys, never
  conflated.**
- **The nsec lives in the OS keychain.** Never logged, never displayed after
  onboarding, never sent, never written to a plain file.
- **Plaintext or a real E2EE design, nothing in between.** DMs are NIP-17.
  Groups are plaintext at rest and the UI says who can read them.
- **No blob served unauthenticated; never `Remote::fetch` on an upload**
  (HIT_ENTER_2 M7).
- **iroh owns the transport.** No HTTP or WebSocket in the chat plane.
  `wss://` only on an anchor that opted in; an HTTP client only in `hit-zap`.
- **HIT_ENTER never holds user funds.** Zaps go wallet to wallet.
- **The 1% fee is a second payment on top, from the sender's wallet — never a
  cut of the zap**, shown before sending, zap first and fee best-effort, and
  the fee payment carries no message id or pubkey (`DESIGN.md` §11).
- **No GPL/AGPL/LGPL dependency** — this is closed-source software.
  `THIRD_PARTY_NOTICES.md` from `cargo about`, as in HIT_ENTER_2.
- **The mirror is written before the UI is told.**
- **One node, many roles** — the app, an anchor and the CLI are the same
  `hit-node` with different policies. No anchor-only path for something an
  app also does.
- Wire types and our event kinds are documented in `docs/PROTOCOL.md` in the
  same commit that changes them. Other Nostr software will see our events.
- `cargo clippy -- -D warnings`, no `unwrap()`/`expect()` outside tests and
  `main()`, append-only migrations, SQL checked at compile time.

## Tone of the docs

As in HIT_ENTER_2: opinionated, shows its arithmetic, recommends rather than
surveys, says the uncomfortable part out loud, and has a "deliberately not"
section and open questions.
