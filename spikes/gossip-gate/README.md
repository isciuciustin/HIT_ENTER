# S2 — Gated gossip

**Question (`DESIGN.md` §15 q2):** can an `iroh-gossip` topic refuse a peer
whose `EndpointId` is not a member — so a private channel can be live over
gossip without a non-member reading it?

**Verdict (2026-10-10): yes, with the gate at the ALPN, not the topic.** 24
checks held on the final run, on loopback with no relay. The mechanism is
ours (a wrapping `ProtocolHandler`); `iroh-gossip` has no membership feature.

```bash
eval "$(mise env -s bash)"
cd spikes/gossip-gate && cargo build --release
./target/release/gossip-gate-spike            # everything, ~2.5 min
./target/release/gossip-gate-spike t2         # one section: t0 t1 t2 t3 t4
./target/release/gossip-gate-spike t2 wait-60 # one T2 variant (60 s)
```

Own lockfile, not a workspace member. **`iroh-gossip 0.101` resolves only
against iroh 1.3.0** — 0.99 wants `iroh =1.0.0-rc.0`, and nothing in 0.95–0.100
matches 1.1.0. HIT_ENTER_2's transport is on 1.1.0: the product must move to
iroh ≥ 1.3 to use gossip (S1's first build already uses 1.3).

## The mechanism

```rust
impl ProtocolHandler for Gate {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();               // from the connection
        if !self.members.contains(&peer) { conn.close(403u32.into(), b"not a member"); return Ok(()) }
        self.gossip.handle_connection(conn).await …
    }
}
```

`Gossip::builder().alpn(..)` gives each gated set its own instance and ALPN;
the `Router` maps ALPN → gate.

## Results

| | |
|---|---|
| **T0** ungated: a stranger told one member's id + the topic | joins and reads — the problem is real |
| **T1** gated: same stranger | never joins, receives nothing; members (3) swarm and deliver normally; 2 refusals logged |
| **T2** refused, then admitted | see below |
| **T3** revoke a connected member | list-only removal: **still receives**. Remove + close live connections: receives nothing, cannot rejoin |
| **T4a** one ALPN, allow-list `{A,B,X}`, two groups | X (not in group 2) **joins group 2's topic and reads it** |
| **T4b** one ALPN per group | X joins group 1, receives group 1, receives nothing from group 2 |

**T2 — after a refusal, what makes the peer join?** (N's gate admits A in
all of these; see the bug note.)

| variant | joined |
|---|---|
| N waits on its original subscription (20 s) | ✗ |
| N waits 60 s | ✗ |
| N subscribes again | ✗ |
| N drops the subscription, then subscribes | ✗ |
| N calls `join_peers([A])` | ✗ |
| **a member** calls `join_peers([N])` | ✓ instantly (delivery of the very next broadcast: 1 of 2 runs — best-effort, see below) |
| N restarts its endpoint + gossip instance (same key) | ✓ instantly, received 3 of 3 |

Cause not isolated: the refused peer's gossip actor does not retry on its own.

## What it means for the design

1. **Gating unit = ALPN.** One gossip instance per set of peers who may read.
   Group → its members; private channel → its viewers (`hit-group` computes
   both). Per-topic gating on a shared ALPN is **not** possible.
2. **Revocation closes connections.** Keep clones of accepted `Connection`s.
3. **Don't subscribe until a member** (the app knows membership from
   `hit/nostr/1` first). On admission, a member dials the newcomer or the
   newcomer builds a fresh instance.
4. **Every member runs the same gate** — links are bidirectional, both ends
   must admit.
5. **Gossip stays best-effort** — negentropy repairs what it drops.

## Not tested — the verdict does not cover these

- **Any real network.** Loopback, `RelayMode::Disabled`, an in-memory
  address lookup. Hole punching and relays are untouched.
- **A member who introduces a stranger.** Gossip's membership messages carry
  peer ids and a member's actor dials whoever it is told about. The gate
  stops strangers *connecting in*; it cannot stop a member from handing a
  stranger the swarm. Same for a compromised member (§6's problem).
- **What a refused peer learns.** The QUIC handshake completes before the
  close, so N learns A's `EndpointId` is online at an address. No content.
- **Cost at scale.** One actor and ~one connection per (peer, gated set),
  3 accepted connections for 2 groups × 2 peers. Not measured for 20 groups
  × 50 peers, nor on a phone's battery.
- **Churn.** Many revocations, flapping members, many topics per instance.
- **Per-message authenticity.** Gossip payloads here are bare bytes. In the
  product they are signed Nostr events, verified before use (S1).
- The time-based checks use 3 s windows; a very slow machine could flip a
  negative ("receives nothing") only in the safe direction, a positive could
  flake (as `a-dials-n`'s delivery did once).

## Harness bugs worth knowing (the mistakes were mine)

- N's own gate must admit A. With an empty allow-list N refuses A's dials and
  every T2 variant failed — **that was the first, wrong, "result"**. Fixed
  by admitting A; the N-side retry failures persisted, so they are real.
- A `match` arm missing for `wait-60` ran the restart logic instead and
  reported a false success. Caught because identical code gave different
  answers; fixed and re-run.
- The runtime prints `connection task panicked: JoinError::Cancelled` from
  `iroh-gossip` on exit — a task aborted at shutdown, harmless.
