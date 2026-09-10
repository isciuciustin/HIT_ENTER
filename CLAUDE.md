# HIT_ENTER

Open-source, self-hosted, local-first group chat over a peer-to-peer network.
Users run their own server on their own machine — **no port forwarding** — and
all messages live on disk in plaintext, on the host and on every member's client.
Username + password auth only. No cloud, no bots, no telemetry, no E2EE.

**The full architecture and milestone plan is in [`docs/PLAN.md`](docs/PLAN.md).
Read it before starting non-trivial work.** The wire protocol is in
`docs/PROTOCOL.md` once M2 lands.

## Tech stack

- **Transport:** [iroh](https://www.iroh.computer/) v1.1 — QUIC + TLS 1.3, hole
  punching, public-key addressing, relay fallback
- **Backend:** Rust — `tokio`, SQLite via `sqlx`
- **App shell:** Tauri v2
- **Frontend:** Svelte 5 (runes) + Vite + TailwindCSS

## Layout

```
crates/he-proto/    wire types + framing + validation — shared, no I/O
crates/he-server/   iroh protocol handler, SQLite, auth — no Tauri dependency
crates/he-client/   dialing, connection mgmt, local message mirror
crates/he-serverd/  headless server binary
crates/he-cli/      debug client (there is no curl for a QUIC protocol)
src-tauri/          Tauri shell: commands, tray, lifecycle
web/                Svelte frontend
```

## Rules that are not negotiable

- **One binary, two roles.** The host's client dials its own server's
  `EndpointId` exactly like a remote client does. Never add a "local host"
  shortcut — one network path means one code path to debug.
- **iroh owns the transport.** No HTTP, no WebSocket, no TLS certificates, no
  bound public ports, no UPnP. Do not reintroduce `axum`/`rustls`/`rcgen`.
- **Three identity layers, never conflated** (PLAN §3): `EndpointId` = device,
  account = person, session = connection. **Always read the peer's `EndpointId`
  from the iroh connection, never from the message body.**
- **Accepting a connection is not authorization.** Anyone who knows the
  `EndpointId` can connect; only a valid `Hello` grants access.
- **Accounts are per-server**, by design. No cross-server identity before 1.0.
- **Passwords are hashed, never encrypted.** Argon2id only. No code path
  anywhere recovers a plaintext password. Registration is invite-gated.
- **Messages are plaintext at rest, by design** (PLAN §10). No end-to-end
  encryption; the host can read their space's messages. Never add partial or
  ad-hoc message encryption — the choice is plaintext or a real E2EE design,
  nothing in between. Transit encryption (QUIC + TLS 1.3) is mandatory and comes
  from iroh; it is not a thing to disable, and it is what protects the password
  during login.
- **Never log** a password, a secret key, or message content. Plaintext in the
  database is a documented trust model; plaintext in a log is an accident.
- **`server_meta.secret_key` is the server's identity** — treat it like an SSH
  host key. Losing it kills every invite ticket ever issued.
- Types that cross the wire live in `he-proto` and nowhere else. Changing one
  updates `docs/PROTOCOL.md` in the same commit, and bumps the ALPN if breaking.
- `cargo clippy -- -D warnings` must pass. No `unwrap()`/`expect()` outside tests
  and `main()`.
- Migrations are append-only. Never edit one that has shipped.
- **SQL is checked at compile time.** `sqlx::query!` macros verify every query
  against the real schema. The cached results live in `.sqlx/` at the workspace
  root and are checked in, so a normal build needs no database; CI sets
  `SQLX_OFFLINE=true` so a stale cache fails the build instead of silently
  reaching for one. Adding or changing a query means regenerating it — see
  Commands.

## iroh gotchas

- **iroh 1.0 renamed `NodeId`/`NodeAddr` → `EndpointId`/`EndpointAddr`.** Any
  tutorial or answer using `NodeId` predates 1.0 and will not compile. Check
  docs.rs, not memory.
- Hole punching is **not** universal — symmetric NAT and CGNAT fall back to a
  relay. That path is slower but works, and it is E2E encrypted. Surface it in
  the UI (PLAN §6); never treat relayed as broken.
- Default relays and DNS discovery are operated by n0 and are best-effort. They
  are a documented, replaceable convenience — self-hosting a relay is a
  supported path (PLAN §4).

## Linux desktop gotcha

On Wayland with the proprietary NVIDIA driver, WebKitGTK's dmabuf renderer kills
the window at startup:

```
Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
```

`.cargo/config.toml` sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`, which fixes
`cargo run` and `cargo tauri dev`. **It does not apply when you execute
`target/debug/hit-enter` directly** — that path needs the variable set by hand,
and release packaging needs it in the `.desktop` `Exec=` line (tracked in M6).

## Commands

```bash
cargo tauri dev                  # run the desktop app
cargo run -p he-serverd          # run a headless server (./he-data, override with --data-dir)
cargo run -p he-cli -- --help    # debug client
cargo test --workspace           # all tests
cargo clippy --workspace -- -D warnings
cargo fmt --all
cd web && npm run dev            # frontend only
```

After adding or editing a `sqlx::query!`, refresh the checked-in query cache
(needs `cargo install sqlx-cli --no-default-features --features sqlite,rustls`):

```bash
export DATABASE_URL="sqlite://$PWD/target/he-server-dev.db"
sqlx database create
sqlx migrate run --source crates/he-server/migrations
cargo sqlx prepare --workspace -- --all-targets   # writes .sqlx/, commit it
```

The dev database under `target/` is scratch — delete it and re-run the two
`sqlx` commands whenever a new migration lands.

## Scope discipline

Deferred on purpose — do not add without an explicit decision: bots/webhooks,
voice/video, role-permission matrices, a central directory, any service we
operate. Saying no to these is the product.
