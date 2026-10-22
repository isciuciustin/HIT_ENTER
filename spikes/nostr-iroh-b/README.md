# S1 — Nostr over iroh

**Question (`PLAN.md` M0):** can two apps be two Nostr relays — NIP-01
`EVENT`/`REQ`/`EOSE` over an iroh ALPN, NIP-77 negentropy to converge after
one was offline — and which store sits behind it?

**Verdict (2026-10-10): yes.** Every assertion held on the first run, in one
process and across two real OS processes. **Our own SQLite behind `sqlx` is
enough as the store**; nothing in this spike argued for `nostr-lmdb`. What
was *not* tested is listed below — read it before leaning on the verdict.

## Run it

```bash
eval "$(mise env -s bash)"
cd spikes/nostr-iroh-b
cargo build --release
./target/release/nostr-iroh-spike demo   # one process, two endpoints, asserts
./run-two-processes.sh                   # two OS processes, relay restarted
```

Not a workspace member; own lockfile. `iroh =1.1.0` (the version
HIT_ENTER_2's transport code runs on), `nostr 0.45`, `negentropy 0.5`,
`sqlx 0.9` + SQLite. ~780 lines in four files.

## What it does

- **Wire:** ALPN `hit/nostr/1`, one QUIC bi-stream per session, frames are
  `u32` LE length + JSON, 1 MiB cap checked before allocating — HIT_ENTER's
  `he-proto` framing. Message bodies are the `nostr` crate's own
  `ClientMessage`/`RelayMessage`, so the JSON is NIP-01/NIP-77 exactly.
- **One node, two roles:** `node::serve` answers; `node::sync` dials. Both
  apps run both. Roles reversed in demo step 3 with no code difference.
- **Sync:** `NEG-OPEN` with the group filter → `NEG-MSG` rounds → ids we
  lack are fetched with `REQ ids` and **verified**, ids the peer lacks are
  pushed with `EVENT` and acknowledged with `OK`.
- **Store:** `events` (id, pubkey, created_at, kind, json) + `tags` for
  single-letter tags, so a `#h` group filter is an indexed lookup. Filters
  translate to SQL with `QueryBuilder`.

## Results

| | |
|---|---|
| 2000 shared events (727 KB JSON), 7 + 4 differing | reconciled in **2 rounds, 2.3 KB** on the wire, 12 ms, loopback |
| the same, naively "send everything" | 727 KB — **~320×** more |
| nothing changed | 1 round, **709 B**, 6.5 ms |
| 300 disjoint on each side (two processes) | 1 round, 20 KB (it is an id list), 92 ms, converged at 600 |
| relay killed, both write, relay restarted (new `EndpointId`) | converged at 608 each, 2 rounds, 3.3 KB |
| `#h=g1` sync with an `g2` event on the peer | 0 leaked: the filter scopes negentropy |
| forged event (content changed after signing) | `OK false "invalid: Invalid event ID"`, not stored |
| announced frame length 4 GiB | refused before allocation, stream dropped |
| peer identity | read from `conn.remote_id()`; the demo asserts it equals B's id |

**Proportional to the difference, not the history** — `DESIGN.md` §4's claim
holds, on a set of 2000.

## Not tested — the verdict does not cover these

- **Any network but loopback and a LAN.** `presets::Minimal`: no relay, no
  discovery, no hole punching. The ticket is `id@ip:port`. S5 and the
  cross-network S3 are where that gets answered.
- **Authorization.** Anyone who connects can `NEG-OPEN` any filter and
  `EVENT` anything with a valid signature. No binding-first handshake, no
  `hit-group`. That is M1/M2; this spike only proves the plumbing it
  plugs into.
- **Interop with other Nostr software.** The JSON is the `nostr` crate's, so
  it should be right, but nothing outside this spike has spoken to it.
  `DESIGN.md` §4 says the relay "can be tested with existing Nostr
  tooling" — the *messages* can, but existing tools speak WebSocket and
  nothing speaks iroh. A dev-only WS↔iroh bridge would be needed, outside
  the chat plane. **Open.**
- **Event semantics beyond "an event".** Every event is stored as regular.
  Replaceable/addressable kinds (group metadata, bindings by replacement),
  NIP-09 deletion and expiry are not implemented in the store. M2's
  `hit-store` has to, and the `tags` table is already shaped for it.
- **Scale.** Negentropy storage is rebuilt from SQL on every `NEG-OPEN`:
  `O(matching events)` per sync. Trivial at 2000; measure before 100 k per
  group, or cache per-group.
- **`nostr-lmdb` was not built.** The store decision is "SQLite is
  sufficient and keeps compile-checked SQL", not "SQLite beat LMDB".
- **Compile-checked SQL.** The spike uses `sqlx::query` (no `DATABASE_URL`).
  The product macros are a build-setup question, not a spike one.
- Concurrency (many peers at once), mid-sync disconnects, large events.

## Gotchas this cost time

- `nostr 0.45`: key generation needs the **`os-rng`** feature; signing is
  `EventBuilder::finalize(&keys)` via the `FinalizeEvent` trait (not
  `sign_with_keys`); types live in `nostr::prelude`, not the crate root;
  tag letters are `SingleLetterTag::LOWERCASE_H`.
- `negentropy 0.5`: `Id::new` is deprecated for `Id::from_byte_array`.
  `reconcile_with_ids` returns `None` when it is done. Frame limit is 0 or
  ≥ 4096; 128 KiB leaves headroom, since hex doubles it under our 1 MiB cap.
- `sqlx 0.9`: `QueryBuilder` has no lifetime parameter, and `sqlx::query`
  refuses a `&&str` as "dynamic SQL" — deref it.
- iroh streams: a clean end between frames is
  `ReadExactError::FinishedEarly(0)`. Graceful close is: client
  `finish()`, read until the relay finishes, then `conn.close`.

## What M2 inherits

1. `hit/nostr/1` framing, session loop and sync as written here are the
   skeleton of `hit-proto` + `hit-node`; the binding frame goes *before*
   the first `REQ`/`NEG-OPEN`.
2. Serve a `NEG-OPEN` only for a group the asker is in, and only for the
   channels they may view — `hit-group` decides, per `DESIGN.md` §4.
3. `hit-store` = this schema + replaceable/addressable/deletion semantics.
4. Persist the iroh secret key: here every run is a new `EndpointId`.
