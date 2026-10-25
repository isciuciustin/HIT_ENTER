# Releasing HIT_ENTER

One release, three machines: Linux is built on the laptop, macOS on the
owner's M1, Windows on the owner's PC. Each machine's Claude session
builds its own file and **uploads it to the same draft release**; the
checksums are computed last, from the files actually on the release, so
no session has to pass lines to another. The draft is invisible to the
public until the owner says publish.

Releases live on **`isciuciustin/hitenter`** (the site repo), not on the
code repo. The code repo gets a matching tag.

## Now: 0.1.0-beta.5 (public communities, M13)

**State (2026-10-25):**

- Code: `main` at or after `75bcfb6` — `Cargo.toml` says
  `version = "0.1.0-beta.5"`. Build from that, nothing older.
- Draft release **`v0.1.0-beta.5`** on `isciuciustin/hitenter`, with the
  Linux files: `hit-enter_0.1.0-beta.5_amd64.deb`,
  `HIT_ENTER_0.1.0-beta.5_x86_64.AppImage`, `THIRD_PARTY_NOTICES.md`.
  Release notes are written; no `SHA256SUMS` yet (on purpose — §Last).
- The VPS anchor (`wss://anchor.hitenter.app`) already runs beta.5.
- **Waiting on:** the macOS `.dmg` (§macOS), the Windows `.zip`
  (§Windows), then §Last.

What changed since beta.4, so you know what to look at in a smoke test:
public groups (join with a code-less link; newcomers wait 5 minutes),
read a public group before joining (green banner, *join*), **browse
communities** (a third tab in the *+* dialog, with search and "Bob is
here" lines), a group's *about* in settings, a **public profile**
section in *you* (relays list, "list the public groups I am in"), the
DM inbox on by default. Nothing here is platform-specific code, but none
of it has run on macOS or Windows yet.

**New since beta.4 that touches the network:** the app now opens
WebSockets to public relays (Damus, nos.lol, Primal) on its own — the
profile is published at start, *browse communities* reads from them.
Test windows driven on one machine must set `HIT_PUBLIC_RELAYS=none`, or
every throwaway test identity lands on Damus. The owner's own app
publishing is intended (it is M13's final check: their name in Primal).

### Rules for every session (from `CLAUDE.md`)

- Read `CLAUDE.md` first. **The code is public (MIT):** never commit a
  secret, key, wallet string or `.env`.
- **Never open the microphone or speakers without asking the owner.**
- Anything that asks for the machine's password, the owner runs.
- Upload to the **draft** only. Publishing (`--draft=false`) is the
  owner's call, done once all three platforms are on it (§Last).
- These machines have no `.claude/settings.local.json` allow rules (that
  file is per-machine and git-ignored). If auto mode's safety check
  answers "no verdict", stop and give the owner the command to run with
  `!` — don't look for another way round it.

---

## macOS (the M1) — `HIT_ENTER_0.1.0-beta.5_aarch64.dmg`

Background: `docs/BUILD_MACOS.md` §0 (how the Slint build works on the
Mac, the libopus trap, the ad-hoc signature). The tools are installed
from beta.3/beta.4.

```bash
cd ~/path/to/HIT_ENTER          # the clone on the Mac
git pull
grep '^version' Cargo.toml       # must say 0.1.0-beta.5
cargo build -p hit-desktop       # debug: does it still compile here?
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace           # the internet tests are #[ignore]d
rm -rf target/release-upload     # a stale SHA256SUMS or .dmg from beta.4 must not mix in
scripts/release-macos.sh 0.1.0-beta.5
```

The script refuses a binary that names `$HOME` or links a Homebrew
library; if it refuses, fix that (see `BUILD_MACOS.md` §3), never bypass
it.

**Smoke test, with the owner** — open
`target/release-upload/HIT_ENTER_0.1.0-beta.5_aarch64.dmg`, drag the app
to Applications over the old one, run it:

1. The owner's identity and groups open unchanged. **Every update asks
   for the keychain password once** (ad-hoc signature,
   `BUILD_MACOS.md` §9) — expected; *Always Allow*.
2. *+* → **browse communities** tab: it opens with the search field
   focused and either lists groups or says "None found." (no crash, no
   hang past ~8 s).
3. *you* → scroll to **public profile**: the relays field shows the three
   defaults; the line under the name says it is public.
4. A group the owner owns → settings: the **about** field is under the
   name; *make public / make private* is there.
5. No call unless the owner wants one.

**Upload to the draft:**

```bash
gh release upload v0.1.0-beta.5 --repo isciuciustin/hitenter \
  target/release-upload/HIT_ENTER_0.1.0-beta.5_aarch64.dmg
gh release view v0.1.0-beta.5 --repo isciuciustin/hitenter --json assets --jq '.assets[].name'
```

Upload only the `.dmg` — not this machine's `SHA256SUMS` or notices
(§Last makes one sums file for everything; the notices are already up).
Anything found or fixed on the Mac goes into `BUILD_MACOS.md` and
`docs/SESSION_LOG.md` in the same commit; push only on the owner's go.

---

## Windows (the PC) — `HIT_ENTER_0.1.0-beta.5_x64.zip`

Background: `docs/BUILD_WINDOWS.md` (the toolchain, static CRT, why there
is no echo canceller, the named pipe, home paths). The toolchain is
installed from beta.3/beta.4. PowerShell:

```powershell
cd $HOME\path\to\HIT_ENTER        # the clone on the PC
git pull
Select-String '^version' Cargo.toml   # must say 0.1.0-beta.5
cargo build -p hit-desktop
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
if (Test-Path target\release-upload) { Remove-Item target\release-upload -Recurse -Force }
powershell -ExecutionPolicy Bypass -File scripts\release-windows.ps1 -Version 0.1.0-beta.5
```

The script refuses a binary that names your home folder or links a DLL
users may not have (`vcruntime`, `msvcp`, a vcpkg opus); fix, never
bypass.

**Smoke test, with the owner** — unzip
`target\release-upload\HIT_ENTER_0.1.0-beta.5_x64.zip` somewhere new and
run `hit-enter.exe` (SmartScreen: *More info* → *Run anyway*; it is
unsigned, `BUILD_WINDOWS.md` §5):

1. The owner's identity and groups open (Credential Manager holds the
   key; nothing to re-import).
2. *+* → **browse communities**: search field focused, a list or
   "None found.", no hang past ~8 s.
3. *you* → **public profile** shows the three default relays.
4. A `hit-enter:` link still opens in the running app (the named pipe).
5. Calls: headset only on Windows (no echo canceller), and only if the
   owner wants one.

**Upload to the draft:**

```powershell
gh release upload v0.1.0-beta.5 --repo isciuciustin/hitenter `
  target\release-upload\HIT_ENTER_0.1.0-beta.5_x64.zip
gh release view v0.1.0-beta.5 --repo isciuciustin/hitenter --json assets --jq '.assets[].name'
```

Only the `.zip`. Findings go into `BUILD_WINDOWS.md` and
`docs/SESSION_LOG.md`; push on the owner's go.

---

## Last: checksums, publish, site (any machine with `gh`; the Linux laptop)

When the draft holds the `.deb`, `.AppImage`, `.dmg` and `.zip`:

```bash
d=$(mktemp -d) && cd "$d"
gh release download v0.1.0-beta.5 --repo isciuciustin/hitenter \
  -p '*.deb' -p '*.AppImage' -p '*.dmg' -p '*.zip' -p 'THIRD_PARTY_NOTICES.md'
sha256sum -- * > SHA256SUMS && cat SHA256SUMS
gh release upload v0.1.0-beta.5 --repo isciuciustin/hitenter SHA256SUMS --clobber
```

Computed from the files on the release, so it is what users download.
Then, **on the owner's go only**:

1. Publish: `gh release edit v0.1.0-beta.5 --repo isciuciustin/hitenter --draft=false`.
2. Tag the code: `git tag v0.1.0-beta.5 75bcfb6 && git push origin v0.1.0-beta.5`
   (or the commit the Mac/Windows builds were made from, if a fix
   landed — all three must be built from the same commit).
3. The site (`~/Work/hitenter-site/public/index.html`): every
   `0.1.0-beta.4` link and file name → beta.5, and each platform's
   `sha256` line from the `SHA256SUMS` above. Deploy with
   `npx wrangler deploy` from the owner's main PC (the laptop has no
   Cloudflare login).
4. A line in `CLAUDE.md` "Where this is right now" and in
   `SESSION_LOG.md`.

**Compatibility, said in the notes:** beta.4 apps and anchors refuse a
code-less join and ignore the new `public`/`about` edits — everyone in a
group, and its anchor, should update.
