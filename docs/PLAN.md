# HIT_ENTER — Architecture & Build Plan

Self-hosted, local-first group chat over a peer-to-peer network. One binary is
both the client and, optionally, the server. No cloud accounts, no port
forwarding, no bots.

---

## 1. What we are building (and what we are not)

**In scope**
- A desktop app (Tauri v2) that connects to one or more chat servers.
- The same app can *host* a server: flip a switch, your machine now runs a
  "space" others can join — from anywhere, with no router configuration.
- Username + password auth. Nothing else. No email, no phone, no OAuth.
- Text channels, invite tickets, message history stored on disk.
- Everything readable offline, because every client keeps its own copy.

**Explicitly not in scope** (say no early, stay fast)
- Bots, apps, slash-command platforms, webhooks.
- Voice/video. (Revisit after v1.0. It is a whole second project.)
- A central directory, global accounts, or any service *we* operate.
- Roles/permission matrices beyond `owner / member`.
- Reactions, threads, rich embeds — deliberately deferred to M7.
- **End-to-end encryption.** Messages are stored in plaintext and the host can
  read them. This is a decision, not an oversight — see §10.

---

## 2. The two decisions everything else follows from

### 2.1 One binary, two roles

`hit_enter` ships as a single Tauri application containing two subsystems:

- **Client** — always running. Talks to servers over iroh, owns a local SQLite
  mirror of every message it has ever seen.
- **Server** — off by default. When enabled, it opens an iroh `Endpoint` that
  accepts the `hit-enter/0` protocol, backed by an embedded SQLite database.

A user who hosts runs both, and their client dials their own server's
`EndpointId` exactly like a remote client does — iroh resolves that to a
loopback or LAN path automatically. **No special case for the host.** One network
path means one code path to debug.

A headless `hit_enter-serverd` binary falls out of this for free.

### 2.2 iroh is the transport

We do not bind a public port, we do not obtain a TLS certificate, and we do not
ask anyone to configure a router. All of that is delegated to
[iroh](https://www.iroh.computer/) (v1.1.0, Sept 2026).

This is the single biggest simplification available to this project, and §4
explains exactly what it buys and what it costs.

---

## 3. Identity model — three layers, read before writing auth

There are **three** distinct identities in this system. Conflating them is the
easiest way to build something insecure, so they are named up front:

| Layer | What it identifies | Provided by | Lifetime |
|---|---|---|---|
| **EndpointId** | a *device* | iroh (ed25519 public key) | per install |
| **Account** | a *person on one server* | username + Argon2id password | forever |
| **Session** | one *logged-in connection* | random bearer token | days |

**The transport authenticates the device; the application authenticates the
person.** iroh guarantees that bytes claiming to come from EndpointId `X` really
came from the holder of `X`'s secret key — but it says nothing about *who* that
is. That is what the password is for.

Because there is no central service, **accounts are per-server**. Joining two
spaces means two accounts. This is the Mumble / homeserver model, and it is a
consequence of "no cloud", not a shortcut. It means a server owner can never see
credentials for a user's other spaces, and password hashes never leave the
machine that owns them.

### Device enrollment — why this composition is nice

The three layers combine into a genuinely good login story:

1. First join: user enters username + password + invite ticket.
2. On success the server records that account's `EndpointId` as an **enrolled
   device**.
3. Every later connection from that device is authenticated by iroh's key
   exchange alone. **No password prompt, ever again, on that machine.**
4. The password is needed only to enroll a *new* device — and the owner can
   revoke any enrolled device from the server UI.

The user experiences a server list they click into. Neither a stolen password nor
a stolen device key is sufficient on its own to enroll somewhere new.

Do not build cross-server identity in v1. A portable account key is a real design
(M8), but it is a different product decision and must not be smuggled in early.

---

## 4. Reachability — what iroh actually solves

This is where self-hosted chat projects normally die. iroh removes most of the
problem, but "most" is doing real work in that sentence and the difference must
be designed for, not discovered in M5.

### What iroh gives us

- **Hole punching.** Implemented inside the QUIC connection via an
  `n0_nat_traversal` extension. For typical home routers this establishes a
  direct peer-to-peer path with **no port forwarding and no UPnP**.
- **Dial keys, not IPs.** A server is addressed by its `EndpointId` (an ed25519
  public key). Its IP can change, it can move from ethernet to wifi to a
  different city — the invite ticket still works and open connections heal
  rather than drop.
- **QUIC + TLS 1.3** encryption and authentication on every connection, with no
  certificate to obtain, renew, or pin.
- **Relay fallback.** When a direct path cannot be punched, traffic routes
  through a relay server instead. The connection does not fail; it degrades. When
  a direct path later becomes available, iroh "switches to the new best path —
  transparently, without dropping the connection."
- **Discovery.** Given only an `EndpointId`, a peer is located by resolving
  `_iroh.<z32-endpoint-id>.<origin-domain> TXT`. Signed pkarr records carry the
  home relay and direct addresses.

### What it costs — be honest about this

| Concern | Reality | Our mitigation |
|---|---|---|
| Hole punching is not universal | Symmetric NAT and CGNAT (common on mobile and some ISPs) defeat it; those connections fall back to relay | Accept it. The connection still works, just slower. Surface path status in the UI (§6). |
| Default relays are n0's | Free, rate-limited, and documented as having "no uptime or performance guarantees" — for dev/testing, not production | Ship with them for convenience; make relay config first-class from M4 and document self-hosting in M6 |
| Default discovery is n0's DNS | `dns.iroh.link` is operated by n0.computer | Offer opt-in BitTorrent Mainline DHT publishing as an alternative; mDNS needs no infrastructure at all |
| Relay operators see metadata | Who talks to whom, and when | **Message content is end-to-end encrypted — a relay cannot read it.** State this plainly in the docs rather than implying relays see nothing. |

### The sovereignty ladder

The project's pitch is "nobody can take this away from you", so every rung must
be reachable without our involvement:

1. **LAN only** — mDNS discovery, zero external infrastructure. Works with the
   internet unplugged.
2. **Default** — n0 discovery + hole punching, relay only as fallback. Zero
   config, works out of the box.
3. **Self-hosted relay** — point the app at your own relay binary (open source,
   in the iroh repo). Documented in M6.
4. **DHT discovery** — publish to Mainline instead of n0's DNS. No central
   resolver at all.

**Never** ship a relay *we* operate as a default. Depending on n0's public relay
is a deliberate, documented, replaceable convenience — not a dependency on us.

---

## 5. Invite tickets

An invite carries everything needed to find, verify, and join a server:

```
hitenter://join?t=<iroh-ticket-b32>&c=K7QP-2M4X-9WTZ
```

- `t` — an iroh ticket: the server's `EndpointId`, its home `RelayUrl`, and any
  known direct addresses.
- `c` — the registration code, gating account creation (§11).

Because the `EndpointId` is a public key, the ticket is **self-verifying**: there
is nothing to pin and no way to be silently redirected to an impostor. The
address hints inside a ticket may go stale as the network changes; the
`EndpointId` never does, and discovery re-resolves from it.

---

## 6. Connection status is a UI feature, not a detail

Since a connection may be direct or relayed, and may silently upgrade from one to
the other, the app shows which:

- **Direct** — peer-to-peer, full speed.
- **Relayed** — working, but routed via a relay. Offer a "why?" link explaining
  NAT, and a pointer to self-hosting a relay.
- **Connecting / Offline** — with the last-known state and queued messages.

This turns iroh's most confusing property into something a user can reason about,
and it is the difference between "the app is slow" and "my ISP uses CGNAT".

---

## 7. Repository layout

A Cargo workspace. The split exists so the server can be compiled and tested
without Tauri, and so protocol types cannot drift between the two sides.

```
hit_enter/
├── Cargo.toml                  # workspace
├── crates/
│   ├── he-proto/               # wire types + framing + validation. NO I/O.
│   │   └── src/{lib,rpc,event,ticket,limits}.rs
│   ├── he-server/              # iroh ProtocolHandler, SQLite, auth
│   │   ├── migrations/         # sqlx migrations, checked in
│   │   └── src/{lib,accept,rpc,auth,devices,db,invite}.rs
│   ├── he-client/              # endpoint mgmt, dialing, local mirror
│   │   └── src/{lib,conn,mirror,keys,discovery}.rs
│   ├── he-serverd/             # headless server binary
│   └── he-cli/                 # debug client: dial a server, run RPCs by hand
├── src-tauri/                  # Tauri v2 shell: commands, tray, lifecycle
├── web/                        # Svelte 5 + Vite + Tailwind
└── docs/
    ├── PLAN.md                 # this file
    ├── PROTOCOL.md             # the wire, in detail — kept current with he-proto
    └── SELF_HOSTING.md         # written during M6, incl. self-hosted relay
```

**Rules:** `he-proto` is the only crate both sides depend on — if a type crosses
the wire it is defined there and nowhere else. Its one concession to I/O is the
off-by-default `io` feature, which carries the async driver for the frame codec
— generic over `tokio::io` traits, innocent of sockets, and there rather than
written twice because two copies of a length-prefix loop is two chances to
disagree about what a truncated frame means. Without the feature the crate is
still types, validation and no runtime. `he-cli` exists because dropping
HTTP costs us `curl`, and we refuse to debug a binary protocol by print
statement.

---

## 8. Data model

Server database (`server.db`). SQLite, WAL mode, `sqlx` with compile-time checked
queries.

IDs are **UUIDv7** stored as TEXT: they sort chronologically, so `ORDER BY id` is
`ORDER BY time`, pagination cursors are just IDs, and there is no sequence to
coordinate. Every TEXT primary key is also spelled `NOT NULL`, because SQLite
otherwise permits NULL in one — a compatibility bug it has documented and will
not fix.

```sql
CREATE TABLE users (
  id            TEXT PRIMARY KEY NOT NULL,
  username      TEXT NOT NULL,
  username_ci   TEXT NOT NULL UNIQUE,   -- lowercased; the real uniqueness key
  password_hash TEXT NOT NULL,          -- argon2id PHC string
  display_name  TEXT,
  is_owner      INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);

-- The bridge between iroh's device identity and our account identity.
CREATE TABLE devices (
  endpoint_id TEXT NOT NULL,            -- iroh EndpointId (z32 public key)
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  label       TEXT,                     -- "Justin's laptop"
  enrolled_at INTEGER NOT NULL,
  last_seen   INTEGER NOT NULL,
  revoked_at  INTEGER,
  PRIMARY KEY (endpoint_id, user_id)
);

CREATE TABLE channels (
  id         TEXT PRIMARY KEY NOT NULL,
  name       TEXT NOT NULL,
  topic      TEXT,
  position   INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE messages (
  id         TEXT PRIMARY KEY NOT NULL, -- UUIDv7 == creation time
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  author_id  TEXT NOT NULL REFERENCES users(id),
  content    TEXT NOT NULL,          -- PLAINTEXT, by design — see §10
  edited_at  INTEGER,
  deleted_at INTEGER                    -- soft delete: clients must be told
);
CREATE INDEX idx_messages_channel_id ON messages(channel_id, id DESC);

CREATE TABLE invites (
  code       TEXT PRIMARY KEY NOT NULL,
  created_by TEXT NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL,
  expires_at INTEGER,                   -- NULL = never
  max_uses   INTEGER,                   -- NULL = unlimited
  uses       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE server_meta (              -- single row, id = 1
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  name        TEXT NOT NULL,
  secret_key  BLOB NOT NULL,            -- iroh SecretKey: THE server's identity
  created_at  INTEGER NOT NULL
);
```

> `server_meta.secret_key` is the most security-critical value in the system.
> Losing it means every existing invite ticket is dead. Leaking it means someone
> can impersonate the server. Back it up; never log it; consider OS-keychain
> storage in M6.

Client database (`mirror.db`), one per user profile:

```sql
CREATE TABLE servers (                  -- every space this client knows
  endpoint_id TEXT PRIMARY KEY,         -- identity key: survives IP changes
  name        TEXT NOT NULL,
  relay_url   TEXT,                     -- last known home relay
  username    TEXT NOT NULL,
  added_at    INTEGER NOT NULL
);
CREATE TABLE cached_messages (
  endpoint_id TEXT NOT NULL,
  id          TEXT NOT NULL,
  channel_id  TEXT NOT NULL,
  author_name TEXT NOT NULL,
  content     TEXT NOT NULL,            -- PLAINTEXT: makes FTS5 search possible
  edited_at   INTEGER,
  deleted_at  INTEGER,
  PRIMARY KEY (endpoint_id, id)
);
CREATE TABLE sync_state (               -- resume point per channel
  endpoint_id TEXT NOT NULL,
  channel_id  TEXT NOT NULL,
  last_id     TEXT NOT NULL,
  PRIMARY KEY (endpoint_id, channel_id)
);
CREATE TABLE outbox (                   -- messages composed while offline
  nonce       TEXT PRIMARY KEY,
  endpoint_id TEXT NOT NULL,
  channel_id  TEXT NOT NULL,
  content     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);
```

`cached_messages` is what makes "all messages saved locally" true for *members*,
not just for the host. Removing a server from the list deletes its mirror.

---

## 9. Wire protocol

There is **no HTTP and no WebSocket**. iroh gives us authenticated, encrypted,
multiplexed QUIC streams; layering HTTP on top would add a dependency and buy
nothing.

- **ALPN:** `hit-enter/0` — bump on any breaking change.
- **Framing:** `u32` little-endian length prefix + JSON body. JSON is chosen for
  debuggability while the protocol is churning; a `postcard` codec goes behind a
  feature flag once M5 is stable and profiling justifies it.
- **Limits:** 1 MiB max frame, enforced in `he-proto` before allocation.

### Stream shapes

QUIC streams are cheap, so each request gets its own:

- **Control stream** — one bi-stream per connection, opened first and held open.
  Carries `Hello` → `Ready`, then becomes the server→client event channel.
- **Request streams** — one bi-stream per RPC: write request, read response,
  close.

### Handshake

```jsonc
// client -> server, on the control stream
{"t":"hello","proto":0,"auth":{"k":"device"}}                      // enrolled
{"t":"hello","proto":0,"auth":{"k":"password","username":"…","password":"…"}}
{"t":"hello","proto":0,"auth":{"k":"register","invite":"…","username":"…","password":"…"}}
```

The client's `EndpointId` is **not** in the payload — iroh already proved it
during the QUIC handshake, and trusting a self-declared one would be the classic
mistake. The server reads it from the connection.

```jsonc
// server -> client
{"t":"ready","user":{…},"channels":[…],"members":[…],"enrolled":true}
{"t":"error","code":"BAD_CREDENTIALS"|"INVITE_INVALID"|"DEVICE_REVOKED"|"RATE_LIMITED"}
```

### Requests (client → server, own stream)

```jsonc
{"t":"send",     "channel_id":"…","content":"…","nonce":"…"}
{"t":"edit",     "id":"…","content":"…"}
{"t":"delete",   "id":"…"}
{"t":"backfill", "channel_id":"…","before":"…","limit":50}
{"t":"resume",   "cursors":{"<channel_id>":"<last_id>"}}
{"t":"invite",   "expires_in":86400,"max_uses":10}
{"t":"typing",   "channel_id":"…"}
```

### Events (server → client, control stream)

```jsonc
{"t":"message",  "message":{…},"nonce":"…"}   // nonce echoed to the sender
{"t":"edited",   "message":{…}}
{"t":"deleted",  "id":"…","channel_id":"…"}
{"t":"presence", "user_id":"…","online":true}
{"t":"revoked"}                                // this device was kicked; disconnect
```

Two details that matter more than they look:

- **`nonce`** — the client renders optimistically the instant you hit enter,
  tagged with a local nonce; the server echoes it back and the client swaps in
  the authoritative row. This is why the app feels fast.
- **`resume`** — reconnect is not a reload. The client sends per-channel cursors
  and receives only what it missed. Flaky wifi costs a few hundred bytes.

Full schema lives in `docs/PROTOCOL.md`, written during M2 and updated in the
same commit as any change to `he-proto`.

---

## 10. Encryption model — exactly what is and is not protected

Three different things get called "encryption" in a chat app. They are decided
independently, and conflating them is how projects end up either insecure or
paralysed. This is the whole answer, in one table:

| Thing | Protected? | Mechanism | Is it a choice? |
|---|---|---|---|
| Passwords | **Yes — hashed** | Argon2id, one-way | No. Never store recoverable passwords. |
| Data in transit | **Yes — encrypted** | QUIC + TLS 1.3 (iroh) | No. QUIC mandates TLS 1.3. |
| Messages at rest | **No — plaintext** | SQLite `TEXT` columns | **Yes, and we choose plaintext.** |
| End-to-end (server can't read) | **No** | — | **Yes, and we choose not to.** |

### Passwords are hashed, not encrypted

Encryption is reversible; hashing is not. A password goes through Argon2id and
the result is stored as a PHC string. **Nobody can read a password back — not an
attacker with the database, not the server owner, not us.** That is the point.
If you ever find yourself able to recover a user's password, something is wrong.

### Transit encryption is mandatory and free

Every connection is QUIC + TLS 1.3, because that is what QUIC *is*. iroh cannot
disable it and neither can we. This is not overhead we chose to take on — it is
the property that makes the rest of the system safe to build:

- It protects the password *during login*, which is the moment it is most
  exposed. A password sent over an unencrypted link is readable by anyone on the
  café wifi, the ISP, or a relay in the path.
- It protects session traffic and message content **from everyone who is not one
  of the two endpoints** — including relay operators (§4).

So "the messages aren't encrypted" is true *on disk*, and false *on the wire*.
Both halves matter and the docs must not blur them.

### Messages at rest are plaintext — deliberately

Message bodies sit in ordinary `TEXT` columns in `server.db` and in every
client's `mirror.db`. Anyone with the file can `sqlite3 server.db 'select
content from messages'` and read everything. **The person hosting a space can
read every message in it.** There is no key, no ratchet, no per-device envelope.

This is a real decision with real benefits, and it is why v1 can be good:

- **Search works.** SQLite FTS5 over the local mirror (M7) gives instant, offline
  full-text search of your own history. Over ciphertext this is a research
  problem, not a feature.
- **Moderation works.** An owner can actually see what is happening in the space
  they are responsible for.
- **Backup and export are `cp server.db`.** No key escrow, no encrypted-backup
  format, no "restored the file but lost the key" support thread.
- **No key management at all.** No device-key sync, no key rotation, no lost-key
  data loss — the single largest source of complexity and of user heartbreak in
  encrypted messengers.
- **It is not a regression.** Discord is not end-to-end encrypted either. We are
  matching the thing being replaced, while removing the part where a company
  owns the disk.

### The trust model, stated plainly

This must appear in the user-facing docs, not just here:

> **The person hosting a space can read every message in it.** Join spaces hosted
> by people you trust. HIT_ENTER protects your messages from your ISP, from the
> network, and from relay operators — not from your host.

That is the same trust model as a self-hosted forum, an IRC server, or an
unencrypted Matrix room. It is honest, it is understandable, and it is the
correct default for a group chat where the host is a friend.

Anyone who wants protection from a compromised *machine* should use full-disk
encryption, which is the OS's job and not ours to reimplement.

### What this forbids

Do not add opportunistic, partial, or "lightly obfuscated" message encryption. A
half-measure buys no real security and costs every benefit listed above. The
choice is plaintext (v1) or a properly designed E2EE scheme (M8+, DMs first) —
nothing in between.

---

## 11. Security baseline

Enforced from M1, non-negotiable:

- **Argon2id** for passwords (`argon2` crate), tuned to ~100 ms on target
  hardware. Never MD5/SHA/hand-rolled. Passwords are **hashed, not encrypted** —
  there is no code path anywhere that recovers a plaintext password (§10).
- **Message bodies are plaintext at rest and that is intentional** (§10). Do not
  "improve" this with ad-hoc encryption. Do not log them either — plaintext in
  the database is a documented trust model; plaintext in a log file that gets
  pasted into a bug report is an accident.
- **Device enrollment binds an account to an `EndpointId`.** Always read that ID
  from the iroh connection, never from the message body.
- **Registration is invite-gated**; invite codes are compared in constant time
  (`subtle`).
- **Rate limiting per `EndpointId` and per username**, with exponential backoff.
  A key is cheap to generate, so also cap concurrent connections and
  unauthenticated handshakes globally.
- **Length caps in `he-proto`**, validated on both sides: message 4000 chars,
  username 3–32 `[a-zA-Z0-9_.-]`, password ≥ 8 chars, channel name ≤ 64.
- **Never log** passwords, secret keys, or message content. Give secret-bearing
  types a `Debug` impl that prints `***`.
- **The iroh secret key is the server's identity.** Treat it like an SSH host
  key: 0600 on disk, never in logs, never in a bug report.
- **Accepting a connection is not authorization.** Any endpoint on the internet
  that knows the `EndpointId` can open a QUIC connection; only a valid `Hello`
  grants access to anything.

---

## 12. Milestones

Each milestone ends in something runnable. No milestone is "refactoring".

### M0 — Skeleton ✅ done
Workspace, crates, Tauri v2 shell, Svelte 5 + Tailwind on Vite, CI running
`fmt` + `clippy -D warnings` + `test`.
**Done when:** `cargo tauri dev` opens a window that says HIT_ENTER.

Shipped: six crates building on Rust 1.98 / edition 2024; `he-proto::limits`
with the shared validation rules; `ServerIdentity` wrapping an iroh `SecretKey`;
`ConnectionPath` for the §6 status indicator; a Svelte 5 window that reads its
version and protocol from Rust over `invoke`, which is what actually proves the
bridge. 10 tests, clippy clean at `-D warnings`.

### M1 — Server core, no network ✅ done
`he-server` as a library. SQLite + migrations. Register/login/enroll logic.
Argon2id, devices, rate limiting — all exercised by direct function calls.
**Done when:** an integration test registers a user, enrolls a device, and
rejects a bad password. Zero frontend and zero networking code written.

Shipped: `Server::{open, create_owner, create_invite, register, login,
authenticate_device, revoke_device}` — the whole of §3's login story as plain
function calls, with the `EndpointId` passed in as an argument precisely because
M2 must read it off the connection. The §8 schema as migration `0001_initial`,
sqlx queries checked against it at compile time (cache in `.sqlx/`, `server.db`
chmod 0600). Argon2id at 19 MiB / t=2 on `spawn_blocking`, with a dummy verify
on the unknown-username path so failed logins cost the same either way.
Constant-time invite matching, redemption atomic in the `UPDATE`, and the whole
registration in one transaction so a taken username cannot burn a use.
Exponential backoff keyed on *both* the `EndpointId` and the username, bounded
in memory. `he-serverd` now creates and opens a real database and prints the
`EndpointId` that invites will point at. 43 tests, clippy clean.

Two things worth knowing before M2 reads this code:

- **The owner account is created locally, never over the network.** Registration
  is invite-gated without exception; the first account cannot be, so it is made
  by the process that already owns the machine, the database and the secret key.
- **One device may hold several accounts on one server**, so device auth takes
  an optional username to disambiguate. The `Hello` in §9 will need to carry it
  — the client's `mirror.db` already stores `servers.username`.

### M2 — Two processes talk over iroh ✅ done
`he-proto` framing. iroh `Endpoint` + `Router` on both sides, ALPN `hit-enter/0`.
Control stream, handshake, send/receive. `he-cli` debug client.
**Done when:** `he-cli` on one machine sends a message to `he-serverd` on
another, addressed only by `EndpointId`, with no router touched.
`docs/PROTOCOL.md` exists.

Shipped: `docs/PROTOCOL.md`, describing exactly what `hit-enter/0` carries
today and what §9 still owes it. Length-prefixed JSON framing in `he-proto`
that checks the 1 MiB cap *before* allocating, and whose parse errors carry a
position but never the body — a `serde_json` message quotes the input it choked
on, and one frame in three has a password in it. `he-server::accept` as an iroh
`ProtocolHandler`: control stream, `Hello` → `Ready`, request streams, and a
`tokio::broadcast` fan-out that echoes the nonce to the sending *connection*
only. `he-client` with a persisted device key, so the second run of `he-cli`
needs no password. Nine end-to-end tests over two real iroh endpoints on
loopback with relays and discovery switched off, plus 84 tests overall, clippy
clean.

Four things worth knowing before M3 builds on this:

- **`Server` still has no idea a network exists.** `accept.rs` translates
  frames into the M1 function calls and owns the broadcast channel; nothing in
  the authentication path learned a new rule. Everything M3 needs is already
  callable without a socket.
- **A refusal has to be flushed before the connection is dropped.** Returning
  from `ProtocolHandler::accept` drops the connection, and a QUIC close
  discards stream data the peer has not acknowledged — which turned "your
  password is wrong" into "connection lost" until `send_final` waited on
  `SendStream::stopped`. Any future frame that is the last one on a stream
  needs the same treatment.
- **The nonce belongs to a connection, not an account.** A second client signed
  into the same account sees its own user's message as somebody else's, which
  is right: it has no optimistic bubble to reconcile.
- **`he-proto` grew an off-by-default `io` feature** for the async driver of
  the frame codec (§7). The default build is still I/O-free and runtime-free.

### M3 — It looks like a chat app
Svelte client: server rail, channel list, message pane, composer. Optimistic send
with nonce reconciliation. Virtualized scrollback with `backfill`. Local mirror
writes.
**Done when:** two app windows hold a conversation, and closing and reopening one
shows history from disk with the network off.

### M4 — Self-hosting, for real
Server toggle in the UI. iroh secret key generation and persistence. Ticket
generation + `hitenter://` deep-link handling. Device enrollment flow. Connection
status indicator (§6). mDNS discovery for LAN. Relay/discovery config surfaced in
settings.
**Done when:** a friend on a different ISP, in a different city, joins your
laptop via a link you pasted into a chat — **and neither of you configured a
router.** *This is the milestone the whole project exists for.*

### M5 — Survives contact with reality
Reconnect with `resume`. Offline `outbox` drain. Edit/delete. Member list +
presence. Typing indicators. Unread markers. Direct↔relay path transitions
handled without dropping state.
**Done when:** you can switch from wifi to a phone hotspot mid-sentence and lose
nothing.

### M6 — Shippable
Owner tools: kick, ban, revoke device, revoke invite, delete channel. Settings.
Keyboard shortcuts. `docs/SELF_HOSTING.md` **including running your own iroh
relay**. Signed builds for Linux/macOS/Windows. `hit_enter-serverd` released
alongside.

**Linux packaging must set `WEBKIT_DISABLE_DMABUF_RENDERER=1`** in the `.desktop`
`Exec=` line (and in any AppImage/Flatpak wrapper). Without it the window dies at
startup on Wayland + proprietary NVIDIA drivers with `Gdk-Message: Error 71
(Protocol error)`. `.cargo/config.toml` sets this for development only; it is
*not* compiled into a released binary, so shipping without the wrapper would
break a large share of Linux desktops. Found the hard way during M0.
**Done when:** someone who is not you hosts a space for their friends from the
README alone.

### M7 — The nice things
Attachments — evaluate `iroh-blobs`, which gives content-addressed, resumable,
verified transfer over the connection we already have. Replies, reactions,
markdown. Full-text search over the local mirror (SQLite FTS5) — instant and
offline. This is where two decisions pay off at once: local-first gives us the
data, and plaintext-at-rest (§10) makes it searchable at all.

### M8 — Post-1.0 questions
Portable cross-server identity. DHT-only discovery mode. Voice (iroh gives us the
transport; the codec pipeline is the work). Federation between spaces.
**Decide none of these before 1.0 ships.**

---

## 13. Crate choices

| Need | Crate | Why |
|---|---|---|
| Transport / NAT traversal | `iroh` 1.1 | QUIC hole punching, key-based addressing, relay fallback |
| Async runtime | `tokio` 1.53 | Required by iroh and sqlx |
| DB | `sqlx` 0.9 (sqlite, runtime-tokio) | Compile-time checked SQL, migrations built in |
| Passwords | `argon2` | RustCrypto, PHC strings, correct defaults |
| IDs | `uuid` (v7) | Time-sortable primary keys, no coordination |
| Serialization | `serde` + `serde_json` | Debuggable now; `postcard` behind a flag later |
| Constant-time | `subtle` | Invite-code and token comparison |
| OS randomness | `getrandom` | Invite codes and bearer tokens; no PRNG state to seed or get wrong |
| Logging | `tracing` + `tracing-subscriber` | Structured, filterable, async-aware |
| Attachments (M7) | `iroh-blobs` | Content-addressed transfer over the existing connection |

**Dropped from the pre-iroh plan** — worth recording so nobody re-adds them:
`axum`, `tokio-tungstenite`, `rustls`, `rcgen` (iroh owns the transport and its
TLS), `mdns-sd` (iroh has local discovery built in), `keyring` for passwords
(device enrollment replaces stored passwords; keyring may still guard the
server's secret key in M6).

Versions are pinned once in `[workspace.dependencies]` so two crates can never
disagree; this table records the *choice*, not the version. Toolchain is Rust
stable (1.98 at M0) on **edition 2024**, pinned in `rust-toolchain.toml`. iroh 1.0 renamed `NodeId`/`NodeAddr` to
`EndpointId`/`EndpointAddr`; **any tutorial using `NodeId` predates 1.0** and
will not compile.

---

## 14. Conventions

- `cargo clippy -- -D warnings` is CI-blocking. No exceptions merged.
- No `unwrap()`/`expect()` outside tests and `main()`. `thiserror` for libraries,
  `anyhow` at the binary edge.
- Every wire-type change touches `he-proto` and `docs/PROTOCOL.md` in the *same*
  commit, and bumps the ALPN if it breaks compatibility.
- Migrations are append-only. Never edit a migration that has shipped.
- Integration tests use a real temp SQLite file and two real iroh endpoints, not
  mocks. Two endpoints in one test process connect over loopback and are fast.

---

## 15. Known open questions

Recorded so they are decided deliberately rather than by accident:

1. **Multiple spaces per host process** — one server = one database = one
   `EndpointId` = one space (v1). Hosting two means two instances. Revisit only
   if it hurts.
2. **Password reset** — no email means no self-serve reset. v1 answer: the owner
   issues a one-time reset code. The owner of a self-hosted space *is* the
   recovery mechanism, and the docs must say so plainly, because it is a real
   trust assumption users are taking on.
3. **Server key backup** — losing `server_meta.secret_key` invalidates every
   invite ticket. Needs an export/restore flow, probably in M6. **Do not ship
   1.0 without one.**
4. **Relay defaults** — ship pointing at n0's public relays (documented as
   best-effort) or force an explicit choice at first host? Leaning: default on,
   with an honest one-liner in the hosting UI. Decide at M4.
5. **Do mirrors ever get pruned?** Unbounded growth is fine for years of text.
   Decide before attachments land in M7.
6. **End-to-end encryption, ever?** Closed for v1 (§10). The only version worth
   revisiting post-1.0 is **DMs first**: 1:1 conversations need neither
   server-side search nor moderation, so they carry the least cost. Group
   channels keep plaintext. Do not open this before 1.0 ships.
