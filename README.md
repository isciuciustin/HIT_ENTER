# HIT_ENTER

**Open-source group chat you host yourself, on a computer you already own.**

No server to rent. No port to forward. No account with anybody — including us,
because there is no us. Your messages live on your disk, in a file you can open.

> **Status: pre-1.0.** Everything described here works and is tested. What is
> not done is the long tail: attachments, replies, reactions and search are
> [M7](docs/PLAN.md#m7--the-nice-things). The builds are not code-signed yet.

---

## Get it

Download from [Releases](../../releases) — the desktop app for Linux, macOS and
Windows, and `he-serverd` for machines with no desktop.

The builds are **not signed with a developer certificate**, so macOS will say
the developer cannot be verified and Windows SmartScreen will warn you. Each
release has a `SHA256SUMS`.

On macOS, drag HIT_ENTER into Applications and open it once; when it is
refused, go to **System Settings → Privacy & Security** and click **Open
Anyway**. If macOS instead says HIT_ENTER **"is damaged and can't be opened"**,
it is not — that is how macOS treats a downloaded app it cannot verify. Clear
the download flag and open it again:

```bash
xattr -cr /Applications/HIT_ENTER.app
```

Or build it: [below](#building-it-yourself).

## Host a space in about a minute

1. Open HIT_ENTER and click **host a space**.
2. Give it a name, and pick a username and password for yourself.
3. Click **invite**, then **copy link**.
4. Paste that link to a friend.

That is genuinely all of it. Your laptop is now serving a space your friends
can join from another city, and neither of you touched a router.

When you close your laptop the space goes down and comes back when you open it.
Everyone keeps their own copy of the conversation in the meantime.

The long version, including running it headless and running your own relay, is
in **[docs/SELF_HOSTING.md](docs/SELF_HOSTING.md)**.

## Join someone's space

Click the link they sent you, or paste it into **+** in the server rail. Pick a
username and password — accounts are per-space, so this one is new — and you
are in.

After that first login this machine is **enrolled**, and you are never asked for
the password again on it. The password only exists to enrol a new machine.

---

## Why it works without a port forward

Networking is [iroh](https://www.iroh.computer/). A space is addressed by a
**public key** rather than an IP address, which buys three things at once:

- **Hole punching.** Two machines behind ordinary home routers negotiate a
  direct path between them. Nobody forwards a port, and no UPnP is involved.
- **Addresses can change.** Move house, change ISP, switch from wifi to a
  hotspot mid-sentence — the invite links still work and open connections heal
  instead of dropping.
- **Nothing to verify.** The address *is* the key. A tampered invite link points
  at a key nobody holds, not at an impostor, so there is no fingerprint for two
  people to compare and no certificate to check.

When two networks refuse to be punched through — some mobile and ISP setups do
— traffic falls back to a **relay**. That is slower, and it is still end-to-end
encrypted: a relay forwards bytes it cannot read. The app shows you which path
you are on, because relayed is *working*, not broken.

The default relays are run by iroh's authors on a best-effort basis. You can
point the app at [your own](docs/SELF_HOSTING.md#running-your-own-relay), or
stay on your LAN where no outside infrastructure is involved at all.

## What is and is not private

Being straight about this matters more than sounding secure.

| | |
|---|---|
| Your password | **Hashed** (Argon2id). No code path here turns one back into a password — not for an attacker with the database, not for the person hosting. |
| Everything on the wire | **Encrypted** (QUIC + TLS 1.3). Your ISP, the café wifi and any relay see ciphertext. |
| Your messages at rest | **Plain text.** The person hosting a space can read every message in it. |

That last row is a decision, not a missing feature. It is what makes instant
offline search, real moderation and one-file backups possible, and it means
there is no encryption key you can lose and take your history with you. It is
the same trust model as any self-hosted forum or IRC server — and the same one
Discord has, minus the company that owns the disk.

**So: join spaces hosted by people you trust.** If you are worried about someone
with physical access to your own machine, turn on full-disk encryption.

## What it does not have

Saying no is most of the design:

- no bots, webhooks, or app platform
- no voice or video
- no roles beyond **owner** and **member**
- no central directory, no global accounts, no service we operate
- no telemetry, no analytics, no crash reporting, no phoning home at all

---

## Building it yourself

You need [Rust](https://rustup.rs/), [Node](https://nodejs.org/) 20+, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your
platform.

```bash
git clone https://github.com/isciuciustin/HIT_ENTER
cd HIT_ENTER
cargo install tauri-cli --version "^2" --locked
(cd web && npm install)      # once: the frontend's dependencies, Vite included
cargo tauri dev
```

Other things you will want:

```bash
cargo run -p he-serverd      # a headless space in ./he-data
cargo run -p he-cli -- --help  # poke the protocol by hand
cargo test --workspace       # 145 tests, no mocks in the ones that matter
```

**On Linux**, WebKitGTK's dmabuf renderer and the proprietary NVIDIA driver do
not get along, and the symptom is a window that never appears. The app sets
`WEBKIT_DISABLE_DMABUF_RENDERER=1` for itself, so this should not bite you — if
you are launching the binary through something exotic and get
`Gdk-Message: Error 71`, that variable is the fix.

### The documentation

- **[docs/PLAN.md](docs/PLAN.md)** — the architecture and the milestone list.
  Start here; it explains *why* before it explains what.
- **[docs/PROTOCOL.md](docs/PROTOCOL.md)** — the exact bytes that cross a
  connection.
- **[docs/SELF_HOSTING.md](docs/SELF_HOSTING.md)** — hosting, backups, relays,
  and what to do when something is wrong.
- **[docs/TESTING.md](docs/TESTING.md)** — running the suite, driving the
  protocol by hand, and getting two windows talking.

### Layout

```
crates/he-proto/    wire types, framing, validation — shared, no I/O
crates/he-server/   protocol handler, SQLite, auth — no Tauri dependency
crates/he-client/   dialing, connection management, the local mirror
crates/he-serverd/  headless server binary
crates/he-cli/      debug client (there is no curl for a QUIC protocol)
src-tauri/          the desktop shell: commands, events, lifecycle
web/                the Svelte frontend
```

Rust · [iroh](https://www.iroh.computer/) · SQLite · Tauri v2 · Svelte 5

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0
([LICENSE-APACHE](LICENSE-APACHE)), at your option.

Contributions are understood to be offered under the same terms.
