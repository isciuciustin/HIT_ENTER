# Spike S1 — Nostr over iroh

**The question** (`docs/PLAN.md` M0, S1): can a Nostr relay run over an iroh
connection — NIP-01 `EVENT`/`REQ`/`EOSE` on its own ALPN, with NIP-77
negentropy syncing two stores after one was offline — and is "our own SQLite"
a good enough store to do it with?

Throwaway crate: its own `[workspace]`, `publish = false`. Not a product crate.

## What it is

```
src/
├── main.rs     CLI: serve | publish | send | req | sync | count
├── node.rs     iroh endpoint, ALPN "hit/nostr/1", device key in <data>/iroh.key
├── frame.rs    u32-BE length + JSON, 1 MiB cap
├── relay.rs    serves EVENT / REQ / CLOSE and NEG-OPEN / NEG-MSG / NEG-CLOSE
├── client.rs   dials; REQ, EVENT→OK, and the negentropy sync (fetch + push)
└── store.rs    sqlx + SQLite; verify-before-insert; runtime queries
```

Dependencies, current as of 2026-10-10: `iroh` 1.3.0, `nostr` 0.45.5,
`negentropy` 0.5.1, `sqlx` 0.8 (SQLite).

**Deliberately not here:** no `AUTH`, no binding event, no groups, no gossip,
no membership check. Any peer that dials may read and write. In the product
the first frame is the peer's binding, checked against the connection's
`EndpointId` (`DESIGN.md` §4) — that is M1/M3, not this spike. The key for
signing is a throwaway `Keys::generate()` per `publish`; the iroh key is a
plain file. **Neither is how the product holds keys.**

## Run it

```bash
cargo build                                   # from this directory
B=./target/debug/nostr-iroh

$B --data /tmp/a serve                        # prints "node id: …"; leave running
$B --data /tmp/b publish "hello" --count 5    # five signed notes, local only
$B --data /tmp/b sync --peer <NODE_ID>        # negentropy, both directions
$B --data /tmp/c req --peer <NODE_ID> --kind 1 --limit 3
$B --data /tmp/c send --peer <NODE_ID> "ok note"
$B --data /tmp/c send --peer <NODE_ID> "forged" --tamper   # must be refused
$B --data /tmp/a count
```

`serve` keeps its node id across restarts (`<data>/iroh.key`).

## Results, same laptop, two processes (2026-10-10)

| check | result |
|---|---|
| NIP-01 over iroh (`REQ` → `EVENT`s → `EOSE`, `EVENT` → `OK`) | ✅ works first time; `req --limit 3` returned 3, 1149 B in |
| forged event (content edited after signing) | ✅ `OK false "invalid: Invalid event ID"`; **not stored**, relay logged it |
| first sync, 5 events | ✅ 1 negentropy round, 5 pushed, **0.48 s** total (0.34 s of it dialling) |
| bulk, 5,020 events pushed | ✅ 6.0 s total, 1.85 MB out — ~1.2 ms per event, one `OK` round trip each |
| **gap sync: 5,036 events each side, 30 differ** | ✅ **2 rounds, 10 fetched + 20 pushed, 18 KB total, 0.51 s** |
| sync of identical sets (5,066 events) | ✅ **1 round, 751 B total**, nothing moved |
| node id survives a restart | ✅ |

**Reading it.** The cost follows the *difference*, not the history: 30 missing
events out of 5,036 cost 18 KB, of which ~11 KB is the events themselves;
re-syncing identical sets is under a kilobyte and one round trip. That is
exactly the property `DESIGN.md` §4 leans on ("negentropy for truth").

**Store: "our own SQLite behind sqlx" is fine for this job.** A 5,066-event
store is 2.6 MB on disk. The spike's filter matching loads every row and
matches in Rust, which is O(n) per `REQ`/`NEG-OPEN` — fine at 5 k events,
not something to ship (M2 needs SQL-side filtering on kind/author/time and
an index). Nothing about the choice itself pushed back.

## What this does not prove yet

- **Across two machines.** Laptop ↔ gaming PC on the LAN is the next run
  (below); a NAT-crossing run is blocked by the owner's connection, as in S3.
- **Offline for long, or both sides large.** The gap test had one side
  offline for a few seconds and 30 events. A week of churn is a different
  sync.
- **Bulk push is one `OK` per event.** Fine on a LAN (1.2 ms); over a 100 ms
  link 5,000 events would be ~8 minutes. M2 should pipeline pushes (send N,
  then read N `OK`s) or push in a batched frame.
- **No live subscription.** `REQ` here is one-shot: stored events then
  `EOSE`, and the stream stays quiet. Live fan-out is gossip's job (S2).
- **Relay session errors are noisy:** the serving side logs
  `closed by peer: bye (code 0)` as a WARN when a client hangs up normally.
  Treat `code 0` as clean in the product.

## Next run: laptop ↔ gaming PC

On the PC, after `git pull`: `cd spikes/nostr-iroh && cargo build`, then
`serve`; on the laptop `publish` some events and `sync --peer <PC_NODE_ID>`,
then the other way round. Record whether the path was direct or relayed
(`RUST_LOG=iroh=info`) and the dial time.

## Verdict

*Provisional — one machine only.* Nostr's relay protocol runs over iroh
without changes, negentropy syncs by difference, and SQLite is a sound store.
Nothing so far changes `DESIGN.md` §4. Final verdict after the two-machine
run.
