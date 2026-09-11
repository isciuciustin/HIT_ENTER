# HIT_ENTER

**Messaging app of the century!**

An open-source, self-hosted group chat. You run the server. Your messages live on
your disk. Nobody can suspend you from a computer you own.

> Status: early development, and it is a chat app now — server rail, channels,
> scrollback, and a composer that sends the instant you hit enter. Hosting from
> inside the app, invite links and LAN discovery are next. See the
> [build plan](docs/PLAN.md) and the [wire protocol](docs/PROTOCOL.md).

## What it is

- **Self-hosted, with no port forwarding.** Flip a switch in the app and your
  laptop is hosting a space your friends can join from anywhere. No router
  config, no VPS, no dynamic-DNS, no account with us — there is no "us".
- **Local-first, in plain text.** Every message is stored on the host *and*
  mirrored to every member's client, in an ordinary SQLite file you can open and
  read yourself. Your history is yours: greppable, backup-able, searchable
  offline, and not locked behind a key you can lose.
- **Simple auth.** A username and a password. No email, no phone number, no
  OAuth, no recovery questions about your first pet. After the first login your
  device is enrolled and you are never asked again.
- **Invite links.** Share one link; your friends are in.
- **No bloat.** No bots, no app platform, no nitro, no ads, no telemetry.

## How it works

One application is both the client and the server. Most people just run the
client and join someone else's space. Anyone who wants to host flips the server
on — their own client then dials it exactly like any other member, so there is no
privileged "host mode" to go wrong.

Networking is handled by [**iroh**](https://www.iroh.computer/). Servers are
addressed by a **public key**, not an IP address, so:

- Connections are established by **hole punching** straight through home routers.
  Nobody forwards a port.
- A server can change wifi networks, ISPs, or cities and its invite links keep
  working.
- Every connection is **QUIC + TLS 1.3** encrypted and authenticated by that key,
  so an invite link is self-verifying — there is no certificate to check and no
  way to be silently redirected to an impostor.

When two machines sit behind networks that refuse to be punched through (some
mobile and ISP setups), traffic falls back to a **relay**. That is slower, but it
is still **end-to-end encrypted — a relay can never read your messages**, it only
forwards them. The default relays are run by iroh's authors on a best-effort
basis; you can point the app at your own, or stay entirely on your LAN where no
outside infrastructure is involved at all.

Servers are independent: your account on your friend's space is *theirs*, and no
central service holds your credentials.

## What is and isn't private

Being straight about this matters more than sounding secure:

- **Your password is hashed** (Argon2id), never stored in a readable form. Nobody
  can recover it — not an attacker with the database, not the person hosting.
- **Everything on the wire is encrypted** (QUIC + TLS 1.3). Your ISP, the café
  wifi, and any relay that forwards your traffic see ciphertext and nothing else.
- **Your messages are stored in plain text**, and **the person hosting a space
  can read every message in it.** There is no end-to-end encryption.

That last point is a deliberate choice, not a missing feature. It is what makes
instant offline search, real moderation, and one-file backups possible, and it
means there is no encryption key you can lose and take your history with it. It
is also the same trust model as any self-hosted forum or IRC server — and it is
what Discord does too, minus the company that owns the disk.

**So: join spaces hosted by people you trust.** If you are worried about someone
with physical access to your own machine, turn on full-disk encryption.

## Tech stack

Rust (`iroh`, `tokio`, SQLite) · Tauri v2 · Svelte 5 + Vite + TailwindCSS

## Building

Start with [`docs/PLAN.md`](docs/PLAN.md) — it is the architecture and the
milestone list. [`docs/TESTING.md`](docs/TESTING.md) is how to run and poke at
all of it locally.

```bash
cargo tauri dev              # the desktop app
cargo run -p he-serverd      # a headless server: creates ./he-data, prints its EndpointId
cargo test --workspace       # everything
```

To watch two processes talk to each other over iroh, addressed by nothing but a
public key:

```bash
cargo run -p he-serverd -- --owner you     # make the owner account
cargo run -p he-serverd -- --invite        # print an invite code and the EndpointId
cargo run -p he-serverd                    # serve

# in another terminal
cargo run -p he-cli -- --server <endpoint-id> --invite <code> --username friend info
cargo run -p he-cli -- --server <endpoint-id> send general "hit enter"
```

Or join it from the app. Two windows on one machine need two data directories,
because the directory is what holds the device key:

```bash
HE_DATA_DIR=/tmp/alice cargo tauri dev
```

## License

TBD — will be an OSI-approved open source license before the first release.
