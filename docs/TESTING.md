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
| `he-proto` | 49 | framing, limits, invite links, network config, and that no wire type prints a password |
| `he-server` | 39 | registration, login, enrolment, invites, rate limiting — all by direct function call, no socket |
| `he-client` | 28 | the mirror, the device key, and **15 end-to-end tests over real iroh endpoints** |
| `hit-enter` | 5 | the settings file: defaults, an older version's file, a corrupt one |

> **`cargo test -p he-proto` on its own runs fewer than `--workspace` does.**
> Some tests live behind the two off-by-default features — `io` for the framing
> driver, `net` for the endpoint config (PLAN §7) — so testing that crate in
> isolation silently skips them. `--workspace` enables both through unification,
> because `he-server` and `he-client` ask for them. To run them deliberately:
> `cargo test -p he-proto --features io,net`. This is the shape of trap to watch
> for whenever a crate gains an optional feature — nothing fails, the tests just
> do not exist.

The end-to-end tests are the interesting ones:

```bash
cargo test -p he-client --test end_to_end -- --nocapture
```

They bind two real iroh endpoints on loopback with relays and discovery
switched off, run a real `he-server` against a real temp SQLite file, and dial
it over QUIC. No mocks — a mock of a QUIC handshake proves nothing (PLAN §14).
They need no network and take well under a second.

Four are worth knowing by name, because they are the M3 and M4 claims:
`the_mirror_answers_after_the_server_is_gone`,
`a_backfill_page_overlapping_live_events_does_not_duplicate`,
`a_pasted_invite_link_is_enough_to_join` and
`a_link_from_a_different_space_does_not_open_this_one`.

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

# mint an invite — prints the code, the EndpointId, and a join link carrying both
cargo run -p he-serverd -- -d /tmp/space --invite --max-uses 5

# serve it; ^C to stop
cargo run -p he-serverd -- -d /tmp/space
```

Then, in another terminal. A link is the whole address *and* the invite, so
`--server <link>` on its own is a first join:

```bash
export HE_SERVER='hitenter://join?t=…&c=…'      # the link printed above

# first contact: the invite creates the account, the password enrols this key
HE_PASSWORD='alice pw' cargo run -p he-cli -- \
    --key /tmp/alice.key --username alice info

# every time after that: no password, because the device key is the login
cargo run -p he-cli -- --key /tmp/alice.key send general "hit enter"
cargo run -p he-cli -- --key /tmp/alice.key history

# in a third terminal, watch events arrive live
cargo run -p he-cli -- --key /tmp/alice.key watch
```

`HE_SERVER` also takes a bare `EndpointId` or a bare `endpoint…` ticket, and
`--invite <CODE>` overrides whatever the link carries — which is how a stale
link gets reused with a fresh code.

### Testing without the internet

Both binaries take the same relay and discovery flags, because both sides of
the ladder in PLAN §4 have to be reachable from a terminal:

```bash
# local network only: no relay, no n0 DNS, mDNS for discovery
cargo run -p he-serverd -- -d /tmp/space --lan
cargo run -p he-cli -- --key /tmp/alice.key --lan info

# your own relay instead of n0's (repeat --relay for several)
cargo run -p he-serverd -- -d /tmp/space --relay https://relay.example.com
```

`--lan` is `--no-relay --no-dns`; `--no-mdns` turns off the local-network
announcement as well. The server prints which combination it ended up with on
startup, so a surprising one is visible rather than inferred.

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

The M3 demo is two windows holding a conversation, and closing and reopening one
showing history from disk with the network off. The M4 demo adds the half the
project exists for: one of those windows **hosts** the space, and the other joins
it with nothing but a pasted link.

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

**`HE_DATA_DIR` is not optional.** The data directory holds the device key, the
mirror and — if this instance hosts — the space; the device key *is* the
identity a server enrols (PLAN §3). Two windows sharing one directory are one
device with one enrolment, which is not the thing you are trying to test.

It is also what keeps the two windows apart at the process level. A clicked
`hitenter://` link starts a second copy of the app, so the app holds a
single-instance lock — but scoped to the **data directory**, not the machine,
because two data directories are two devices. Without `HE_DATA_DIR` the second
launch hands its arguments to the first window and exits, which is correct for a
released app and fatal to this recipe.

`WEBKIT_DISABLE_DMABUF_RENDERER=1` is set by `.cargo/config.toml` for
`cargo run` and `cargo tauri dev`, but **not** when you execute
`target/debug/hit-enter` directly — so it has to be set by hand here. Without
it, WebKitGTK on Wayland with the proprietary NVIDIA driver kills the window at
startup with `Gdk-Message: Error 71 (Protocol error)`.

In each window: **+** in the server rail, then paste the EndpointId and the
invite code from step 2, and pick a username and password.

### Hosting from the app — the M4 demo

In the first window: **host a space** on the empty state, or the house icon at
the bottom of the server rail. Give it a name, a username and a password.

What happens next is worth understanding, because it is the whole architecture
in one click (PLAN §2.1):

1. `server.db` is created in that window's data directory, which generates the
   **space's** identity — a different key from the device key already there.
2. The owner account is made **locally**. It is the one account an invite
   cannot gate, so it never crosses the network.
3. That same window's client then *dials the space by its `EndpointId`* with the
   owner's password, exactly as a stranger's client would. Watch the log: a
   `hosting` line, then `password login; device enrolled`, then
   `session established`. There is no host shortcut and there must never be one.

Then **invite** in the channel list footer, **copy link**, and paste it into the
second window's join dialog. The dialog parses it as you type: it shows the
space's address, fills in the invite code, and says so if you are already a
member or if the link is your own space.

A few things to check that only exist in M4:

- **The space's address is not your device's.** The host panel says so; the
  settings pane shows the device key separately. Handing out the wrong one is
  the mistake this wording exists to prevent.
- **Close the space, then reopen it.** Members drop cleanly rather than timing
  out, and reopening reconnects the host's own client — if your own space sits
  at `offline` in your own window, that is the bug.
- **`settings.json`** next to the mirror. Editing it by hand is supported; a
  corrupt one must start the app with the defaults, not refuse to open.
- **Relay and discovery settings.** Changing them rebinds the space on the spot
  and says plainly that your own connections keep the old settings until you
  restart — because the client endpoint is bound at startup and holds every open
  session.
- **A link with no relay in it.** Mint one immediately after opening a space,
  before it has reached a relay, and the invite dialog warns that it will only
  work on your local network. That is the difference between "a friend in
  another city joins" and "it worked on my machine".

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
cat /tmp/alice/settings.json                                      # relays, discovery, hosting
```

A space hosted from the app puts its `server.db` in that instance's data
directory rather than in a `/tmp/space` of its own, so for the M4 demo the last
two queries read `/tmp/alice/server.db`.

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
