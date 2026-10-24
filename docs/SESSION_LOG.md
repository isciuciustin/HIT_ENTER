# Session log

What each session did, and **what was already argued about** — check here
before reopening a settled question. Newest last.

---

## 2026-09-27 — the idea becomes a plan, and M0 starts

### Where it started

The owner's idea since March 2026, first published as **"bando"**
(justin-hakr-flexer.codeberg.page): a Discord replacement with no
"closed cabinets", no censorship, and creators rewarded — Tauri, Svelte,
Rust, **Nostr for identity and authenticity, iroh for files and voice**.
After HIT_ENTER worked between two cities, the question was whether
"**every person is a relay**" could replace Nostr's public relays.

### The answer, in one line

**Yes, because a Nostr event is self-authenticating**: untrusted home
machines can carry each other's messages, since nobody has to trust the
messenger — only check the signature. iroh supplies what Nostr lacks: a
connection to a machine with no public address. Four things are not free,
and `DESIGN.md` §1 names them: a sleeping relay (→ anchors), authority with
no server (→ §6), iroh's own relays and discovery (n0's by default), and
money, which is never peer-to-peer (→ §11).

### Decisions — settled, do not reopen without a reason

| decision | why | where |
|---|---|---|
| **The app is called HIT_ENTER**, not bando | the owner's call | — |
| **It lives in `HIT_ENTER_0.1`**, a **private** repo | the owner's call; `HIT_ENTER_1.0` is a **public** repo and must receive nothing from here | `CLAUDE.md` |
| **Closed source, all rights reserved** | the owner: *"privacy is a human right … this project is going to follow my vision"*. Reversed an earlier plan for MIT/Apache | `DESIGN.md` §15 q1 |
| HIT_ENTER_2's code **may** move here | same owner, same licence — nothing is relicensed. Record each piece that moves, here | `PLAN.md` §1 |
| The original HIT_ENTER's code may move here, with its notice | MIT/Apache permits closed use | `PLAN.md` §1 |
| **A 1% fee on every zap sent from the app** | the owner's call | `DESIGN.md` §11 |
| …taken as **a second payment on top, from the sender's wallet** | the recipient gets exactly what was sent; HIT_ENTER is never in the zap's path — the line between a service fee and a money transmitter | `DESIGN.md` §11 |
| …shown before sending; none under 100 sats; zap first, fee best-effort | Lightning has no fractional sats; a recipient must never lose a zap to our fee | `DESIGN.md` §11 |
| …**a plain payment naming nothing** | a zap receipt would publish forever which message every fee was for | `DESIGN.md` §11 |
| **HIT_ENTER never holds user funds** | a custodial wallet is money transmission | `DESIGN.md` §12 |
| DMs are in v1, as **NIP-17** | a DM to an offline person is stored by someone else's machine; NIP-17 is a real, standard E2EE design | `DESIGN.md` §7 |
| Group authority: **signed admin actions + rank rule + a late group-key sequencer** | one authority peer brings back the single point of failure; full state resolution is Matrix's years of bugs | `DESIGN.md` §6 |
| Voice v1 is **a mesh, capped at 8** | Opus arithmetic: 7 × 32 kbit/s up at 8 people is fine, 25 is not | `DESIGN.md` §10 |

### The honest things said along the way

- **"Could I become a billionaire with open source?"** It has happened
  (Red Hat, MongoDB, Automattic), always by selling convenience around free
  code, never the code. The odds are tiny for anyone; chat is
  winner-take-most; decentralisation limits what can be captured by
  design. Treat it as an outcome, not a plan: 10 groups → 1,000 people who
  would miss it → 100 paying for anchors.
- **The 1% fee's arithmetic:** 1,000 users zapping 5,000 sats a month is
  about $50 a month. Hosted anchors pay the bills first; the fee grows with
  the network.
- **Closed source and privacy pull against each other**: users cannot
  inspect a promise. Written into `DESIGN.md` §15 q1 once, with three
  mitigations — publish `PROTOCOL.md`, use standards for anything secret,
  and pay for an independent review before 1.0.

### M0 — spike S3, part 1

- Rust 1.98.1 installed via `mise` — the machine had none.
- n0's `callme` built with echo cancellation in 1m 15s, and two local
  copies connected in **0.44 s** with Opus both ways, through a silent
  PipeWire sink so the owner's mic was never recorded.
- **Real-time audio priority took three fixes, and one is still open:**
  `rtkit` installed (by the owner — `sudo` has no TTY here, not even via
  `!`); PipeWire restarted so it picks rtkit up (a logout had not restarted
  it); and **terminal-launched processes still inherit `RLIMIT_RTTIME` 0/0**,
  which cannot be raised. The call works without it.

### M0 — spike S3, part 2, first attempt (2026-10-04)

- `callme-cli` built on the friend's machine, a **macOS** (Apple silicon)
  MacBook Air, in `~/Work/tries/callme` — outside this repo. Two traps,
  written into `spikes/voice/README.md`: Homebrew autotools are needed
  (`glibtoolize`), and a failed `webrtc-audio-processing-sys` build has to
  be `cargo clean`ed before a retry.
- First call, Mac ↔ Linux on **the same LAN**: direct connection, but
  **robotic sound with gaps**. The Mac log blames audio, not network: a
  Bluetooth headset mic in HFP (24 kHz mono), 698 dropped capture chunks in
  the first two minutes, and a steady once-a-second playback underrun that
  looks like clock drift. **Not a verdict** — the Linux `accept.log` has not
  been read, and it was not a two-city call.
- `gh` is not logged in on the Mac. Privacy was checked anonymously instead:
  `curl https://api.github.com/repos/isciuciustin/HIT_ENTER_0.1` → **404**,
  i.e. not public.

### M0 — spike S3, part 2, second attempt set up (2026-10-10)

- Second Linux machine of the owner's (gaming PC, Redragon Zeus Lite USB
  headset) joins the laptop (USB gaming headset) for a **Linux ↔ Linux** call,
  removing the Mac's Bluetooth/HFP suspect.
- The laptop had no Rust or callme: installed `rust@stable` (1.99.0) via
  mise and built callme (`afd586f`) in `~/Work/tries/callme` in 1m 22s.
  A 12 s `accept` smoke test opened PipeWire's `pulse` device at 48 kHz and
  printed a node id; `rtkit` is still not installed on the laptop (warning
  only).
- Added `spikes/voice/call.sh` (`setup` / `devices` / `accept` / `connect`),
  and a README section with the recipe and a one-change-at-a-time ladder.
  The call itself has **not** happened yet.

**Result of the second attempt (2026-10-10):** laptop ↔ gaming PC, same
LAN, USB headsets, ~2 min — **clear, no robotic sound, low latency**. Laptop
log: direct, 0 capture xruns, 7 playback xruns. Provisional verdict written;
speakers, busy machine and cross-network are still untested.

**Speaker test (2026-10-10):** laptop on internal speakers + internal mic, PC
on headset, ~7 min, direct. PC's voice clear on the speakers; the laptop mic
path was **robotic with gaps** (~20 capture and ~20 playback xruns a minute,
vs ~3.5 on the USB-headset call). Not a clean echo test (one person, one
room). **Open:** rerun with `--disable-processing`, ideally with two people,
to split mic / canceller / CPU.

**Speaker test round 2, `--disable-processing` (2026-10-10):** xruns dropped
~a third (capture ~12/min, playback ~14/min) and the laptop mic sounded a
little better, but still robotic with gaps. So AEC is a minor cost, not the
cause; the laptop's internal analog path is the suspect (the USB headset on
the same machine had ~3.5/min). Next: `rtkit`, then named ALSA devices.

**Speaker test round 3, `rtkit` installed (2026-10-10):** `rtkit` +
PipeWire restart + callme started via `systemd-run --user`. callme *still*
got no real-time threads (`setrlimit` EPERM, no request reaches
`rtkit-daemon`; the message is `audio_thread_priority`'s cleanup path
hiding the first error). Xruns ~13/min capture and playback with AEC on —
about round 2's level. So `rtkit` was not shown to matter. **Note:**
`pkill -f <pattern>` from the Bash tool matches the tool's own shell and
kills it; use `systemctl --user stop callme-accept`.

### Mistakes, so they are not repeated

- A folder called `BANDO` was created before the name was settled, then
  removed. **Ask where a new project goes before creating it.**
- The docs were first written into `HIT_ENTER_1.0` — the **public** repo —
  then moved to `HIT_ENTER_0.1`, and `HIT_ENTER_1.0` restored to its pushed
  state. Nothing private was ever committed or pushed there. **Check a
  repo's visibility before writing anything private into it.**
- `! sudo …` was suggested for `rtkit`; it cannot work, because `!` gives
  sudo no terminal to ask for the password on.

### M0 — S3 parked (2026-10-10)

- The cross-network call could not be run: the owner's village connection is
  too slow, and the iPhone Personal Hotspot would not turn on (cellular
  settings / plan not checked further). **Decision: park S3 with a
  provisional verdict** rather than block the app on it. Written into the
  README, `PLAN.md` M0 progress and `CLAUDE.md`.
- The round-3 time-out is most likely the `systemctl --user stop` that ended
  that run, not a network fault. Unconfirmed.
- **Do not repeat:** more speaker rounds on the laptop's internal analog
  audio; `pkill -f` from the tool shell (kills it).

### M0 — S1 started: Nostr over iroh (2026-10-10)

- Crate `spikes/nostr-iroh` (own `[workspace]`, `publish = false`):
  `iroh` 1.3.0 (the docs said 1.1), `nostr` 0.45.5, `negentropy` 0.5.1,
  `sqlx` + SQLite with runtime queries. Framing rewritten (~40 lines);
  `HIT_ENTER`'s `he-proto` is not on this laptop.
- **One-machine result:** NIP-01 `EVENT`/`REQ`/`EOSE`/`OK` and NIP-77 work
  over `hit/nostr/1`; a forged event is refused before storing; a 30-event gap
  over a 5,036-event history syncs in 2 rounds / 18 KB / 0.5 s, identical sets
  in 1 round / 751 B. Store choice holds. Details and caveats in the spike
  README.
- `nostr` 0.45 API traps, for the next crate: no `JsonUtil` (use `serde_json`
  on `Event`); `Keys::generate()` needs feature `os-rng`; sign with
  `EventBuilder::new(Kind::TextNote, ..).finalize(&keys)` after
  `use nostr::event::FinalizeEvent`; types live under `nostr::prelude`.
- `/usr/bin/time` is not installed; a command wrapped in it fails silently
  if its output is grepped.
- **Still open for S1:** the laptop ↔ gaming PC run, pipelined bulk push, and
  SQL-side filtering.

### Next session

**S2 (gated gossip) and S5 (two endpoints in one process).** The S1 laptop ↔
PC run (recipe in `spikes/nostr-iroh/README.md`) can ride along. S4 when the owner has two wallets and an NWC string. S3's remaining
tests (two-person echo, busy machine, cross-network) whenever the conditions
exist.

### M0 — S1, second independent build (2026-10-10)

- Built `spikes/nostr-iroh-b/` (~780 lines, not a workspace member, iroh
  `=1.1.0`): `hit/nostr/1` framing from `he-proto`, a relay session loop
  speaking the `nostr` crate's NIP-01 + NIP-77 messages, negentropy sync
  both ways, and SQLite behind `sqlx`. No code carried over from
  HIT_ENTER or HIT_ENTER_2 beyond the framing *shape* (re-written).
- **Verdict: yes.** In one process and as two OS processes with the relay
  killed and restarted. 2000 events reconcile in 2.3 KB (~320× less than
  sending them); forged events are refused; a 4 GiB frame length is refused
  before allocating; the peer id is read off the connection.
- **Decided:** own SQLite via `sqlx` is the store for M2. Not a comparison
  with `nostr-lmdb` — that was not built. **Settled unless scale says
  otherwise:** negentropy storage is rebuilt per `NEG-OPEN`; measure before
  ~100 k events per group.
- **Not covered** (so do not cite S1 for them): any real network (no relay,
  no hole punching), authorization, third-party interop (existing Nostr
  tools speak WebSocket; a dev-only bridge is an open question for
  `DESIGN.md` §4's "tested with existing tooling"), replaceable/deletion
  semantics, scale.
- This ran in parallel with the build above and was found only when the push
  was rejected; the owner chose to **keep both**, mine moved to
  `spikes/nostr-iroh-b/`. They agree on the verdict. Complementary: the
  first has bulk (5 k) timings and a persistent key; this one has SQL-side
  filtering, forged/oversized/peer-id assertions and a restart-the-relay
  script. **Lesson:** `git fetch` and read `SESSION_LOG.md` before starting a
  spike when two sessions can be live.
- Next: S2 and S5; S4 when the owner has wallets.

### M0 — S2 done (2026-10-10)

- Built `spikes/gossip-gate/`: a `ProtocolHandler` that gates
  `Gossip::handle_connection` on `conn.remote_id()`. **Verdict: yes, a topic
  can be gated — at the ALPN.** A shared ALPN lets any allowed peer join any
  topic by id (shown), so gating a private channel needs its own gossip
  instance.
- `DESIGN.md` §4 rewritten and §15 q2 closed. It had said "one gossip topic
  per group" with a gated-topic fallback; the real shape is one *instance*
  per set of readers.
- Findings: list-only revocation leaves the open connection delivering;
  a peer refused once does not self-recover (a member dialling it, or a
  restart, does — cause not isolated); both ends must gate;
  `iroh-gossip 0.101` needs iroh 1.3, HIT_ENTER_2 is on 1.1.
- **My mistakes in the run, so they are not repeated:** the first T2
  "failure" was my empty allow-list on the newcomer's own gate; a missing
  `match` arm made `wait-60` report a false success. Both were caught by
  asking why identical code gave different answers.
- Not covered: real networks, member-introduced strangers, cost at scale,
  churn.
- Next: S5; S4 when the owner has wallets.


### M0 — S5 done (2026-10-10)

- Built `spikes/two-endpoints/` (iroh `=1.3.0`, own lockfile; the test relay
  runs in a child process so its memory stays out of the numbers).
  **Verdict: yes, two endpoints in one process work and are cheap, but only
  with separate keys.** 17 loopback checks plus 1 on the real network.
- **T1:** a relay-only endpoint advertises 0 IPs, and a stranger holds no IP
  path to it whichever side dials. The direct endpoint beside it leaks as
  usual, which shows the harness can see a leak.
- **T2, one key on two endpoints:** 10 of 10 dials reached the *newest*
  endpoint, and the older came back only when the newer closed. HIT_ENTER_2
  had read `clients.rs` as "taking turns". In fact it is **last
  registration wins, and the loser goes dark with no error.** → two device
  keys, two bindings.
- **T3, cost:** after the first endpoint's one-time ~11–16 MB, a relay-only
  endpoint costs ~0.1–0.5 MB, 2 sockets and 0 threads; a direct one ~1–2 MB
  and 4 sockets. Idle CPU is negligible on loopback.
- **T4:** close or rebind either endpoint and the other carries on.
- **T5, real network:** HIT_ENTER_2's loopback test could not rule out the
  port mapper or QAD putting a public IP into a relay-only endpoint's address
  (`procure_mapping()` runs regardless). By reading the code, neither can,
  because neither has a socket. On the real network the relay-only endpoint
  showed 0 IPs and the direct one 3 (2 public). Only counts were printed,
  never addresses.
- **Decided, for the design:** each group lives on exactly one endpoint, with
  all of its ALPNs. `DESIGN.md` §7 and §15 q5 are updated, and
  §15 q5 recommends going ahead with two endpoints; M4 still makes the call,
  now on complexity alone.
- **Not covered:** a peer on another machine across NATs (repeat T1 on the
  laptop ↔ PC run), the relay linking both ids, real-network idle cost,
  macOS/Windows/Tauri, gossip and blobs on both at once.
- Next: S4 when the owner has wallets and an NWC string; the laptop ↔ PC run
  for S1 + S2 + S5's T1. After that, M0's leftovers (workspace skeleton,
  `about.toml`, `THIRD_PARTY_NOTICES.md`) and M1.

### M0 — workspace skeleton (2026-10-10)

- `Cargo.toml` workspace (resolver 3, edition 2024, `rust-version` 1.90), six
  crates: `hit-event`, `hit-proto`, `hit-store`, `hit-group`, `hit-node`,
  `hit-cli`. Path dependencies fix the layering: cli → node →
  proto/store/group → event. Every crate is `publish = false` with no
  `license` key. `spikes/` is excluded. **Not created yet:** `hit-anchor`,
  `hit-files`, `hit-voice`, `hit-zap`, `src-tauri`. Each joins when its
  milestone starts.
- `[workspace.dependencies]` pins what the spikes settled: `iroh =1.3.0`,
  `iroh-gossip =0.101.0`, `nostr 0.45`, `negentropy 0.5`, `sqlx 0.9` (with
  `macros`). No member uses them yet.
- The no-`unwrap()`/`expect()` rule is now a lint: `clippy::unwrap_used` and
  `expect_used` are denied workspace-wide, and `clippy.toml` allows both in
  tests. A `main()` that needs one writes a local `#[allow]`.
- `about.toml`/`about.hbs` were adapted from HIT_ENTER_2. `about.hbs` has a
  section waiting for HIT_ENTER's notice when its code first moves in.
- **Decided: MPL-2.0 is accepted** (owner, 2026-10-10). Run against the
  spikes' trees (~330–430 crates each), the licence check found exactly one
  non-permissive crate: `attohttpc`, via iroh's **default `portmapper`
  feature** (portmapper → igd-next → attohttpc). HIT_ENTER_2's `about.toml`
  attributes it to Tauri/WebKit; under iroh 1.x it comes from the port
  mapper. The alternative was turning the port mapper off, which means more
  relayed connections. Nothing GPL-family anywhere in the planned stack.
- Trap: `cargo install cargo-about --locked` installs **nothing** from 0.9
  on and exits 0 with only a warning; it needs `--features cli`.
- Next: M1. S4 when the owner has wallets.

### M1 — done (2026-10-10)

- **`hit-event`** has three modules. `person` holds the key; `store` is where
  the nsec lives; `binding` is *"npub P speaks from device D"*.
  - `Person` holds `nostr::Keys`. It has no `Debug` of the secret, and the
    secret leaves by exactly two named paths: `to_ncryptsec` (recommended
    backup, scrypt 2^16) and `reveal_nsec_for_backup` (onboarding,
    `Zeroizing`). Import errors never echo their input.
  - `SecretStore` has two implementations. `Keychain` uses **keyring 4.2**,
    default `v1` mode: macOS Keychain, Windows Credential Manager, and on
    Linux the Secret Service over pure-Rust zbus, no libdbus. `MemoryStore`
    is for tests. **There is deliberately no file store.**
  - The binding: `sign_binding`, `sign_revocation`, `needs_renewal`,
    `verify_binding(event, endpoint_id)` and `verify_binding_at`, plus
    `BindingBook` (`insert`, `authenticate`, `devices_of`). A node must use
    `authenticate`, because `verify_binding` on one event cannot see a
    revocation.
- **Decided in M1** (written into `DESIGN.md` §3 and `PROTOCOL.md`):
  - **Kind 30436**, provisional (§15 q4 stays open).
  - **The device countersigns the binding.** The `proof` tag is an ed25519
    signature over `hit-enter/device-binding/1:` ‖ the npub. Without it,
    anyone could publish "my npub is at your device" as a discovery record.
    Revocations carry no proof, because a lost device can't sign.
  - **`d` must be canonical lowercase hex, compared as a string.** iroh's
    parser also accepts base32, and two spellings would mean two
    replaceable slots, letting a binding survive its revocation.
  - **A revocation beats a binding on a same-second tie**; otherwise
    NIP-01's lower-id rule applies.
  - Expiry: sign 7 days, refuse more than 30, re-sign at half-life; up to
    10 minutes of clock skew is allowed.
- **Done-when tests** (`tests/m1_done_when.rs`) pass, along with 16 unit tests:
  - two devices bound; each verifies against its own device and is refused
    against the other; one is revoked and refused after;
  - a no-leak scan over everything a node writes to disk or a log, with a
    planted control that the scan must catch.
  The `#[ignore]`d real gnome-keyring round trip also passes. Clippy is
  clean with and without the `keychain` feature. The licence check passes
  with keyring, zbus, scrypt and ed25519 in the tree.
- **Traps:**
  - `nostr` 0.45.5 does not compile `nip49` without `nip46`/`nip59`
    (`Error::crypto` is cfg'd on those). `nip59` is on; M7 needs it anyway.
  - `TagKind` is not in the prelude, and `Tag::custom` takes a `&str`.
  - `EventBuilder::text_note` is gone; use `EventBuilder::new(Kind::TextNote, ..)`.
  - keyring's `Debug` for `BadEncoding`/`BadDataFormat` prints the raw secret
    bytes; its `Display` doesn't. `store.rs` converts through `Display` in one
    function, with a comment saying why.
  - clippy's `allow-unwrap-in-tests` covers only `#[test]` functions, not
    helpers in `tests/*.rs`; that file has a file-level allow.
- **Not checked, for the owner:** gnome-keyring writes a keyring that has an
  **empty password as plain text**. The agent was (rightly) blocked from
  reading `~/.local/share/keyrings/`, so whether the login keyring here is
  encrypted is unknown. The owner can check with
  `head -c 16 ~/.local/share/keyrings/Default_keyring.keyring | od -c`:
  `GnomeKeyring` is encrypted, `[keyring]` is plain text. Onboarding should
  warn about this on Linux; that's an M4 question.
- Next: M2.

### M2 — done on one machine (2026-10-10)

- **`hit-proto`.** ALPN `hit/nostr/1` and the limits. The framing and its
  async driver are **ported from HIT_ENTER's `he-proto`** (`frame.rs`,
  `io.rs`, HIT_ENTER commit `6471762`). That's the first code to move in
  from HIT_ENTER; its MIT notice is in `about.hbs` → `THIRD_PARTY_NOTICES.md`.
  The `HIT-BINDING` hello frame is new.
- **`hit-event` additions.** `Verified` (signature-checked; the store accepts
  nothing else) and `replace` (NIP-01 slots plus `replaces`, shared by the
  store and `BindingBook` so they can't disagree about a revocation).
- **`hit-group`.** The M2 `Roster` (`may_connect`, `may_read`, `may_store`,
  `CARRIED_KINDS` = 1 + 30436). M2 has no groups, but the questions are
  already asked here, so M3 replaces the state behind them, not the call
  sites.
- **`hit-store`.** Migration `0001_events.sql` (STRICT tables; a partial
  unique index for replaceable slots). Fixed queries use `sqlx::query!`
  with a `.sqlx` cache and `SQLX_OFFLINE`; filters use `QueryBuilder` with
  bound values (a test feeds it a SQL-injection-shaped tag value).
- **`hit-node`.**
  - A `Router` with one `ProtocolHandler` (the relay) and `sync` (the
    dialler), sharing **one gate**, `Inner::accept_event`: signature →
    `hit-group` → `BindingBook` for bindings → store → subscribers.
  - Bindings are exchanged both ways at session start. A refusal is a
    `NOTICE`, then finish and wait for `stopped()` (HIT_ENTER_2's
    `send_final` lesson), then close 403.
  - Device key file 0600. Our binding is renewed at half-life. **A node
    whose device the person revoked refuses to start** instead of
    re-binding itself.
- **`hit-cli`.** `init/id/binding/allow/peer/post/log/sync/run`. The nsec
  goes in the keychain; `node.json` holds only public facts. `post` writes
  to the store without binding an endpoint, so it can run beside `run` (a
  second endpoint with the same key would trip S5's "newest wins").
- **Done-when results:**
  - The automated test passes (and was stable 3/3).
  - `scripts/m2-by-hand.sh`, three processes on the desktop, A `kill -9`ed:
    A fetched exactly **20, 0 duplicates, 1 round, 7–8 ms**. C's copied
    binding and C's unlisted identity were both refused.
  - **Not run between two machines.** `docs/TESTING.md` §2 has the recipe.
- **Mistakes:**
  - My own test assertion was wrong: a newly admitted node fetches the
    other members' *bindings* as well as their notes (27, not 26).
  - A `RwLockReadGuard` in an `if let` scrutinee lives through the block
    and across the `.await` inside it, so the handler future wasn't `Send`.
    Bind the result first.
  - I wrote "`IN ()` is a syntax error"; SQLite accepts it. Removed before
    commit.
- **Keychain hygiene:** test identities go under `hit-enter-m2-test` and are
  removed with `secret-tool clear service hit-enter-m2-test` (keyring 4
  files them with a `service` attribute). The script does it on exit.
- 41 tests, clippy `-D warnings` clean. The licence check passes with about
  500 crates, one MPL (`attohttpc`) and nothing GPL-family.
- Next: M3. The two-machine M2 run when there is a second machine.

### M3 — done on one machine (2026-10-11)

- **Decisions** (each in `PROTOCOL.md` and the code's module docs):
  - **The roster is gone.** `hit-group::Groups` (the groups a node carries)
    answers every question the M2 `Roster` did, per event: may it be
    stored (`judge`), may this person read it (`may_read`), which gossip
    carries it (`gossip_group`). Kind 1 is no longer carried.
  - **Authenticating is not authorizing.** A session is no longer refused
    for who the peer is; every `REQ`, negentropy set and `EVENT` is judged
    per event. A stranger is served nothing and can hand over a join —
    that is how joining works with nobody in charge.
  - **Group state = four group-key-signed events with `seq`** (NIP-29's
    `39000`–`39003`, our tags). The store orders them by `seq` first
    (`hit_event::replaces`), like the fold.
  - **The owner = named by the group key *and* signer of a `9007`**, the
    device binding's two-signature idea applied to ownership.
  - **The fold's cutoff**: actions dated at or after the snapshot apply;
    a compaction is dated one second after what it folds. Within one second,
    apply until nothing more applies; across seconds, strict order.
    Honest limit, in `DESIGN.md` §6: an action the compactor never saw,
    dated before it, is lost (visibly).
  - **An action is stored only if folding it with everything held applies
    it.** Invites are credentials, judged by "may the author invite", and
    checked again when a join uses them.
  - **Gossip: one instance per group, ALPN `hit/gossip/1/<G>`, gated in an
    endpoint hook** (`after_handshake`) for both directions. iroh's `Router`
    fixes its ALPNs at spawn, so `hit-node` has its own accept loop
    (`accept.rs`) and calls `set_alpns` as groups come and go.
  - Private channels' messages travel **by sync only** until they get their
    own gossip instance; the gossip allow-list is every member.
  - CLI commands that only sign work on the store (`publish_offline`, the
    same judgement as a running node), so they can run beside `run`.
- **Moved from HIT_ENTER_2** (the owner's code, same licence):
  `he2-proto/src/perm.rs` → `hit-group/src/perm.rs` (bits unchanged;
  `MANAGE_SPACE` renamed `MANAGE_GROUP`, same bit; serde dropped), and the
  resolution and escalation rules of `he2-space/src/roles.rs`, rewritten as
  pure functions over `GroupState` (`base`, `effective`, `rank`,
  `outranks`, grant check). The refusal tests that apply to M3 (kick,
  roles, invites, view/post, the owner) are ported in
  `hit-group/tests/fold.rs`; mute, ban, slowmode, moderator deletes and role
  editing wait for M5 with their tests.
- **Mistakes, mine, all caught by tests or the by-hand run:**
  - Invites were judged by "did the fold apply it" — an invite is never
    applied, so every invite was refused.
  - The cutoff was "strictly after the snapshot", which loses every action
    in the same second as a group's creation — the first join, in practice.
  - The fold broke same-second ties by event id alone, so a join could be
    refused because its inviter's own join sorted after it. Same bug again
    one layer up: a sync accepted a fetched batch one event at a time.
  - **Revocation only closed incoming gossip connections**; gossip also
    dials out, and a kicked member kept hearing the group. The kick test
    caught it; the gate moved into the endpoint hook.
  - Making our own binding readable by anyone fixed one duplicate and
    caused refusal noise with every stranger, and then a real bug: a joiner
    never stored the binding of the member it joined through, so its gossip
    gate refused that member. Now negentropy leaves out the two parties'
    bindings and each side re-offers the other's hello binding at the
    session's end.
  - The sync loop synced with its targets **one after another**, so a dial
    to a device that was down held everyone else up, and backoff kept a
    returning peer waiting. Each target now syncs in its own task, and a
    device that completes a hello with us has its backoff reset.
  - A by-hand script matched `/group/` in awk and got two lines.
- **Results:**
  - 99 tests (`hit-group` 49, incl. 36 adversarial), clippy `-D warnings`
    clean, stable 3/3 over the workspace and 5/5 for the node tests.
  - `m3_done_when.rs`: three peers, live gossip with no sync, the owner
    shut down, the two keep talking, a fourth joins from a member's invite,
    the owner returns to everything; a stranger gets nothing by `REQ`,
    negentropy or gossip. A second test kicks a member: it stops hearing
    the group.
  - `scripts/m3-by-hand.sh`, five processes, the owner `kill -9`ed: every
    expectation held, compaction to seq 1 lost nothing. `m2-by-hand.sh`,
    now in a group: fetched exactly 20, 0 duplicates.
  - The licence check passes with `iroh-gossip` 0.101 and its tree.
- **Not done / open:** the two-machine run (M2's and M3's). NIP-29
  interop needs translation, not pass-through. Messages from a member
  removed just after sending are judged on arrival, so peers can disagree
  (M5). A negentropy set is built by parsing every carried event and
  asking `may_read` — one rule in one place, at a cost per sync that will
  want an index past a few thousand events.
- Next: M4 (the app), or the two-machine run first if a second machine
  turns up.

### M4a — the app, on one machine (2026-10-11)

- **M4 is split** (its size line says to). M4a is an app two people can
  talk in; M4b adds reactions, presence proper, a paged member list, S5's
  second endpoint and the frontend's licence notices. The v0.1.0 tag waits
  for M4's real done-when (`TESTING.md` §4).
- **Decisions:**
  - **The shell is HIT_ENTER_2's; the store is new.** `src-tauri` (crate
    `hit-app`, binary `hit-enter`, identifier `app.hitenter.desktop`, URL
    scheme `hit-enter:`): single instance per data directory, deep links
    that open the join dialog and never join by themselves, the dmabuf
    workaround, the clipboard plugin. HIT_ENTER_2's `chat.svelte.ts` (1,700
    lines) was shaped around one host per space; `web/src/lib/chat.svelte.ts`
    is a new one of ~250 over a node that already is the mirror.
  - **The logic lives in `state.rs`, without Tauri**, so it is tested
    without a window; `commands.rs` is one line per command.
  - **Onboarding (`DESIGN.md` §3):** a new key goes into the keychain at
    once — a crash must not lose it — but the node does not start until the
    backup step is done, so no group can be made or joined before. Two ways:
    an `ncryptsec` file (password ≥ 8), or the key shown once and its last 8
    characters typed back. `app.json` (npub, backed-up flag; never a
    secret) makes a quit half way resume at the backup step. `reveal_key`
    refuses at any other step, so "never displayed after onboarding" is
    enforced in Rust, not by the window. An import is already backed up.
    If `app.json` names an identity the keychain no longer holds, the app
    says so and asks for the backup.
  - **Profiles are carried** (kind 0, NIP-01 as-is), by the binding rule:
    stored and served only between people who share a group, never on
    gossip. Writing a name keeps every other field, so an imported Nostr
    identity keeps its `picture`, `about` and `lud16`.
  - **"Reachable", not "online":** a person is shown as reached when one of
    their devices completed a session with us, either way, in the last 20
    s (`Node::reachable`). A device we have not tried is unknown, not away;
    presence proper is M4b.
  - **Offline needs no outbox:** a message is stored here first, shown at
    once, and the sync loop (every 3 s) delivers it to whoever is reached
    next. The header says *nobody reachable — kept here until someone is*.
  - **The IP badge stays** ("your IP is visible to this group") until S5's
    second endpoint makes it untrue; the profile dialog says the same in a
    sentence.
  - `HIT_DATA_DIR`, `HIT_KEYCHAIN_SERVICE`, `HIT_RELAYS=none`, `HIT_PORT`
    and `HIT_PEERS` exist for running two windows on one machine; the app
    as shipped needs none of them.
  - `hit-node`'s `create_group`/`compact` take `&(dyn SecretStore + Sync)`:
    a Tauri command's future must be `Send`.
- **Moved from HIT_ENTER_2** (the owner's, same licence): `src-tauri/src/lib.rs`'s
  shell (single instance, `instance_id`/`fnv1a`, deep links, the dmabuf
  workaround), the icons, the `.desktop` template, `web/`'s Vite, Svelte,
  TypeScript and Tailwind setup, `app.css`, `focus.ts`, `Dialog.svelte`,
  and the look of the rail, channel list, message pane and composer.
- **Mistakes, caught by tests or by driving the window:**
  - A test renamed someone twice in one second and kept the *old* name:
    replaceable events of the same second are settled by NIP-01's lowest
    id. `set_name` now dates a profile one second after the one it
    replaces.
  - The key-confirmation field was not focused after *show my key* removed
    the button that had focus — WebKitGTK then sends the page no keys.
  - The member list showed a joiner's key after their name had arrived (it
    read the member row, not the names map).
  - With relays off, two windows could not find each other after a restart:
    an invite's `@ip:port` lived only in memory. Relays on, discovery does
    it; for tests, `HIT_PEERS`.
- **Results:** 106 tests (+7: `hit-app` 3, `hit-event::profile` 2, a
  profile fold test, `hit-node/tests/profile.rs`), stable twice; clippy
  `-D warnings` clean over the workspace including the app; `npm run check`
  0 errors, 0 warnings; the licence check passes with Tauri (six MPL-2.0
  crates now, the same five under Tauri that HIT_ENTER_2 ships, plus
  `attohttpc`). Two windows driven from the keyboard with `wtype`, each
  keystroke behind a focus check: onboarding by the key shown once, a name,
  a group, a message, an invite copied and pasted into the second window,
  a join with history and names, a live message both ways, then A
  `kill -9`ed, A reopened alone with history from disk and a message
  written to nobody, B started — `fetched 2`, delivered by itself. The test
  keychain service was cleared after. (The run also replaced the
  clipboard's content with an invite link.)
- **Not done / open:** the *encrypted file* backup and the import screen
  through the GUI (the GTK save dialog ignored `wtype`'s Enter, Tab and
  Escape; both are unit-tested); everything in M4b; the real done-when with
  a person in another city (`TESTING.md` §4); `cargo tauri` is not
  installed — `cargo build --features custom-protocol` is the same without
  it. Same-second messages from two people can show in either order
  (`created_at` is whole seconds).
- Next: M4b, or `TESTING.md` §4 if a friend in another city has an
  afternoon.

### M4b — reactions, withdrawals, presence, pages, notices (2026-10-11)

- **Decisions:**
  - **Reactions are NIP-25 (kind 7) with our `h` and `channel` tags**,
    judged as a message — no separate permission bit. One target, 1–32
    characters.
  - **Withdrawing is NIP-09 (kind 5), the author's own only, and only kinds
    9 and 7** (`hit_group::DELETABLE_KINDS`). The store applies it
    (`apply_deletion`) and refuses a withdrawn event that arrives later
    (`is_deleted`), so negentropy cannot resurrect it. A withdrawal naming a
    join or an action is ignored for it: the fold is not NIP-09's to
    rewrite. Withdrawals came in with reactions because un-reacting needs
    them; deleting one's own message came free.
  - **Presence is ours, kind 20436, ephemeral**: never stored or synced,
    on the group's gossip only, refused from non-members and when older than
    120 s. 30 s heartbeat, 75 s window, and a device that completed a sync
    with us counts too. The window says *online*; the member list splits
    online and offline.
  - **Events said in a channel no longer refold the group.** Before, every
    group event but a message took the fold lock and refolded; with
    reactions and presence that would have been a refold per emoji.
  - **The member list pages in the window, not over the bridge**: the
    state is already in memory on the Rust side.
  - **Read marks live in `read.json`** beside `app.json` (app state, not
    events); a mark only moves forward.
  - **The frontend's notices are generated from the build**, not
    `package.json`: Svelte is a dev dependency compiled into the bundle.
    Both notices files ship as bundle resources.
  - **S5's second endpoint is deferred; the badge stays** (PLAN, M4b).
- **Mistakes, caught by tests or by driving the window:**
  - A presence test passed in 0.04 s without testing gossip at all: Bob's
    sync had already made him "present". It now watches for the presence
    event itself.
  - Two of my tests assumed the order of two messages sent in one second
    (`created_at` is whole seconds; peers agree on `(created_at, id)`, which
    is not typing order).
  - Four focus defects in the window (TESTING.md §3): closing a dialog left
    nothing focused; WebKitGTK's `<select>` swallowed Escape; withdrawing
    removed the focused button; every message in the Tab order.
- **Results:** 112 tests (+6: three fold tests, a store test, the node's
  `reactions.rs`, an app test), stable three times; clippy clean; `npm run
  check` 0/0; both licence checks pass. Two windows driven from the
  keyboard: import, join, react, un-react, withdraw, presence.
- **Not done / open:** `TESTING.md` §4. A private channel's reactions and
  withdrawals go by sync, like its messages. Reactions show the five quick
  ones; there is no emoji picker. Same-second messages from two people show
  in `(created_at, id)` order.

### M5 — moderation without a server, on one machine (2026-10-11)

- **Decisions:**
  - **NIP-29's kinds where they exist, ours where they do not.** Ban and
    unban are `9001` with `["ban"]` / `["unban"]` (a ban of someone who left
    is how you keep them out); group edits are `9002` with our edit tags —
    name, channels with slowmode, overrides, role definitions — all or
    nothing, judged against the state *before* the event; a moderator's
    removal is `9005` with our `channel` tag; mute is ours, `9030`, with an
    end time. Snapshots carry bans, mutes and slowmode, so compaction keeps
    them.
  - **HIT_ENTER_2's rules, unchanged:** a moderator removes anyone's message
    in a channel where they hold `MANAGE_MESSAGES`, no rank; no slowmode
    exemption; the default role is the owner's alone; an override cannot
    hand out what its author lacks there; only channel bits survive one.
  - **Silences.** Kick, ban and leave silence their target until they are
    back; a mute, for its span. Peers refuse a silenced person's messages
    and reactions and **drop** ones that beat the silencing action to them.
    Messages are still judged by membership *now* for anything dated before
    a removal: judging by membership at the message's time would let a
    banned user backdate into the past. Said in `PROTOCOL.md`.
  - **Slowmode is per peer and honest about it**: judged against held
    messages, it stops honest clients and bursts, not a liar who spaces
    timestamps.
  - **A removed person may read the action that removed them** — the one
    exception to "a non-member reads nothing" — or their app never learns.
  - **The node's fold lock is a read/write lock**, taken before judging:
    state changes write, every other group event reads.
  - `Node::sign_unjudged` / `push_unjudged` and `hit-cli lie` exist to be
    the lying client. An honest node never uses them.
- **Mistakes, mine, all caught by tests, the script or the window:**
  - A ban could be applied, in the fold's same-second retry, *before* the
    target's join — leaving them banned with no silence, so their later
    messages stayed everywhere. M5's done-when test caught it about one run
    in three. A ban now always silences. On the way I first blamed, and
    fixed, a real but different race (a message judged before a ban and
    stored after its purge): the read/write lock.
  - A running node never re-read group state another process wrote to its
    store — the CLI's `ban` while `run` serves — so the moderator who banned
    M took M's lies. Found by the by-hand script; the sync loop now refolds
    a group whose events changed. M3's `kick` had the same hole.
  - A removed person never learned it: the removal cut their gossip, and
    sync served them nothing. Found driving the window.
  - Two tests mine: a rejoin signed in the same second as the original
    join *is* the original join (same id — "duplicate", accepted).
  - The muted composer was `disabled`, which drops focus for good; the ban
    confirmation lost focus.
- **Results:** 171 tests, stable three times; `fold.rs` 1,844 lines (HIT_ENTER_2's
  `permissions.rs`: 1,239); clippy clean; `npm run check` 0/0; licences
  unchanged. `scripts/m5-by-hand.sh` as `TESTING.md` §1 says; M2's and M3's
  scripts re-run, unchanged. Two windows: mute, unmute, ban, the banned
  side's banner, group settings.
- **Not done / open:** invite revocation and use caps; override editing in
  the UI; the two-machine runs; `TESTING.md` §4.
- Next: `TESTING.md` §4 and v0.1.0 when a friend is available; otherwise M6
  (anchors).

### M6 — anchors, on one machine (2026-10-11)

- **Decisions:**
  - **An anchor is a member.** It joins with an invite like anyone and says
    it is an anchor with NIP-24's `"bot": true` in its kind 0. Every rule —
    sync, gossip, reading, storing — is then the member's rule; nothing in
    `hit-node` is anchor-only. Apps label it; invite links name its devices
    first.
  - **Keys on a server come from systemd credentials**, not a plain file:
    encrypted at rest, decrypted into the service's ramfs. `init
    --credentials` and `hit-cli group export-key` write a key to standard
    output **only into a pipe** (`systemd-creds encrypt`), never a terminal.
    The rule "no secret in a plain file" holds; "the nsec lives in the OS
    keychain" is read as "in the OS's secret store", which on a server is
    this.
  - **The `ws://` bridge reads, it does not post.** NIP-42 with the
    person's own key, then exactly the `hit/nostr/1` answer for that npub.
    Group state is **translated** to NIP-29 and signed by the anchor, served
    only there — never stored or synced. NIP-29 clients' posts lack a
    `channel`; letting them post is a protocol decision left open.
  - **TLS is a proxy's job** (Caddy in `SELF_HOSTING.md`): no certificate
    code. axum (MIT) is in `hit-anchor` only — CLAUDE.md's "no HTTP or
    WebSocket in the chat plane" holds.
  - **`iroh-relay` is n0's binary**, documented, not embedded; an anchor
    (and the CLI's node) can point at it with `--relays <url>`.
  - Compaction runs on a timer, only for groups whose key the anchor
    holds and only when actions are pending (`GroupState::live_actions`).
- **Mistakes, mine:**
  - The by-hand script failed one run in three. The join said "timed out",
    hiding the anchor's real answer behind a dead member's timeout; the join
    error now lists every peer's answer, which showed the anchor refusing:
    the invite had not reached it, because the script killed the inviter
    straight after minting. The script waits now, and `SELF_HOSTING.md`
    says the real-world version: an invite made the moment before its maker
    goes offline does not work at 4am.
  - `hit-anchor run` stopped only on Ctrl-C; systemd stops with SIGTERM. It
    handles both now. (The script's leftover anchors were a different bug:
    backgrounding a shell function records the subshell's pid.)
  - `pgrep -f` with a pattern in my own command line killed my own shell —
    the CLAUDE.md rule against `pkill -f`, learned again.
- **Results:** 175 tests (+4: anchor profile, credentials, the two M6
  done-when tests), stable three times; clippy clean; licences pass with
  axum and tungstenite (one version of each). `scripts/m6-by-hand.sh` 8/8;
  M2's, M3's, M5's still pass.
- **Not done / open:** a real NIP-29 client against a real `wss://`
  (`TESTING.md` §5); posting from NIP-29 clients; the app pointing at an own
  relay; an anchor's DM inbox (M7) and files (M8).
- Next: M7 (DMs), or `TESTING.md` §4–§5 when a friend and a VPS are at hand.

### M7 — DMs, on one machine (2026-10-11)

- **Decisions:**
  - **NIP-17 as specified, none of the cryptography ours**: `nostr`'s
    NIP-44 and NIP-59 seal and wrap; `hit-event/src/dm.rs` only shapes it,
    inside `Person` (whose keys stay `pub(crate)`). One rumor, two wraps
    (recipient, self).
  - **Who keeps a wrap**: its recipient, and the anchors their inbox list
    names. The sender keeps an outbox copy (the node's `Local` path, not
    `hit-group`'s rule — `hit-group` cannot know who sent a wrap, which is
    the point of a wrap). Never on gossip.
  - **The inbox list is NIP-17's kind 10050 with our `anchor` rows**, each
    with a device id: that is how Alice reaches an anchor she shares no
    group with — and why the anchor then holds nothing of hers (her binding
    is refused as a stranger's). Kept between group-mates like a profile.
  - **Naming your anchors as your inbox is opt-in**, with the trade-off in
    the window: they cannot read a DM or tell who sent it, but can tell that
    someone wrote to you, and when. Asked of the owner.
  - **Not built, asked of the owner:** publishing profile and inbox to
    public relays (an outbound WebSocket from the app) — and with it DMs
    with Amethyst/0xchat and DMs to people you share no group with.
- **Mistakes:**
  - The byte scan first failed reading a file that vanished: SQLite folds
    its WAL into the database and deletes it after the last connection
    closes, after `shutdown` returns. The scan waits and skips a vanished
    file; its bytes are in `node.db` by then.
  - Driving the window: my key sequence missed the invite button (the
    owner's new *settings* button), and pasted the clipboard's unrelated
    contents into Bob's join dialog — refused, correctly. And the DM button
    opened a list with nothing to type into; it now opens the latest
    conversation with its composer focused.
- **Results:** 181 tests (+6: two `dm.rs`, a fold DM test, two
  `m7_done_when.rs`, an app DM test), stable three times; clippy clean;
  `npm run check` 0/0; licences unchanged. Two windows: a DM each way,
  unread badges, a badge surviving a restart.
- Next: M8 (files), or the by-hand runs still owed (`TESTING.md` §2, §4,
  §5) when a friend and a VPS are available.

### M8 — files, on one machine (2026-10-11)

- **Decisions:**
  - **NIP-92 `imeta` on kind 9**, `url hit-blob:<blake3>` plus NIP-94's
    `x` (SHA-256) for a Blossom mirror later; no provider list in the
    message. Limits (10 files, 50 MB, name and type lengths, hash shape) are
    part of judging the message, by every peer.
  - **`iroh-blobs` 0.103.1** (accepts iroh ^1.0), `fs-store` only, on our own
    accept loop. HIT_ENTER_2's rules carried over: pull-only; never
    `Remote::fetch`; two serving gates (a device that speaks for someone; a
    hash attached to a message they may read — so a private channel's files
    are private); push refused; get-many all or nothing; observe gated; GC
    by a protect callback reading the live set fresh, aborting on error.
  - **The live set is the event store**: an `attachments` index filled when
    a message is stored and emptied with it (cascade) — withdrawals,
    moderator removals and silences reclaim files with no extra code.
  - **A member caches** its own files and the most recently used up to
    1 GB; **an anchor pins all** (`FilePolicy`) and fetches what is missing.
  - **Fetch races the dials**: every candidate provider at once, the
    transfer over the first that serves. The author is often the one who
    has gone; waiting out their dial cost 10–30 s a file.
  - **DM files split off as M8b** (NIP-17 kind 15 needs the file encrypted
    and its key sealed — a piece of its own).
  - `hit-cli post-file` / `get-file`, for the by-hand script.
- **Mistakes and finds:**
  - The first done-when run took 30 s: the dead uploader's dial timed out
    before anyone else was asked. Then 13 s with a 10 s dial timeout; then
    3 s with the dials raced.
  - **The window's CSP blocked every inline picture** (`default-src 'self'`
    has no `blob:`), and the bytes needed `Uint8Array` before a Blob. Both
    found driving the window. HIT_ENTER_2 has the same CSP line — worth a
    look there.
  - The GTK open dialog, like M4's save dialog, cannot be driven with
    `wtype`. And a screenshot of it showed the owner's home folder; deleted,
    and not repeated.
  - Clippy's `cloned_ref_to_slice_refs` in six places.
- **Results:** 186 tests (+5: an `attachments_of` test, a fold file test, a
  store index-and-cache test, `m8_done_when.rs`, an app file test), stable
  three times; clippy clean; `npm run check` 0/0; licences pass with
  `iroh-blobs` (redb, bao-tree and the rest: MIT/Apache).
  `scripts/m8-by-hand.sh` twice.
- Next: M8b (DM files) or M9 (voice); the by-hand runs still owed
  (`TESTING.md` §2, §4, §5).

### M8b — DM files, on one machine (2026-10-11)

- **Decisions:**
  - **NIP-17 kind 15 with AES-256-GCM** from RustCrypto's `aes-gcm` (in the
    tree already): a fresh key and nonce per file, `ox` checked after
    decrypting. No cipher code of ours; `hit-event/src/dm.rs` calls it.
  - **Only ciphertext is stored or sent**, as an ordinary blob. The
    decrypted bytes exist in memory on the way to the window (or to a file
    the person chose with *save*).
  - **Who may be handed the ciphertext**: the two people, by a
    `blob_grants` row each node writes as it sends or receives the file —
    a sealed message tells no node who may read it, so the gate needs this.
  - **The window never holds a key**: `DmView` carries name, type, size and
    hash; it asks for bytes by DM id.
  - **Honest limit:** an anchor holds the wrap but cannot open it, so it
    cannot serve the file; it comes from the sender's or recipient's own
    devices.
- **Results:** 189 tests (+3: a seal/open/wrap test, `m8b_dm_files.rs`
  with a byte scan of both data directories and a refused third member, an
  app test asserting no key reaches the window), stable three times;
  clippy clean; licences pass (`rand_core` 0.6 joins). Driven: a DM file
  from `hit-cli dm-file` shown inline in a window.
- Next: M9 (voice) — or the by-hand runs still owed (`TESTING.md` §2, §4,
  §5) when a friend and a VPS are at hand.

### M9 — voice, on one machine (2026-10-11)

- **Decisions:**
  - **Our own `hit/voice/1`, not `iroh-roq`**: the latter exists only for
    iroh 0.33. A JSON line each way on one bi-stream (`Join`, `Answer`),
    then one Opus frame per QUIC datagram: `[version][seq u16][flags][opus]`.
  - **Opus 0.3** (system libopus 1.6.1 through pkg-config; 0.4 wants cmake,
    not installed). 20 ms frames, VoIP, 32 kbit/s, in-band FEC at 10 %.
  - **A jitter buffer of our own** (3 frames): plays, recovers a lost frame
    from the next one's FEC, conceals, restarts when starved. Four tests.
  - **Presence is kind 20437**, a sibling of 20436 rather than a field in
    it: `in` every 5 s, `out` on leaving and on quit, 15 s window. Judged
    like 20436 plus a voice-channel and `VIEW_CHANNEL` check.
  - **Mesh, cap 8, lower device id dials** — no coordinator, never two
    connections for one pair.
  - **The listener is the gate**, by the *device* the connection proves
    (`Node::persons_of`); and each listener decides who is *heard*
    (`SEND_MESSAGES`, not muted), every second — so a moderator's mute
    works mid-call without trusting the muted person's app.
  - **The node gained pluggable protocols** (`Node::serve_protocol`): an
    ALPN and a handler, joined to the accept loop. `hit-voice` depends on
    `hit-node`, not the other way round.
  - **Devices**: cpal 0.18 on its own thread (a `Stream` is not `Send`
    everywhere), 48 kHz mono inside, linear resampling and downmix at the
    edges, the mic bounded at 80 ms and the speakers at 200 ms (drop the
    oldest). Devices chosen **by id** — cpal 0.18's names are descriptions
    and repeat.
  - **WebRTC audio processing 0.5, `bundled`** (the system's
    `webrtc-audio-processing-1` is the wrong major for its `.pc`):
    delay-agnostic echo cancellation, moderate noise suppression, high-pass.
    Its crates name a `license-file`; `about.toml` clarifies both as
    BSD-3-Clause with checksums of the text read.
  - **In the app**: `HIT_AUDIO_IN` / `HIT_AUDIO_OUT` beat the settings —
    for the null-sink recipe. Push-to-talk is a window key (`` ` ``) and a
    hold button; a global hotkey is not attempted.
- **Mistakes and finds:**
  - **I opened the owner's real microphone and speakers for about four
    seconds.** A re-run of the `devices` example "to list the devices" ran
    its loop on the system defaults (a 440 Hz tone out, levels in; nothing
    recorded or kept). The example now refuses to open audio unless both
    `IN` and `OUT` are named, and that is in memory for future sessions.
  - The steady test tone came through at a sixth of its level with
    processing on: noise suppression doing its job. Bursts in the GUI run,
    and echo cancellation measured on its own (0.2105 → 0.0013).
  - Under `wtype`, `KeyboardEvent.code` was not `Backquote`; the talk key
    also matches the character now.
  - A jitter test expected the wrong slot; the accept loop held an
    `RwLock` guard across an await (moved into a helper).
  - Found while documenting: **a voice channel not everyone can view does
    not work** — gossip carries only channels everyone can view, and
    ephemeral events are never synced. Written down as open, not fixed.
- **Results:** 197 tests (+8: codec, wire, four jitter, resampling,
  `m9_voice.rs`), stable twice; clippy clean; `npm run check` 0/0; licences
  pass with cpal, opus, audiopus_sys and webrtc-audio-processing. Driven in
  two windows on null sinks (`TESTING.md` §4d).
- Next: §6, a real call with people — two first, one on speakers; then
  M10 (zaps) or the M9 gaps (private voice channels, DM calls).

### M9 — 1:1 calls from a DM (2026-10-11)

- **Decisions:**
  - **Same ALPN, a second kind of join**: `{"dm": <binding>}` beside the
    channel join (an untagged enum, so the channel line is unchanged). The
    binding travels in the join because the callee may hold none for the
    caller; it is checked by the node's `BindingBook::authenticate`, the
    same as a `hit/nostr/1` hello (`Node::authenticate`, new).
  - **Who may make an app ring**: someone it shares a group with, or has
    *sent* a DM to. Receiving a DM is not consent — otherwise any stranger
    could write one line and then ring.
  - **Ring every device, first answer wins**; the losers' connections are
    dropped, which is what stops them ringing. 45 s, then "no answer".
  - **The callee answers by its connection's handshake**: the answer line
    is written only when the person picks up, so the caller simply waits
    on it. "declined", "busy", "no answer" come back as the `why`.
  - **An answered call ends when the other connection goes**; the window
    shows why ("declined", "busy", "no answer", "they hung up") for 8 s.
  - **The banner does not take focus** — a person mid-sentence would answer
    with Enter. The desktop is asked for attention instead (Tauri's
    `request_user_attention`); there is no ring tone.
- **Limits, said plainly:** a callee whose device bindings this node has
  never seen cannot be called — in practice, a group in common is needed.
  Fetching a binding from the callee's inbox anchors would lift it.
- **Mistakes and finds:** the "answered" flag was first set before the
  peer was added, so the once-a-second upkeep could end a call in the gap —
  moved after. Two focus losses in the window (the *call* button and the
  banner each vanished with focus inside them). The ended toast covered
  the *call* button; moved below the header.
- **Results:** 198 tests (+1: `m9_dm_calls.rs` — answered both ways and
  heard, busy, hung up, declined, unanswered, caller gives up, a stranger
  with a valid binding refused without ringing, then allowed once written
  to, and consent one-way), stable twice; clippy clean; `npm run check`
  0/0. Driven in two windows on null sinks (`TESTING.md` §4d).

### M9 — private voice channels (2026-10-11)

- **Decisions:**
  - **Presence for a restricted room goes direct, never on gossip**:
    `Node::announce_voice` hands the 20437 to every device of every member
    who may view the channel, over `hit/nostr/1`, each heartbeat (4 s cap
    per hand-off). Gossiping it would tell every member who sits in a room
    they cannot see. Cost grows with the eligible devices; fine for the
    staff room it is for.
  - **The app's one shape of privacy**: the owner's *private* box sets
    overrides for every role — Member denied `VIEW_CHANNEL`, every other
    role allowed. Owner-only because overriding a role needs outranking
    it. Finer overrides stay the CLI's.
  - **🔒 means "the Member role is denied view"**, not "not every current
    member can view" (the gossip rule) — with only owner and moderators in
    a group, nobody is shut out yet, but the room is private.
  - **A node's own edits, puts, removals and mutes are dated after its own
    last one of the same kind** in that group. Found by the app test: a
    private toggle and its undo in one second folded in id order and the
    undo lost. Narrowed after the first attempt — dating every action after
    the group's newest — broke every join (an invite dated after the
    group's creation was newer than the join that used it).
- **Mistakes and finds:** disabling a settings control while it saves
  took focus off it (WebKitGTK then sends no keys) — the dialog now
  restores focus after every action, which also fixes M5's slowmode select.
- **Results:** 199 tests (+1 `m9_private_voice.rs`, checked to fail with
  the direct hand-off turned off; the app test extended), stable;
  `scripts/m5-by-hand.sh` and `m8-by-hand.sh` still pass. Driven: the
  private box and the lock in one window.

### M9 — calling someone with no group in common (2026-10-11)

- **The finding first:** before this, a DM could not reach someone with no
  group in common *at all* — their devices were unknown and their inbox
  list (kind 10050) is a personal kind `hit-group` stores only for group
  mates. Calls inherited that.
- **Decisions:**
  - **Every DM carries the sender's device binding**, sealed in the rumor
    (`["device", …]`). Consent-shaped: you reveal your devices to the
    people you write to, and only they can read it. Kept by the recipient
    after the binding book's checks and only if its author is the sender;
    stored (`Inner::learn_binding`, the one path that stores a binding
    `hit-group` would refuse), so it outlives a restart and is refreshed by
    every DM past the 7-day binding life.
  - **A contact link for first contact**, named like the invite link but
    carrying no binding — a binding expires within days; the link should
    last as long as the device. Opening it dials once and takes the
    binding from the `hit/nostr/1` hello, which proves it.
  - **The call gate is unchanged**: a link lets you write, not ring.
  - **With no groups, the app shows DMs**; the welcome screen offers
    *write to someone*.
- **Mistakes and finds:** the first run lost Carol's binding on restart —
  the DM came over a session whose hello had already put her binding in
  the book, so `learn_binding` saw it as stale and skipped the store; it now
  always stores. Two window defects with no groups (above, `TESTING.md`).
- **Limits:** names stay npubs between people with no group (profiles are
  group-scoped). A contact link's device must be online once. Neither side
  has an inbox anchor unless a group gave them one, so a DM to someone
  offline waits on the sender's device.
- **Results:** 203 tests (+4: contact link parsing ×2, the sealed device
  tag, `m9_no_group_calls.rs`), stable twice. Driven in two windows on null
  sinks.

### M9 — drift compensation (2026-10-11)

- **Decisions:**
  - **Correct where a hardware clock meets the tick, nowhere else**: the
    mic buffer (device fills, tick drains) and the speaker queue (tick
    fills, device drains). A controller per buffer (`drift.rs`) reads the
    level — smoothed, proportional plus a slow integral — and sets the
    edge resampler's ratio within ±0.5 % (a twentieth of a semitone;
    ten times any real device). Target 40 ms each: 30 ms left a 0.3 %
    slow clock 3 s of headroom and the controller arrived late.
  - **Between the machines, the jitter buffer**: both ends tick on
    NTP-disciplined system clocks, and voice activation empties it at
    every pause, which resets latency — so no resampling there, only a
    one-time skip-ahead if 120 ms beyond the delay pile up (a stall, or a
    very long speech).
  - **Count what still goes wrong** (`device::Stats`, S3's second
    inheritance): mic drops and short ticks, speaker drops and underruns —
    from the moment each buffer first fills, not before. The call bar shows
    a glitch count when it is not zero.
- **Results:** 207 tests (+4: three drift simulations — fast and slow
  clocks at ±0.3 % and ±0.05 % with no drop or gap for ten minutes and the
  ratio settling on the skew; the same clocks uncontrolled overflow or
  starve ~90 times; bounds — and the jitter catch-up), stable twice. A
  minute's call through real cpal devices on null sinks: all four counts
  zero. **Not shown:** a real drifting device — that needs hardware and a
  person (`TESTING.md` §6).

### Handover (2026-10-11, end of session)

- **Saved for the next session:** the window-driving helpers that lived in
  the agent's scratchpad are now `scripts/gui/` (launch, onboard, drive,
  shot, sinks, stop), smoke-tested; CLAUDE.md says where this stands, the
  three ways to go next, and what waits on the owner.
- **Waiting on the owner:** the M7 inbox default and public relays; a
  Bluetooth HFP warning; whether DMs should carry the sender's name so
  people with no group in common are not npubs to each other.
- **Lessons this session paid for**, in CLAUDE.md: WebKitGTK's
  focus-or-no-keys (five separate bugs, M4–M9); never open audio without
  naming the device (the real mic and speakers were opened once by an
  example, ~4 s, nothing kept); `pgrep -x`, never `pkill -f`.

### M10 — zaps, against a fake network (2026-10-11)

- **Asked:** "start building hit-zap against fakes" — the owner has no
  wallets set up yet; S4 and the done-when wait for real ones.
- **Decisions:**
  - **A receipt enters a group inside a carrier** (kind 9436, ours): a 9735
    is signed by the provider, not a member, and has no `h`. The carrier
    is a channel kind, judged like a reaction (view the channel) plus
    NIP-57's checks of the receipt inside.
  - **Whose provider counts is the recipient's word**: `zap_pubkey` in
    their profile, written by their app from their address's server. NIP-57
    has the *sender* fetch it over HTTPS; a third member cannot, and
    trusting the carrier's author would let any member forge totals. A
    forged receipt is stored and counted nowhere; the check runs when
    counting, since the profile may arrive after the carrier.
  - **Receipts go only to the group's anchors** (their `relay` profile
    field); the anchor takes a 9735 without NIP-42, checks it and carries
    it in. No public-relay fallback (an open question, `DESIGN.md` §15).
  - **Check the invoice before paying it**: exactly the amount, and
    committing to the request. A lying address server is caught (tested).
  - **Our own small relay client** for NWC and receipts, on tungstenite and
    rustls with iroh's `ring` — no `nostr-sdk`, no second crypto library;
    `nostr`'s `nip47` types do the NWC encryption. Our own bech32 and
    bolt11 field reader (no length limit, no signature check — NIP-57 does
    not ask one); BOLT 11's coffee vector is the test.
  - **No fee address yet**: `FEE_ADDRESS = None`, so nothing is charged or
    shown; a debug build takes `HIT_ZAP_FEE_ADDRESS` for testing.
  - **The NWC string is a secret** like the nsec: keychain only
    (`nwc:<npub>`), crosses `invoke` once, `Wallet`'s `Debug` prints none.
  - **QR fallback**: our SVG from the `qrcode` crate (MIT/Apache, no
    dependencies with `image` off); the fee is a second QR after the zap.
- **Mistakes and finds:** the QR step ended at once in the window — the
  anchor's relay answers a `REQ` with `auth-required`, which closed the
  receipt watch; the app now also watches for the anchor's carrier. The
  done-when test was mutation-checked: with the provider-key check removed
  it fails at "the forgery counted".
- **Results:** 220 tests (+13: bolt11 and LNURL vectors, receipt checks,
  anonymous zaps, the carrier, profile fields, the fee's arithmetic,
  addresses, two fake-network zap runs, the app's wallet and QR flows, and
  `m10_done_when.rs`). Driven in two windows against the fake network
  (`TESTING.md` §7a). **Not done:** S4 and the done-when with real wallets
  (§7b).

### Handover (2026-10-11, after M10)

- **Saved:** `scripts/gui/stop.sh` now also stops the test anchor and the
  fake network, and waits for every process to exit — a window still
  hanging up swallowed the next launch through the single-instance plugin
  (found relaunching for the zap QR fix). README and CLAUDE.md say so.
- **Next:** CLAUDE.md lists four starts — a real call (§6), the two-machine
  runs and v0.1.0 (§2, §4, §5), real zaps (§7b), or M11 hosted anchors.
- **Waiting on the owner:** the fee address; zaps in groups without a
  `wss://` anchor; and the earlier M7/M9 questions.

### Session of 2026-10-12 — wallets for real zaps (no code)

- **Funding:** the owner bought 13,552 sats (≈53 RON) on Kraken and
  withdrew some over Lightning to a Coinos invoice — it arrived. Kraken
  pays invoices, not Lightning addresses; each withdrawal needs a new one.
- **Alby Hub dropped** (the owner found it too hard; our users won't run
  it either). **Coinos sends over NWC; Wallet of Satoshi receives** — a
  different provider, so a real Lightning payment. WoS's LNURL-pay answer
  checked: `allowsNostr`, a `nostrPubkey`, 1-sat minimum. Primal is the
  fallback receiver (needs a Nostr identity, wallet limited by country).
- **`wss://` without a VPS:** a Cloudflare quick tunnel to the anchor's
  `--ws` port (`TESTING.md` §7b). `cloudflared` is installed. Its address
  changes per run.
- **Not done:** the anchor and tunnel not started; no NWC connection made;
  no sats spent. S4 is next when the owner picks it.
- Explained to the owner: what an anchor is, what `wss://` is for, why a
  zap's receipt needs it. The NWC `secret` is a spending key, not the
  wallet's key — never paste it anywhere.

### Session of 2026-10-18 — M12: the Slint app, the switch, beta.3

- **Decided (owner):** Slint, keep the design, no bake-off; benchmark once
  before the first Slint release; Linux and macOS in one release.
- **Built, M12-1…M12-9:** `hit-app-core` (logic and event pump out of
  Tauri), `hit-desktop` (onboarding; groups, channels, messages, members;
  settings, moderation, reactions; DMs, files, pictures; voice and calls;
  zaps and the profile; one window per data dir, `hit-enter:` links,
  window focus), each driven in two windows from the keyboard.
- **Traps found** (CLAUDE.md "The app" has the list): Slint's software
  renderer draws no colour emoji and no SVG (Noto Emoji monochrome
  bundled; the QR drawn in Rust); new models drop focus (patch in place);
  wrapped text needs a width; a window whose least width beats its tile is
  clipped by Hyprland; **glibc's arenas kept 506 MB of freed photo
  decodes** (fixed mmap threshold, two arenas, one decode at a time).
- **Found about releases:** the Tauri betas were built on Arch and needed
  glibc 2.43 — **beta.2 never ran on Debian or Ubuntu** (checked in a
  container). Linux builds now run in Ubuntu 22.04. A `.deb` version must
  sort above the last (`0.1.0~beta.3` sorted below `0.1.0-beta.2`).
- **Benchmark (Linux, `scripts/bench/`):** native vs Tauri: 32 vs 264 MB
  (sign-up), 62 vs 620 (1,000 messages), ~100 vs 734 (50 photos); Discord
  idle 533 MB. The five-minute call run was stopped by the owner (M12-7's
  short call: 55 MB). The M1: 81 MB after a call.
- **Released:** v0.1.0-beta.3 (pre-release), Linux built here from
  `3a0483e`, macOS on the M1 (links by Apple Event added there); every file
  re-downloaded and checked; hitenter.app updated and deployed.
- **Mistakes, said:** a test command started the owner's real app once
  (`hit-enter --version` — there is no such flag); stale debug windows
  survived `stop.sh` until it matched debug builds; an invite test
  overwrote the owner's clipboard (later runs saved and restored it).
- **Next:** the owner's pick — Windows (new, item 0 in CLAUDE.md), a real
  call, the two-machine runs, real zaps, or hosted anchors.

---

## 2026-10-22 — open source, MIT

- **Decided (owner): HIT_ENTER is open source under MIT.** Reverses the
  2026-09-27 "closed source, all rights reserved" — `DESIGN.md` §15 q1 has
  both. MIT over AGPL by the owner's choice, after the trade-off was said:
  anyone may fork, close a fork, or drop the 1% fee.
- **Published** to the public `isciuciustin/HIT_ENTER` as one fresh commit
  (owner's choice: no history), every doc included; that repo's old `main`
  (the original HIT_ENTER) was overwritten, its tags `v0.1.0`/`v0.1.1`
  kept. Binaries stay on `isciuciustin/hitenter`.
- **Done for it:** `LICENSE`, `license = "MIT"` on every crate, notices and
  `about.toml` reworded, the About box, README; the owner's receiving
  Lightning address removed from `TESTING.md`.

## 2026-10-22 — the north star: public communities

- **Decided (owner): HIT_ENTER is "the next Discord, a better one", for
  public communities on decentralized software** — not privacy-first.
  The measure: *the Unturned Discord community running on HIT_ENTER*.
  Groups are mostly public; DMs stay end-to-end encrypted.
- **Done:** `DESIGN.md` §0 (the star, and the six gaps between today and a
  community that size), README, `CLAUDE.md`, the GitHub description and
  hitenter.app reworded. Nothing in the code changed.

## 2026-10-24 — the VPS: the first always-on anchor

- **Rented early** (PLAN.md §0 had it for November): OVH VPS-1, 2 vCore,
  4 GB, 40 GB NVMe, Debian 13, automated backups, ~$5.35/month, no
  commitment. **In Canada (Beauharnois)** — kept by the owner although
  Europe was recommended (~100–130 ms more for European users); can move.
  `148.113.150.182` / `2607:5300:205:200::e7f4`, user `debian`, key-only
  SSH (`ssh hitenter` on the laptop).
- **Hardened:** ufw (OpenSSH, 80/tcp, 443/tcp, 7600/udp), unattended
  upgrades, fail2ban, no root or password login. **Trap:** OVH's
  `50-cloud-init.conf` says `PasswordAuthentication yes`, and in sshd the
  *first* value wins — a `99-` override does nothing; it is
  `00-hardening.conf`. Check with `sudo sshd -T`.
- **`hit-anchor` running** under systemd as `SELF_HOSTING.md` says:
  built in the Ubuntu 22.04 release image (`target/u2204`, glibc ≤ 2.34),
  npub `npub1n3psr29lffm8d0jfxqxyga97asduzfcs0p0q88lgpum0rywlv30q874trj`,
  its key only in `/etc/credstore.encrypted/` (no TPM: systemd warns the
  host key is not on encrypted media).
- **`wss://anchor.hitenter.app` moved off the laptop's Cloudflare tunnel**:
  Caddy 2.6.2 (Debian's) proxies to `127.0.0.1:7777`, Let's Encrypt
  certificate, DNS `A` + `AAAA` *DNS only* (grey cloud), set by the owner
  in the dashboard — this machine has no Cloudflare token that edits DNS.
  Checked: NIP-11 answers over HTTPS, the WebSocket upgrade gives `101`.
  The old locally-managed tunnel `hit-anchor` is idle, not deleted.
- **First group pinned** (`npub12k7vf030…5eflal`): 15 events and 1 file.
  **`pin` needs the service stopped** — it starts a second node with the
  same device key and port. **Open:** every sync round the owner's app
  offers 23 events the anchor refuses as "not a member" — most likely
  profiles/bindings of people from the owner's other groups (refused by
  design, `hit-group/src/lib.rs` `is_personal`), *not confirmed*: the log
  does not name the kind. Owed: log the kind, stop re-offering refused
  events, and the owner's check that the anchor shows in the member list
  and delivers a message sent while the owner was offline.
- **Owner's laptop:** the beta.3 app in `~/.local/bin/hit-enter` replaced
  by beta.4 from the release (sha256 checked; the AppImage unpacked —
  Omarchy has no FUSE 2). KeePassXC set up for secrets.

## 2026-10-24 (second session) — the 23 refused events: expired invites

- **Found, not guessed.** The owner's app store, copied and replayed
  through `hit-group` as an empty anchor would take it (apply order,
  refold after each state change, retries): **30 held, 23 refused** — the
  VPS's numbers. Judged against the *final* fold instead, 0 refused: the
  bug only shows to a node that arrives late.
- **The cause:** a join (`9021`) is honoured by an invite valid *at the
  join's time*, but `is_folded` treated every **expired** invite as
  folded — not synced, refused. A node arriving after the expiry can
  never apply the joins it honoured. Four members' joins failed ("not
  applied"), and their 19 profiles, bindings, messages, reactions and
  deletions were refused as "not a member". *Not* other groups' profiles
  (yesterday's guess): the owner's app holds one group.
- **The fix:** an invite is folded only once it expired **before the
  snapshot's cutoff** — then every join it honoured is folded too, into
  the snapshot's member list. Replayed with the fix: 58 held, 0 refused.
  Test `an_expired_invite_is_kept_and_synced_until_a_snapshot_passes_its_expiry`;
  `PROTOCOL.md`, the fold, step 4.
- **Both sides need it** — the app to offer the invites, the anchor to take
  them: the anchor redeployed, and beta.5. **The workaround today:** the
  owner compacts the group (group settings): the new snapshot names the
  four as members, and the current anchor takes the rest.
- **Logs:** a refused push (both ends) and a refused fetch now name the
  event's kind, short id and author — never its content.
- **Re-offering refused events: kept, deliberately** (`PLAN.md` §0 asked
  to stop it). Both peers run the same rules, so a lasting refusal means
  their states disagree — a bug, as here, or one side not caught up yet.
  Suppressing the offer would have hidden this bug and kept the four
  members' history off the anchor even after a compaction fixed the state;
  re-offering 23 events a round costs nothing worth saving.
- **Open, same family:** `judge` refuses an invite whose author may not
  invite *now* (left, or lost the permission). A late node then cannot
  apply joins that invite honoured while its author could. Not hit yet;
  the fix needs a rule for keeping such invites without letting strangers
  store them (e.g. keep one only if folding it applies a join).
- **`target/` is owned by root** (yesterday's Docker build), so cargo
  cannot write `target/debug`; this session built in
  `~/.cache/hit-enter-target`. Fix: `sudo chown -R $USER target`.
- **After the owner's compaction** (19:17 UTC): the 23 stopped — the four
  members and their history are on the anchor. The new anchor binary
  (`hit-anchor.prev` kept beside it) logged them by kind first, which
  matched the replay event for event.
- **Then one more, every ~3 s both ways:** a member's **expired device
  binding** (offline past its 7 days), offered by the owner's app and
  refused by the anchor's binding book. `reconcilable` now leaves expired
  bindings out (test `an_expired_binding_is_not_offered`, failing before).
  It is the app that offers it, so the noise stops with beta.5, not with
  an anchor redeploy.
- **The zap dialog's ~4 s give-up, fixed.** Why ~4 s: the receipt watch
  subscribes on the group's anchor relays, the anchor answers `CLOSED
  auth-required`, every watch task ends, the channel closes and `wait`
  returns at once — not after its 30 s. Now a wallet zap not counted at
  once is left pending, and the dialog shows "Paid N sats to X. Waiting
  for its receipt to reach the group…" then waits with `zap_wait` (the QR
  path's: the carrier, up to two minutes) and replaces the line — only if
  the dialog still shows it, not a newer zap's. The fake network gained
  `receipts_need_auth()`; the app test zaps through it and has Carol's
  node carry the receipt in. Not driven in the window, not yet real sats.
- **The owner answered the public-groups questions:** (1) in a public
  group a newcomer **posts at once**; (2) profiles **go to public Nostr
  relays by default**; (3) the DM inbox is **on by default, with limits** —
  asked whether it would fill the VPS (it would: anchors keep DMs forever
  today, ~200 MB/day at Unturned scale, and anyone can flood one person),
  the owner chose a **7-day** expiry (30 recommended) and a **10 MB cap
  per person**. Written up as **M13 — Public communities** in `PLAN.md`,
  with two questions left: a new group public by default (recommended
  no), and which public relays.
- **And the last three:** a new group is **private by default**, but
  making it public must take little thought (one choice at creation, one
  switch in settings, one sentence of what it means); the **best public
  relays** (Damus, nos.lol, Primal — chosen here); and a **1-minute
  newcomer wait** before posting in a public group, which admins can
  change — refining "posts at once" against raids. M13 updated.
- **Raised: the newcomer wait is at least 10 minutes in a public group**,
  a floor in the fold that admins can raise but not lower (the owner, same
  session, after the 1-minute answer). Said back: Discord offers this as
  an optional level; here it is every public community's, and costs the
  newcomer who came to ask one question.
- **Settled at 5 minutes.** Asked whether 10 was too much (the owner
  found Unturned's 5 annoying): a fixed wait stops instant scripts at any
  length and patient raiders at none, so 10 buys little over 5 and costs
  every newcomer. Floor 5 minutes, admins may raise it; a raid mode that
  raises the wait when joins spike is the real defence, with automod.
- **M13 step 1 built: the public flag.** NIP-29's own words on `39000`:
  `public`+`open` or `private`+`closed`, always one pair — NIP-29 reads a
  group with neither as public, and every group before M13 has neither,
  so ours reads public only from `public` without `private`. `Edit::Public`
  in a `9002`, **owner only** — a test gives someone a role holding every
  permission: they may rename, not publicize. Kept by `to_snapshot`. The
  bridge's translated `39000` uses the same tags. App: `create_group(name,
  public)`, `set_public`, `GroupView.public`; window: *private / public*
  links in "make a group" with one sentence under the choice, and a
  *make public / make private* button in settings for the owner (settings
  now offered to an owner who could do nothing else there). The 5-minute
  sentence waits for step 2, where it becomes true.
- **M13 step 2 built: join without an invite, and the newcomer wait.**
  `Action::Join { code: Option }`, `join_public`; the fold honours no code
  when the group is public at that point; `InviteLink.code: Option` (a
  public link has no `&c=`), `Node::public_link`. Newcomers: every
  self-join (by invite too) records its time until the next snapshot;
  `newcomer_wait` in `39000` / a `9002` edit with *manage channels*;
  `PUBLIC_NEWCOMER_FLOOR` = 300 s in the rule. **Found by the M5 test:**
  with a wait of 0 my first rule refused a message dated before its
  author's join ("wait 0 s") — no wait now means no rule. **Not built,
  on purpose:** a moderator's re-put lifting the wait — a `9000` changing
  no roles needs no permission, so a newcomer could put themselves. The
  window: invite dialog gives the public link in a public group (and
  *invite people* shows for every member there), a once-a-second ticker
  counts the composer down, settings has *newcomers wait: 5 min (the
  public minimum)*, and a channel's "private" is now **"mods only"** —
  the word "private" is the group's. Driven in two windows: Bob joined
  Alice's public group with a code-less link and watched 4:49 → 4:46.
  **Test windows reuse `target/gui/<name>`:** `stop.sh` forgets the
  identities but not the data, so a fresh Alice opened the old Alice's
  group — clear the folder between runs.
- **M13 step 3 built: read before joining.** Deviation from the plan,
  said: a public group's **actions and invites are readable** by
  non-members — the plan kept them members-only, but a reader's own node
  folds them to know who is a member, and without them it refused every
  message. Cost: an invite read while public still works if the group
  goes private before it expires. Profiles of a public group's members are
  anyone's; bindings stay members' (they name devices to dial).
  `Node::follow` reads a group **in memory only**. The bridge reads an
  unauthenticated client as a fresh key — the one rule then shows public
  groups only. **Found writing the node test:** a node dates its own
  second action of a kind in one second a second *ahead* (to keep their
  order), so a join right after "make public" in the same second was
  dated before it and refused — a test artifact (people do not join in
  the owner's same second), the test spaces the edits. Driven: Bob read
  Alice's public group with her name shown, joined from the banner, and
  the keyboard stayed in the composer (the button vanishing is the focus
  trap). `wtype`'s BackSpace does not reach Slint either.
