# S5 — Two endpoints, one app

**Question (`DESIGN.md` §7, §15 q5):** can one process run a direct iroh
endpoint and a relay-only one (`clear_ip_transports()`) side by side — a
stranger in a public group never learns your IP, a friend in a DM gets a
direct path — and what does the second endpoint cost? Does anything in iroh
assume one endpoint per process?

**Verdict (2026-10-10): yes, and it is cheap — provided each endpoint has
its own key.** 17 checks held on loopback, plus one on the real network.
Nothing in iroh 1.3 assumes one endpoint per process. The second endpoint
costs about **0.3–0.5 MB, two sockets and no threads**. **One key on two
endpoints does not work:** the relay routes every dial to the newest one,
and the older goes dark without an error.

```bash
eval "$(mise env -s bash)"
cd spikes/two-endpoints && cargo build --release
./target/release/two-endpoints-spike                # T1 split, T2 same-key, T4 independent (~40 s)
./target/release/two-endpoints-spike cost pair 8    # T3: direct | relay-only | pair (~45 s each)
./target/release/two-endpoints-spike real-net       # T5: n0's relays; prints counts, never addresses
```

Own lockfile, not a workspace member, iroh `=1.3.0` (the version gossip
needs, S2). The loopback sections run the relay in a **child process**
(`serve-relay`), so the relay's memory never counts toward T3.

## Results

**T1 — split.** D (direct) and R (relay-only), separate keys, one process,
one tokio runtime. P is a stranger with a default endpoint. Every dial uses
`EndpointId` + relay URL only, which is what an invite carries.

| | P's paths | P learns our IP |
|---|---|---|
| own `EndpointAddr` | D: 3 IPs · R: **0** | — |
| P dials D | 1 relay, 1 IP | **yes** (the baseline: the leak is real and visible) |
| P dials R | 1 relay, **0 IP** | no |
| R dials P | 1 relay, **0 IP** | no |

**T2 — one key, two endpoints.** E1 (direct) bound first, then E2
(relay-only), with the same `SecretKey`. Ten dials from fresh strangers, 2 s
apart:

```
E2 E2 E2 E2 E2 E2 E2 E2 E2 E2      → after E2 closes: E1
```

That is not "taking turns", as HIT_ENTER_2 guessed from reading
`iroh-relay`'s `clients.rs` (`spikes/relay-path` never ran it). **The newest
registration owns the id at the relay, and the older endpoint is unreachable
by relay for as long as the newer one lives.** It gets no error. Closing the
newer one promotes the older one again. So **two endpoints mean two keys and
two bindings**, which is what `DESIGN.md` §7 already said ("each with its own
binding"). The test shows why that has to be so.

**T3 — cost** (desktop PC, Ryzen 5 5600, 12 threads, loopback, 2 s settle per step, RSS after
idling; the first endpoint carries a one-time ~10–16 MB of library, TLS and
allocator setup that any app pays once):

| kind | 1st | each further | sockets each | threads | bind + online | idle CPU, 20 s |
|---|---|---|---|---|---|---|
| direct | +16.1 MB | ~1.0–2.4 MB | 4 | +1 total, then 0 | 245 ms | 0.05 % of a core (8 eps) |
| relay-only | +11.1 MB | **~0.1–0.5 MB** (one 1.1 MB outlier) | **2** | +1 total, then 0 | 245 ms | 0.10 % (8 eps) |
| pair (the product) | +13.4 MB | ~1–2 MB per pair after the 3rd | 6 | +1 total, then 0 | 490 ms | 0.05 % (16 eps) |

The arithmetic for the app: one direct + one relay-only endpoint is the
first endpoint's fixed cost plus **well under 1 MB** for the second. Even
eight pairs stayed under 35 MB RSS. Every endpoint runs on the shared tokio
runtime; none starts its own threads.

**T4 — independence.** Closing D leaves R's open connection and R's new
dials working. Rebinding D with the same key (what a network change or a
"private mode" toggle does) answers again, and R does not notice. Closing R
leaves D working. 6/6.

**T5 — real network** (n0's relays, the real gateway, default port mapper,
15 s after online):

| | relay | IP addresses in its own `EndpointAddr` |
|---|---|---|
| direct | 1 | 3, of them 2 public |
| relay-only | 1 | **0** |

This was the gap in HIT_ENTER_2's loopback-only result. A relay-only
endpoint still calls `port_mapper.procure_mapping()`, and
`update_direct_addresses` adds port-mapped and QAD-observed addresses
regardless of transport, so by the code alone it could have advertised a
public IP. In practice it does not: with no IP socket the port mapper never
gets a local port (`get_mapping` is a no-op), and QAD has no socket to probe
from. Read in `iroh-1.3.0/src/socket.rs` (~l. 790, 943, 1810) and
`portmapper-0.19.3`, then shown on the real network.

## What it means for the design

1. **Two endpoints, two device keys, two bindings.** The binding names an
   `EndpointId`, so each endpoint needs its own (M1's `verify_binding`
   already works this way). A person's npub signs both. Peers who see both
   bindings know they are the same person, which is fine: the IP is what we
   hide, not the person.
2. **Each group lives on exactly one endpoint**, decided when you join, and
   everything for that group (its `hit/nostr/1`, its gossip ALPN from S2,
   blobs, voice) goes over that endpoint. Mixing would hand the IP to the
   peers it was meant to hide from.
3. **Cost is not an argument against it.** The second endpoint is under a
   megabyte and no threads. The cost that is left is complexity: two
   bindings to publish, revoke and rotate, plus a per-group choice in the UI.
   M4 decides on that basis, not on memory.
4. **Relay-only traffic is relay bandwidth**, ours or n0's
   (HIT_ENTER_2 `OPERATIONS.md` §6). Public groups being relay-only makes
   them the relay bill.
5. **Never reuse a key across two live endpoints.** It fails silently (T2).
   Rebinding one endpoint with its own key (T4) is fine.

## Not tested — the verdict does not cover these

- **A two-machine run on a real network.** T1's path results are loopback.
  T5 checks only the relay-only endpoint's *own* advertised address, not
  what a remote peer actually sees on a connection across NATs. The laptop ↔
  PC run (riding along with S1's) should repeat T1 with P on the other
  machine.
- **The peer's candidates still arrive.** HIT_ENTER_2 noted that a
  relay-only side still receives the *other* peer's candidates over the
  relay. That protects the person who is relay-only, not their peers. Not
  re-checked here.
- **The relay sees both.** Both endpoints keep a connection to the same
  relay from the same IP, so a relay operator can link the two
  `EndpointId`s. Inherent, not measured.
- **Idle cost on a real network.** T3's idle CPU is loopback. Real relays
  mean pings and periodic net_report probes per endpoint; not measured.
  Neither was battery.
- **macOS, Windows, a Tauri process.** Linux only, plain binary.
- **Gossip and blobs on both endpoints at once.** The protocols were a toy
  echo ALPN. S2's gossip was gated on one endpoint.
