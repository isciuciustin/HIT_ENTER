# Testing HIT_ENTER locally

Three ways in, in ascending order of effort and descending order of how often
you should reach for them:

1. **[the automated suite](#1-the-automated-suite)** — what CI runs, no setup
2. **[two processes, no GUI](#2-two-processes-no-gui)** — `he-serverd` and
   `he-cli`, the fastest way to exercise the protocol by hand
3. **[two app windows](#3-two-app-windows)** — the real thing, and the only way
   to see the UI

---

## 1. The automated suite

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cd web && npm run check
```

**No database and no environment variables.** `sqlx::query!` checks every query
against a real schema at compile time, but the results are cached in
`crates/he-server/.sqlx` and `crates/he-client/.sqlx` and are checked in, so a
fresh clone builds with nothing installed. CI sets `SQLX_OFFLINE=true` so that
a *stale* cache fails the build rather than silently reaching for a database
that happens to be lying around.

Where the tests are, and what each layer is actually for:

| | tests | what it proves |
|---|---|---|
| `he-proto` | 31 | framing, limits, and that no wire type prints a password |
| `he-server` | 39 | registration, login, enrolment, invites, rate limiting — all by direct function call, no socket |
| `he-client` | 24 | the mirror, the device key, and **11 end-to-end tests over real iroh endpoints** |

> **`cargo test -p he-proto` on its own runs 27, not 31.** The four framing I/O
> tests live behind the off-by-default `io` feature (PLAN §7), so testing that
> crate in isolation silently skips them. `--workspace` enables the feature
> through unification, because `he-server` and `he-client` both ask for it. To
> run them deliberately: `cargo test -p he-proto --features io`. This is the
> shape of trap to watch for whenever a crate gains an optional feature —
> nothing fails, the tests just do not exist.

The end-to-end tests are the interesting ones:

```bash
cargo test -p he-client --test end_to_end -- --nocapture
```

They bind two real iroh endpoints on loopback with relays and discovery
switched off, run a real `he-server` against a real temp SQLite file, and dial
it over QUIC. No mocks — a mock of a QUIC handshake proves nothing (PLAN §14).
They need no network and take well under a second.

Two of them are worth knowing by name, because they are the M3 claim:
`the_mirror_answers_after_the_server_is_gone` and
`a_backfill_page_overlapping_live_events_does_not_duplicate`.

### After changing a query

`sqlx::query!` is checked against a real schema, so a new or edited query needs
its crate's cache regenerated. There are **two schemas** — the server's and the
client mirror's — so there are two databases and two caches, and
`cargo sqlx prepare --workspace` is *not* the right command: one `DATABASE_URL`
cannot describe both. See the Commands section of `CLAUDE.md`.

---

## 2. Two processes, no GUI

The fastest way to poke at the protocol. `he-cli` exists because dropping HTTP
for raw QUIC cost us `curl`, and we refuse to debug a binary protocol with
print statements.

```bash
# once: create the space and its owner account
HE_PASSWORD='owner password' cargo run -p he-serverd -- -d /tmp/space --owner justin

# mint an invite — prints the code and the EndpointId, which are both needed to join
cargo run -p he-serverd -- -d /tmp/space --invite --max-uses 5

# serve it; ^C to stop
cargo run -p he-serverd -- -d /tmp/space
```

Then, in another terminal:

```bash
export HE_SERVER=<the endpoint id printed above>

# first contact: the invite creates the account, the password enrols this key
HE_PASSWORD='alice pw' cargo run -p he-cli -- \
    --key /tmp/alice.key --invite <CODE> --username alice info

# every time after that: no password, because the device key is the login
cargo run -p he-cli -- --key /tmp/alice.key send general "hit enter"
cargo run -p he-cli -- --key /tmp/alice.key history

# in a third terminal, watch events arrive live
cargo run -p he-cli -- --key /tmp/alice.key watch
```

A second `--key` path is a second machine as far as the server is concerned, so
that is how you test two members without two computers.

**Passwords come from `HE_PASSWORD` or stdin, never from an argument.** `ps`
shows arguments to every account on the machine and shells write them to a
history file.

**The address is a public key and nothing else.** There is no host, no port and
no `--addr` in normal use; discovery resolves the key. `--addr IP:PORT` exists
to skip discovery when testing on a LAN with the internet unplugged.

### Checking that nothing leaks

Plaintext in the database is a documented trust model; plaintext in a log file
that gets pasted into a bug report is an accident (PLAN §11). The logs are worth
grepping after any change to the auth or message path:

```bash
RUST_LOG=he_server=debug cargo run -p he-serverd -- -d /tmp/space 2>&1 | tee /tmp/serverd.log
# then, after sending some messages and failing some logins:
grep -iE 'password|secret|<some message you sent>' /tmp/serverd.log   # must find nothing
```

---

## 3. Two app windows

This is the M3 milestone demo: two windows hold a conversation, and closing and
reopening one shows history from disk with the network off.

### One window

```bash
cargo tauri dev
```

That starts Vite and the app together, and is what you want almost always.

### Two windows

Two things bite here, and both have the same shape — a default that is correct
for one instance and wrong for two.

**Share one dev server.** A debug build loads the frontend from
`http://localhost:1420`, and Vite runs with `--strictPort`, so a second
`cargo tauri dev` simply fails. Start Vite once and launch the binary twice:

```bash
cd web && npm run dev &
cd .. && cargo build -p hit-enter

export WEBKIT_DISABLE_DMABUF_RENDERER=1
HE_DATA_DIR=/tmp/alice ./target/debug/hit-enter &
HE_DATA_DIR=/tmp/bob   ./target/debug/hit-enter &
```

**`HE_DATA_DIR` is not optional.** The data directory holds the device key, and
the device key *is* the identity a server enrols (PLAN §3). Two windows sharing
one directory are one device with one enrolment, which is not the thing you are
trying to test.

`WEBKIT_DISABLE_DMABUF_RENDERER=1` is set by `.cargo/config.toml` for
`cargo run` and `cargo tauri dev`, but **not** when you execute
`target/debug/hit-enter` directly — so it has to be set by hand here. Without
it, WebKitGTK on Wayland with the proprietary NVIDIA driver kills the window at
startup with `Gdk-Message: Error 71 (Protocol error)`.

In each window: **+** in the server rail, then paste the EndpointId and the
invite code from step 2, and pick a username and password.

### What to actually check

- **Optimistic send.** A bubble appears grey with `SENDING` the instant you hit
  enter, and turns solid when the server echoes its nonce back. If it stays
  grey, the echo is not arriving.
- **Fan-out.** A message sent in one window appears in the other without a
  refresh. It should *not* be marked as yours there — the nonce belongs to the
  connection that sent it, not to the account.
- **Backfill.** A window that joins second pulls the existing history.
- **The connection dot.** Green is direct, amber is relayed, and relayed is
  *working* — slower, not broken (PLAN §6). Hover it for the explanation.
- **Local-first.** Kill the server and reopen a window: the whole conversation
  renders from disk and the dot settles to offline within ~20s.
- **The outbox.** Type something while the space is down. The bubble stays
  `SENDING` and the message is on disk — draining it on reconnect is M5.

### Reading the databases directly

Everything is an ordinary SQLite file, which is the point:

```bash
sqlite3 /tmp/bob/mirror.db 'select author_name, content from cached_messages order by id;'
sqlite3 /tmp/bob/mirror.db 'select content from outbox;'          # composed while offline
sqlite3 /tmp/space/server.db 'select username, is_owner from users;'
sqlite3 /tmp/space/server.db 'select endpoint_id, revoked_at from devices;'
```

Both files are `0600`. `server.db` holds `server_meta.secret_key`, which *is*
the server's identity — treat that file like an SSH host key, and never paste
its contents into a bug report.

### Starting over

The device key and the mirror are just files. Deleting a client's data
directory makes it a brand new machine, which needs a password to enrol again:

```bash
rm -rf /tmp/alice /tmp/bob          # new devices
rm -rf /tmp/space                   # new space: new EndpointId, every invite dead
```
