# HIT_ENTER 1.0 — Execution plan

The architecture is in [`DESIGN.md`](DESIGN.md). This file is the order of
work.

**Target, v1.0:** an open-source (MIT), Discord-shaped app where you and a
friend in another city make a group, talk in channels, DM, send files, sit in
a voice channel, and zap each other's messages — **with no server either of
you had to rent, and no public Nostr relay in the loop.**

**Beyond v1.0, the north star (2026-10-22):** the next Discord, for public
communities — the Unturned Discord running on HIT_ENTER (`DESIGN.md` §0).
**§0 below is what happens now.**

Every milestone ends in something you can run and show someone. No milestone
is "refactoring", and no milestone is "and then the rest of it".

### About the sizes

HIT_ENTER_2's plan estimated 28–40 weekends; its M0 to M9 landed between
2026-09-19 and 2026-09-23. So the sizes below are **relative weights**, not
calendar promises: an **S** is about M1 of HIT_ENTER_2, an **L** is about its
M7 (attachments). What they are good for is noticing when a milestone is
twice its size — and splitting it rather than pushing through.

---

## 0. Now — until the VPS (2026-10-22 → November 2026)

**2026-10-24: the VPS came early** — OVH, Canada, `wss://anchor.hitenter.app`
now runs there, one group pinned (`SESSION_LOG.md`). The VPS-day items are
ticked; the rest of the month stays as written.

The owner rents a VPS next month; it becomes the first always-on anchor.
Until then, in this order. *You* = the owner, *me* = Claude.

### Week 1 (Oct 23–31) — prove it works for real

- [ ] **Laptop ↔ PC across two networks** (one on a phone hotspot): a
  group, messages, a file, a call — the first real network crossing
  (`TESTING.md` §2). *You + me.*
- [ ] **A real session with the friend in another city**: a group, chat,
  a file, a ten-minute call (`TESTING.md` §4, §6) — what v0.1.0 waits for.
  *You + friend.*
- [ ] Fix whatever those break. *Me.*
- [x] **The zap dialog's ~4 s give-up** on the wallet path — it waits for
  the carrier now (2026-10-24; fake network, not yet real sats). *Me.*
- [ ] **Announce on Nostr** that HIT_ENTER is open source: GitHub link,
  hitenter.app. *You.*

### Weeks 2–3 (Nov 1–14) — public groups

- [x] **The owner answers** (2026-10-24 — posts at once; profiles to
  public relays, yes; DM inbox on, 7 days, 10 MB a person; M13). Asked:
  1. In a public group, can a newcomer post at once, or read until a
     moderator lets them in?
  2. Publish profiles to public Nostr relays by default? (Recommended:
     yes, for public communities.)
  3. Anchors keep your DMs while you are offline (the DM inbox) — on by
     default?
- [x] **Write the "public groups" milestone** — M13 (2026-10-24). Then
  build it: a
  **public** setting on a group, **join from a link without an invite**,
  **read before joining** (`DESIGN.md` §0, rows 1 and 3). *Me.*
- [ ] **Public groups announced on Nostr relays**, and a first, plain
  **"browse communities"** list in the app. *Me.*
- [x] ~~**Rehearse the VPS in Docker**~~ — done on the real VPS instead
  (2026-10-24). Was: **Rehearse the VPS in Docker**: `hit-anchor` under systemd, Caddy
  for `wss://`, exactly as `SELF_HOSTING.md` says — so VPS day is copy and
  paste, not debugging. *Me.*

### Week 4 (Nov 15–22) — release, and get ready

- [ ] **beta.5** with public groups and the fixes: Linux built here, macOS
  and Windows by the owner; hitenter.app links and sha256 lines. *Me + you.*
- [x] **The VPS-day checklist** — done live, 2026-10-24 (`SESSION_LOG.md`).
  Was: **The VPS-day checklist**: which server (Hetzner-class, ~€4–5/month),
  the DNS move of `anchor.hitenter.app` from the Cloudflare tunnel to the
  VPS, install, backups, and the check that it works. *Me.*
- [x] Pick the provider; a card ready. *You.* — OVH VPS-1, 2026-10-24.
- [ ] **Pin the other groups** to the VPS anchor; the owner's check that a
  message sent while they were offline arrives through it. *You + me.*
- [x] **The 23 refused events** — found and fixed (2026-10-24, second
  session): expired invites were never synced, so a late anchor could not
  apply four joins. Refusals now log kind, id and author. Re-offering is
  kept, deliberately (`SESSION_LOG.md`). Live once the anchor and the app
  both run the fix — or at once, if the owner compacts the group. *Me.*

### VPS day

The owner rents the box and points the domain; the checklist does the
rest. The group stays alive with nobody online, zaps get a permanent
`wss://`, and other Nostr apps can see public communities.

**Deliberately after the VPS:** code signing (costs money), Windows echo
cancellation, moving HIT_ENTER_2's billing crates (M11), voice over 8.

---

## 1. What carries over, and what dies

**This is the first HIT_ENTER that is not a fork of the last one.** The
authority model is different (every member is a replica, not a client of one
host), so the server crates do not carry over as a whole. Many of their
*lessons* and several *pieces* do:

| from | into | how |
|---|---|---|
| HIT_ENTER `he-proto` framing, limits, the invite ticket idea | `hit-proto` | as-is (MIT/Apache permits it; keep its notice) |
| HIT_ENTER_2 `he2-space` `roles.rs` + the 49 refusal tests | `hit-group` | **rewritten as a pure function** over group state instead of SQL |
| HIT_ENTER_2 attachments (M7): pull-only uploads, `execute_get` not `fetch`, gated serving, one writer at a time | `hit-files` | the code *and* its tests; the rules as-is |
| HIT_ENTER_2 `he2-client` reconnect (`spawn_retrying`), disk-before-screen | `hit-node`, `src-tauri` | as-is in spirit, re-plumbed |
| HIT_ENTER_2 `src-tauri` + `web/` (Dialog focus trap, `focusOnMount`, `copyText`, `AttachmentView`, the WebKitGTK fixes) | `src-tauri`, `web/` | largely as-is |
| HIT_ENTER_2 `he2-grant`, `he2-control`, `he2-edge` (M9) | hosted anchors (M11) | reused as-is — same owner, same licence |
| HIT_ENTER_2 `docs/TESTING.md` §3 (two windows, and "with the network off") | `docs/TESTING.md` | as-is — it found every real bug M4 had |
| HIT_ENTER_2 `he2-id`, `he2-idp`, `he2-idpd`, email codes, Argon2id | — | **gone.** Nostr keys replace the identity plane |
| HIT_ENTER_2 `he2-space` as the authority, `he2-hostd` | — | **gone.** Replaced by replicated signed state (`DESIGN.md` §6) |

**The licence is decided: MIT** (2026-10-22, `DESIGN.md` §15 q1; closed
source until then). HIT_ENTER_2's code may move here — under MIT, once it
does — record each piece in `docs/SESSION_LOG.md` as it moves. The original HIT_ENTER's code may move
too, with its MIT/Apache notice kept in `THIRD_PARTY_NOTICES.md`.

---

## 2. Rules carried forward

Not up for renegotiation during a milestone (`DESIGN.md` §14):

- Read the peer's `EndpointId` off the connection, never from a message.
- Verify every event before storing, forwarding or showing it.
- Every authorization decision goes through `hit-group`.
- Three keys — person, group, device — never conflated.
- The nsec lives in the OS keychain and is never logged.
- Plaintext or a real E2EE design; nothing in between.
- No blob served unauthenticated; no `Remote::fetch` on an upload.
- No HTTP or WebSocket in the chat plane.
- **HIT_ENTER never holds user funds.**
- The mirror is written before the UI is told.
- `clippy -D warnings`, no `unwrap()` outside tests and `main()`,
  append-only migrations, SQL checked at compile time.

---

## 3. Milestones

```
M0 spikes ─ M1 identity ─ M2 two relays ─ M3 a group ─ M4 an app (v0.1.0)
                                                          │
      ┌───────────────┬──────────────┬────────────┬───────┴──────┐
   M5 moderation   M6 anchors ─ M7 DMs   M8 files ─ M9 voice ─ M10 zaps
                                                                  │
                                                  v1.0 ── M11 hosted anchors
```

M5 through M10 depend on M4 and mostly not on each other; the order below
is the recommended one, not the only one. M7 needs M6 (an offline DM needs
somewhere to wait). M10 needs M6 (a zap receipt needs a `wss://` door).

---

### M0 — Five spikes, before a line of product code
**Size: M**

Every one of these can change the design, so every one happens first.
Each is a throwaway crate under `spikes/`, outside the workspace, with a
README that ends in a verdict — the shape HIT_ENTER_2's `spikes/relay-path`
and `spikes/blobs` already set.

- **S1 — Nostr over iroh.** Two processes, each with an embedded event
  store, speaking NIP-01 `EVENT`/`REQ`/`EOSE` over an iroh ALPN, then syncing
  with NIP-77 negentropy after one was offline. Decide the store: `rust-nostr`'s
  `nostr-relay-builder` + `nostr-lmdb`, or our own SQLite behind `sqlx`
  (keeps the compile-checked-SQL discipline, and a chat's queries are narrow).
  **Recommendation going in:** use `rust-nostr`'s `nostr` crate for events
  and signing, `negentropy` for sync, and our own SQLite for storage.
- **S2 — Gated gossip.** Can an `iroh-gossip` topic refuse a peer whose
  `EndpointId` is not a member? (`DESIGN.md` §15 q2.)
- **S3 — Voice between two cities.** Build n0's `callme` and call your
  friend — the same test that convinced you about HIT_ENTER. Measure mouth-
  to-ear latency direct and via relay. **Use laptop speakers once**, to hear
  what echo cancellation does and does not fix. Then check `cpal` works from
  inside a Tauri process on Linux (PipeWire) and Windows.
- **S4 — One real zap.** A throwaway CLI: fetch a Lightning address's LNURL,
  build and sign a 9734, pay 21 sats through a NWC connection to your own
  wallet, fetch the 9735 back from the relay, validate it. Real mainnet,
  real sats, your own two wallets. Proves every hop of `DESIGN.md` §11.
- **S5 — Two endpoints, one app.** Two iroh endpoints in one process, one
  relay-only. What does it cost in memory, and does anything in iroh assume
  one per process? (`DESIGN.md` §15 q5.)

Also in M0: the workspace skeleton (every crate `publish = false`),
`about.toml` with GPL-family licences off the accepted list, and the first
`THIRD_PARTY_NOTICES.md`.

**Done when:** five READMEs end in a verdict, `DESIGN.md` is corrected
wherever a spike disagreed with it, and `cargo about` passes.

**Progress (2026-10-10): S3 is parked with a provisional verdict.** `callme`
builds and calls on Linux and macOS; Linux ↔ Linux on one LAN with headsets
was clear, direct and low-latency. **Not proven:** echo cancellation on
speakers (two people needed), a busy machine, and any call across networks
(the owner's connection is too slow to test from where they are). What M9
inherits is listed in `spikes/voice/README.md`.

**Progress (2026-10-10): S1 is done — yes.** Two nodes, NIP-01 + NIP-77
over `hit/nostr/1`, converge after one was offline; 2000 events reconcile in
2.3 KB; our own SQLite behind `sqlx` is the store. Verdict, and the long
list of what it did *not* test (any real network, authorization, interop,
replaceable-event semantics, scale), in `spikes/nostr-iroh-b/README.md`.
**Progress (2026-10-10): S2 is done — yes, by ALPN.** A gossip topic can be
gated by `EndpointId` with a wrapping `ProtocolHandler`, but the unit is the
ALPN: one gossip instance per set of readers, so a private channel gets its
own. A list is not revocation (close the live connections), and
`iroh-gossip` needs iroh ≥ 1.3 (HIT_ENTER_2 is on 1.1). `DESIGN.md` §4 and
§15 q2 are corrected. Verdict and limits: `spikes/gossip-gate/README.md`.
**Progress (2026-10-10): S5 is done — yes, with separate keys.** A direct
and a relay-only endpoint run side by side in one process; the relay-only
one advertises no IP (also on the real network), and a stranger holds no IP
path to it. The second endpoint costs under 1 MB, two sockets and no
threads. One key on two endpoints fails silently (the newest registration
wins at the relay). `DESIGN.md` §7 and §15 q5 are updated. Verdict and
limits: `spikes/two-endpoints/README.md`. **S4 not started** — it needs the
owner's two wallets and an NWC string.
**Progress (2026-10-10): the skeleton is done.** The workspace has the six
crates M1–M3 need, all `publish = false` with no `license` key, and
`unwrap_used`/`expect_used` are denied. `about.toml` accepts MPL-2.0, for
one crate under iroh's port mapper, and nothing GPL-family.
`THIRD_PARTY_NOTICES.md` is generated. The workspace has no external
dependencies yet, so the spikes' trees were checked against the same
`about.toml` and pass. **M0 is done except S4**, plus the real-network runs
in the spikes' "not tested" lists.

---

### M1 — A person on a device, with no network
**Size: S**

`hit-event`: our kinds, tag builders, validation — on top of the `nostr`
crate, no I/O. Key generation and import (`nsec`, NIP-49 `ncryptsec`),
the OS keychain (`keyring`), the device binding event, and
`verify_binding(event, endpoint_id)` with no signature-only entry point.
Revocation by replacement. Expiry and re-signing.

**Done when:** a test makes a person, binds two devices, verifies each
binding against its own `EndpointId` and **rejects it against the other**,
revokes one, and rejects it after — and a test greps the keychain-backed
store path to prove no nsec is ever written to a plain file.

**Done (2026-10-10).** Both are `crates/hit-event/tests/m1_done_when.rs`,
with 16 unit tests beside them. The second scans the bytes of everything a
node writes to disk or to a log, for the nsec in bech32, hex and raw form.
A planted control shows the scan can see a leak. A real OS-keychain round
trip (gnome-keyring here) is an `#[ignore]`d test, and it passes. **Beyond
the plan:** the binding is countersigned by the device (a `proof` tag), the
`d` tag must be canonical hex, and a revocation wins a same-second tie.
Kind 30436 is provisional. All of it is in `docs/PROTOCOL.md`.

---

### M2 — Two apps are two relays
**Size: L — the milestone that proves the idea**

`hit-proto` (framing, ALPN), `hit-store` (the event store and filter
queries), `hit-node` (the endpoint, the `hit/nostr/1` handler, binding-first
authentication, negentropy sync, reconnect), and `hit-cli` to drive it —
there is still no curl for a QUIC protocol.

**Done when**, run by hand between your machine and your friend's:
two CLIs exchange signed events; one is killed, the other writes twenty
more, the first comes back and negentropy fetches exactly those twenty and no
duplicates; a third process presenting a copied binding is refused. This is
the emotional halfway point, as M2 was for HIT_ENTER_2: after it, every
remaining problem is one somebody has solved before.

**Progress (2026-10-10): done on one machine, not yet between two.** The
whole sequence passes as an automated test
(`crates/hit-node/tests/m2_done_when.rs`). It also passes by hand with three
real processes on the desktop and A killed with `kill -9`
(`scripts/m2-by-hand.sh`): A came back and fetched exactly 20, with 0
duplicates, in one negentropy round; C's copied binding and C's own
unlisted binding were both refused. **Still owed:** the run "between your
machine and your friend's", over n0's relays across NATs (`docs/TESTING.md`
§2). Until then M2 is proven on loopback only.

---

### M3 — A group, with nobody in charge
**Size: L**

`hit-group`: the group key, `9007` create, channels in `39000`, `9009`
invites, `9021` joins, `9000`/`9001` membership, the state fold (highest-`seq`
snapshot plus newer valid actions), and the roles module ported from
HIT_ENTER_2 as a pure function. Replication filtered by membership and
`VIEW_CHANNEL`, in all three places. Live delivery over gossip (or S2's
fallback). Invite links.

**Done when:** three peers in a group; **the owner quits**; the other two
keep talking, and a fourth person joins from an invite with the owner still
gone; the owner comes back and sees everything. And a non-member who dials a
member and asks for the group's events gets nothing.

**Progress (2026-10-11): done on one machine, not yet between two.** Both
halves pass as an automated test (`crates/hit-node/tests/m3_done_when.rs`,
with messages arriving live over gossip and no sync) and by hand with five
real processes, the owner killed with `kill -9` (`scripts/m3-by-hand.sh`):
B and C kept talking, D joined from **B's** invite with the owner gone, the
owner came back to all four messages and D in the member list, and E — in
no group — was served 0 events by `REQ` and 0 by negentropy. A second test
kicks a member and checks it stops hearing the group's gossip. Compaction
(seq 0 → 1) loses nothing. What M3 does *not* have, by plan: admins'
`9002`/`9005`, bans, mute, slowmode (M5); a private channel's own gossip
instance (its messages go by sync until then). **Still owed:** the same
across real networks, with M2's two-machine run.

---

### M4 — It is an app again → tag v0.1.0
**Size: L — split it the moment it drags**

The Tauri shell and Svelte frontend from HIT_ENTER_2, re-plumbed onto
`hit-node`: onboarding (make or import a key, **the backup step you cannot
skip**), your profile (kind 0), a group list, channels, messages, reactions,
a paged member list, presence, invite links, offline and reconnect. The
"your IP is visible to this group" badge, or S5's two endpoints.

**Done when:** the whole thing through the GUI with a real person in another
city — HIT_ENTER_2 M4's recipe: two windows, a group, a conversation; then
**the network off**: reopen the app with no network, history from disk, a
message queued, the network back, delivered by itself. That half is where
M4's only two real bugs lived last time. Tag **v0.1.0**.

**Progress (2026-10-11): split, and M4a done on one machine.** Split as
the size line says. **M4a** is the app you can talk in: `src-tauri`
(crate `hit-app`, binary `hit-enter`; the shell — single instance per data
directory, `hit-enter:` links, the WebKitGTK workaround — from HIT_ENTER_2)
over `hit-node`, and a new, smaller Svelte store in `web/` with
HIT_ENTER_2's look. It has onboarding (make, or import an `nsec` /
`ncryptsec`; the backup step — an `ncryptsec` file or the key shown once
and its last 8 characters typed back — cannot be skipped, and survives a
quit), a name (kind 0, now carried between people who share a group), the
group rail, channels, messages live and from disk, a member list with
roles and "reached lately", invite links (copied), joining (pasted, or a
desktop link that opens the dialog), the "your IP is visible" badge, and
offline: what you write is stored at once and goes out with the next sync
that reaches anyone. Automated: `hit-app`'s three tests (the backup gate;
a backup file round-trip and import; two apps, one away, delivered by
itself) and `hit-node/tests/profile.rs`. By hand: `docs/TESTING.md` §3,
two windows driven from the keyboard — it found three defects, all fixed.
**M4b**, next: reactions, presence proper, a paged member list, S5's
second endpoint (or keeping the badge), unread counts that survive a
restart, and the frontend's own licence notices (`cargo about` sees only
Rust: Svelte, Tauri's JS and the JetBrains Mono font — OFL-1.1 — need theirs
before a release). **Still owed for the tag:** §4, with a real person in
another city.

**Progress (2026-10-11): M4b done on one machine.** Reactions (NIP-25,
kind 7) and withdrawing your own message or reaction (NIP-09, kind 5 —
applied only to the author's own `9` and `7`, and a withdrawn event cannot
be pushed back by a peer that still holds it); presence (kind 20436, ours:
ephemeral, on a group's gossip only, every 30 s, 120 s maximum age);
online-first member list in pages of 100; unread counts kept in
`read.json` so they survive a restart; `THIRD_PARTY_NOTICES_WEB.md` from
`scripts/web-notices.mjs` (what is actually in the bundle: Svelte, Tauri's
JS, tailwind's CSS, JetBrains Mono under OFL-1.1), both notices files
bundled with the app. **S5's second endpoint is deferred, and the badge
stays**: a relay-only endpoint per group means a second device key and a
second binding per group, and touches every path M2–M3 built — its own
piece of work, not a line item of M4. Keyboard: messages are reachable
(roving tabindex) and every action on them is too. **Still owed for the
tag:** `TESTING.md` §4.

---

### M5 — Moderation without a server
**Size: M**

Admin actions (`9000`/`9001`/`9005`/`9002`) with the rank rule, bans,
moderator deletes, mute and slowmode, compaction by a group-key holder, and
the moderator UI saying plainly what a ban can and cannot do
(`DESIGN.md` §6). HIT_ENTER_2's refusal tests, plus the new ones only a mesh
has: two equal-rank admins banning each other at once; a ban arriving after
the banned user's next message; a snapshot racing an action.

**Done when:** a banned user's patched client (the CLI, lying) posts into
the group and **no honest peer stores or shows it**, and the adversarial
test file is at least as long as HIT_ENTER_2's `permissions.rs`.

---

**Progress (2026-10-11): done on one machine.** The done-when, both
halves: `crates/hit-node/tests/m5_done_when.rs` (a banned user's next
message beat the ban to a peer and is dropped when the ban arrives; then
her patched client pushes messages dated now and backdated, and a reaction,
straight to every peer, and spends her old invite — nobody keeps any of
it) and `scripts/m5-by-hand.sh` (four CLI processes, `hit-cli lie`); and
`hit-group/tests/fold.rs` is 1,844 lines against HIT_ENTER_2's 1,239 in
`permissions.rs`, with HIT_ENTER_2's ban, mute, slowmode, moderator-delete,
channel, override and role tests and the mesh's own: two equal admins
banning each other in one second (neither applies), an admin and a
moderator doing it (the admin wins, either order), a snapshot racing a ban
(the honest limit, asserted), a ban in the same second as the join. The
app: a member dialog (roles, mute, kick, ban — with what a ban can and
cannot do, before the button), group settings (rename, channels and
slowmode, bans, compaction), *remove* on messages for moderators, a
muted composer, a "you were banned" banner. Driving it found two more focus
defects and a protocol gap: a removed person never learned it (now the
removal is the one thing they may read). **Not done**: invite revocation
and use caps (`MANAGE_INVITES` exists, nothing uses it), per-channel
override editing in the UI (the wire and the fold have it).

---

### M6 — Anchors: the peer that never sleeps
**Size: M**

`hit-anchor`: `hit-node` headless with a pin policy. Pins groups, fetches
every file, optionally holds a group's nsec for compaction, optionally
listens on `wss://` as a real Nostr relay, optionally runs `iroh-relay`.
`docs/SELF_HOSTING.md` for a Raspberry Pi and a €4 VPS.

**Done when:** a group with an anchor and every member's app closed still
admits a new member at 4am, and a NIP-29 client (Chachi or 0xchat) reads
the group through the anchor's `wss://` address.

**Progress (2026-10-11): done on one machine; the NIP-29 client half
waits for a real `wss://`.** `hit-anchor` (`crates/hit-anchor`): `init`,
`pin <link>` (joins as a member, marks its profile `bot`), `run` (sync;
compaction on a timer for groups whose key it holds; `--ws` for the
bridge), `--relays <url>` for an own `iroh-relay`; keys from systemd
credentials on a server (`--credentials`) or the keychain. Invite links name
a group's anchors first — from the store, so the CLI's links do too. The
bridge: NIP-11, NIP-42, members-only reads as `hit/nostr/1` answers, group
state translated to NIP-29 and signed by the anchor, live subscriptions,
channel-less posts refused with the reason. The 4am half: automated
(`m6_done_when.rs`, the owner and every member shut down, a newcomer joins
through the anchor and the owner later sees them) and by hand
(`scripts/m6-by-hand.sh`, 8/8). `docs/SELF_HOSTING.md` for a Pi and a VPS:
systemd unit, encrypted credentials, Caddy for `wss://`, `iroh-relay`.
**Still owed:** Chachi or 0xchat against a real `wss://` (`TESTING.md` §5).
Not built, by plan: files (M8), the DM inbox (M7), embedding `iroh-relay`
(the docs run n0's binary).

---

### M7 — DMs, end to end
**Size: M**

NIP-17: gift wrap (1059) around a seal (13) around a chat message (14),
NIP-44 encryption. Direct delivery when both are online; otherwise to the
recipient's inbox anchors (kind 10050). The DM list, unread state, and
opt-in publishing of profile + inbox to public relays for people outside
the app.

**Done when:** a DM to your friend while their app is closed arrives when
they open it, via an anchor — and a test scans **the bytes** of that
anchor's database after delivery and finds neither the text nor the
sender's npub. (HIT_ENTER_2's `a_space_holds_nothing_it_could_log_in_with`,
reborn.) Bonus: a DM from Amethyst or 0xchat arrives, if the user opted in.

**Progress (2026-10-11): done on one machine.** NIP-17 as specified
(`hit-event/src/dm.rs`, `nostr`'s NIP-44/59): a rumor sealed and wrapped,
one wrap to the recipient and one to the sender. `hit-group` keeps a wrap
only for its recipient and the anchors their inbox list (10050, our
`anchor` rows with a device to dial) names; the sender keeps an outbox
copy. Sent straight to the recipient's devices and inbox anchors; the
sync loop retries and syncs with the inbox anchors of everyone we share a
group with. The done-when, automated: Bob asleep, Alice's DM waits at
Bob's anchor — which has never shared a group with Alice — Alice goes
offline too, Bob wakes and reads it; then every byte of the anchor's data
directory holds neither the text nor Alice's key in hex, npub or raw form
(and does hold Bob's, the control). The app: a DM list and conversations,
*message* on a member, unread counts that survive a restart, and an
opt-in "let my groups' anchors keep my DMs" switch with the trade-off said.
**Not built:** opt-in publishing of profile and inbox to public relays (an
outbound WebSocket in the app — a decision for the owner), so DMs with
Amethyst or 0xchat; DMs to someone you share no group with; DM files (M8);
rate limits on what an anchor keeps.

---

### M8 — Files
**Size: M — mostly a port**

HIT_ENTER_2 M7's attachment path onto the mesh: a file message carries a
NIP-92 `imeta` tag (BLAKE3 hash for iroh, SHA-256 too so an anchor can
mirror to a Blossom server later), size, type, and the `EndpointId`s that
have it. Fetch from any provider, verify by hash. Serve only to members
who can view the channel. An LRU cache with a cap on members; anchors pin
all. Inline images as before. DM files encrypted per NIP-17 kind 15.

**Done when:** a file posted while its uploader is online is later
downloaded, byte-for-byte, by a member **after the uploader has gone
offline**, from another member's cache.

**Progress (2026-10-11): done on one machine; DM files are M8b.** NIP-92
`imeta` on kind 9 (BLAKE3 for iroh, SHA-256 for a Blossom mirror later),
limits judged by every peer; `iroh-blobs` 0.103.1 on our accept loop,
served behind two gates (a device that speaks for someone; a hash attached
to a message that person may read — private channels stay private), pushes
refused; fetched whole and verified, dialling every possible provider at
once; a member's cache keeps its own files and the newest up to 1 GB, an
anchor pins everything (`hit-anchor run` fetches what is missing every 30
s); the collector's live set is read from the event store before every
sweep. The done-when, automated (`m8_done_when.rs`: Alice posts, Bob opens,
Alice goes, Carol gets it from Bob byte for byte; a stranger and a member
who cannot see the channel are refused) and by hand
(`scripts/m8-by-hand.sh`, `hit-cli post-file` / `get-file`). The app:
attach (+), pictures inline, a chip with *save* otherwise. **Not done:** attaching through the native dialog by hand.

**M8b — DM files, done on one machine (2026-10-11).** NIP-17 kind 15: the
file AES-256-GCM-encrypted with a fresh key (RustCrypto's `aes-gcm`), the
ciphertext stored and fetched as a blob, the key only in the sealed rumor;
served to the two people only (`blob_grants`, recorded by their nodes as
they send and receive). `m8b_dm_files.rs`: the file arrives whole, every
byte of both data directories is scanned for it and holds none of it in the
clear, and a member of a group they share is refused the ciphertext. The
app: a + in the DM composer, pictures inline, *save*; the window never
sees a key. Limit: an anchor cannot serve a DM file (it cannot open the
wrap to know it) — the sender's or recipient's devices must.

---

### M9 — Voice channels
**Size: L — the riskiest milestone left after M0**

`hit-voice` on S3's findings: `cpal`, Opus, a jitter buffer, echo
cancellation, `hit/voice/1` (or `iroh-roq`). Voice channels in a group's
metadata, voice presence as ephemeral events, a mesh capped at 8, push-to-
talk and voice activation, mute, deafen, a speaking indicator, input and
output device pickers. 1:1 calls from a DM.

**Done when:** four people in two cities sit in a voice channel for thirty
minutes, **one of them on laptop speakers**, and nobody hears an echo; a
ninth person is told the channel is full rather than breaking it.

**Progress (2026-10-11): built and driven on one machine — the done-when is
owed.** `hit-voice`: Opus 48 kHz mono at 32 kbit/s with FEC (system
libopus), a jitter buffer that recovers from FEC and conceals, cpal devices
with WebRTC's echo canceller and noise suppression (bundled, BSD-3-Clause),
and a call engine on our own `hit/voice/1` — `iroh-roq` is iroh 0.33 only.
Voice presence is kind 20437 on gossip; the lower device id of each pair
dials; the listener gates the dialler's *device* to a member who may view
the channel, and re-checks every second who may be heard (`SEND_MESSAGES`,
not muted). The node gained pluggable protocols (`Node::serve_protocol`).
`m9_voice.rs`: two members hear each other's tones; a moderator's mute
silences one mid-call and lifting it brings them back; a stranger dialling
in is refused; a full channel (cap 2 in the test) refuses the next member;
leaving empties the call. The app: 🔊 channels with who is in them and who
is speaking, join by click, a call bar (mute, deafen, push-to-talk on `` ` ``
or a hold button, leave), audio settings (devices by id, echo cancellation),
voice channels made in group settings — all driven on null sinks
(`TESTING.md` §4d). **1:1 calls from a DM** followed the same day: a
*call* button in the DM view, an incoming-call banner (answer, decline;
the desktop asked for attention), ringing for 45 s on every device of the
callee's, and a gate that lets only someone you share a group with or have
written to make your app ring (`m9_dm_calls.rs`). **Private voice
channels** the same day: their presence handed directly to the members who
may view them, never gossiped (`m9_private_voice.rs`: the two in the room
find and hear each other; a plain member never learns who is in it and
cannot join); the owner's *private* box in group settings (members denied,
moderators and up allowed) and a 🔒 in the channel list. **Calling someone
with no group in common**, also the same day: every DM carries its
sender's device binding, sealed, and the recipient keeps it; a **contact
link** (`hit-enter:contact?…`, in the profile dialog; *+ new* in DMs, or
*write to someone* with no groups at all) gets the first one through
(`m9_no_group_calls.rs`: link, DM, call refused, reply, call answered and
heard, kept across a restart). Names stay npubs between such people.
**Drift compensation** the same day: a controller per device buffer
(`hit-voice/src/drift.rs`) nudges the resampler within ±0.5 % to hold
40 ms — simulated for ten minutes at ±0.3 % (ten times a bad device) with
no drop and no gap, where the same run without it drops or starves ~90
times; a jitter-buffer catch-up; glitch counts in the call bar. **Not done:** the
done-when (§6, people and two cities); a warning on Bluetooth HFP; a ninth person at the same instant as the eighth can get
a partial call (each listener refuses past the cap, so they connect to some
and not others).

---

### M10 — Zaps → v1.0
**Size: M**

`hit-zap`, the only crate with an HTTP client: `lud16` on profiles, NWC
connect (connection string in the keychain), zap a message or a file with
an amount and a comment, the QR fallback, receipt fetch and NIP-57
validation, zap totals on messages, anonymous zaps, private groups'
receipts routed only to their own anchor, and **the 1% fee** exactly as
`DESIGN.md` §11 specifies it: a second payment on top, shown in the dialog,
none under 100 sats, zap first and fee best-effort, no message id or pubkey
in the fee payment.

**Progress (2026-10-11): built and driven against a fake Lightning network
on one machine — S4 and the done-when with real sats are owed.**
`hit-zap` (LNURL-pay over HTTPS, NIP-47 over its own small relay client,
the fee, an in-process fake network behind the `fake` feature),
`hit-event::zap` (bolt11 fields, receipt checks, the kind 9436 carrier),
`hit-group` judging carriers, `hit-node` carrying receipts and counting
totals against the recipient's `zap_pubkey`, the anchor's `ws://` taking
receipts, and the app: wallet and address in the profile, the zap dialog
with the fee shown, the QR fallback, totals on messages.
`crates/hit-anchor/tests/m10_done_when.rs` is the done-when below with a
fake network in place of real wallets. Not built: a public-relay fallback
for groups without a `wss://` anchor, and the fee address (none chosen —
no fee is charged until there is one).

**Done when:** you zap your friend's file 1,000 sats from inside the app,
their wallet receives it, the total appears on the file **in both apps and
on a third member's**, and a forged 9735 (valid signature, wrong LNURL key)
is shown nowhere. And for the fee: the 1,000-sat zap arrives as
**1,000** (not 990) while HIT_ENTER's wallet receives **10** with a memo that
names nothing; a 50-sat zap charges nothing; and a fee payment forced to fail
leaves the zap delivered and counted. Tag **v1.0.0**.

---

### M11 — Hosted anchors (the business)
**Size: M — mostly HIT_ENTER_2 M9, reused**

HIT_ENTER_2's grants, lifecycle and billing seam, pointed at anchors instead
of spaces: a "keep my group online" button, a signed grant a hosted anchor
verifies offline, payment in sats (Lightning invoice / NWC) and by card
through the existing Lemon Squeezy adapter.

**Done when:** a group owner pays, from inside the app, and their group
is pinned by a hosted anchor within a minute; stops paying, and it walks
through grace and read-only exactly as HIT_ENTER_2's spaces do.

### M12 — 10× lighter than Discord (after the beta ships)
**Size: S for the trim, S for the spike, L if the window is rewritten**

The goal, written down for the first time (owner, 2026-10-17): **an app
10× lighter than Discord in RAM**. Discord sits at 400–800 MB, so 10× is
**40–80 MB for everything**. Measured on beta.2 (2026-10-17, PSS, idle, one
group, Linux):

| | |
|---|---|
| `WebKitWebProcess` — the window | 244 MB |
| `WebKitNetworkProcess` | 49 MB |
| `hit-enter` — of which **heap ~99 MB**, the rest mapped libraries | 177 MB |
| **total** | **≈ 470 MB**, 46 threads |

That is Discord's range, not a tenth of it. The uncomfortable part: **the
WebKit window alone is 3–4× the whole budget.** No trimming of a web UI
reaches 10×; the browser engine is the cost.

**Against Discord itself — the baseline (2026-10-17, same machine, same
30 s).** Both idle on hidden workspaces; Discord app 1.0.161 (Electron);
HIT_ENTER beta.2 after hours of use (calls, files). Linux, 12 threads,
31 GB, RTX 3050. PSS summed over each app's processes; CPU from
`/proc/<pid>/stat` over the window; GPU from `nvidia-smi pmon`.

| | HIT_ENTER | Discord | |
|---|---|---|---|
| processes | 3 | 10 | |
| **RAM** | **488 MB** (1.52 %) | **726 MB** (2.27 %) | we are **1.5×** lighter |
| **CPU, idle** | 0.73 % of a core (0.06 % of all) | 0.47 % of a core (0.04 %) | **Discord is quieter** |
| **GPU** | 0 % SM, 1 MiB VRAM | 0 % SM, none listed | a tie at zero |

Read plainly: **10× is ~73 MB, 6.7× below today.** And idle CPU is ours to
lose: presence, sync and gossip run where Discord leans on a server —
small, but it is battery on a laptop, so M12 covers it too.

**The next measurement — under load**, the same way, before and after each
step: both windows visible, scrolling a busy channel, a picture-heavy
channel, and a voice call of the same length. Idle and hidden is the
kindest case for both apps; Discord's CPU and GPU usually climb under load,
and so may ours.

**Where the 488 MB is — taken apart (2026-10-17, release build, PSS).**

| | PSS | heap (anon) | threads |
|---|---|---|---|
| **the empty shell** — Tauri + WebKit, onboarding screen, no node | **254 MB** | 124 MB | 48 |
| · `hit-enter` (GTK, WebKit's UI side, Tauri) | 94 MB | 42 MB | 27 |
| · `WebKitWebProcess` (the page) | 116 MB | 53 MB | 11 |
| · `WebKitNetworkProcess` | 43 MB | 29 MB | 10 |
| **the node alone** — `hit-cli run`: iroh, store, sync, gossip, blobs | **34 MB** | **9 MB** | 30 |
| the real app after hours of use | 488 MB | | |

**This is the finding the plan rests on: our node is already 10×-class.**
34 MB is half the 73 MB budget, and its heap is 9 MB. **The weight is the
window**: 254 MB before anyone logs in, and ~200 MB more as the page fills
with messages, pictures and JavaScript state. So M12 is a new window, not
a tuned node. A web UI cannot get there: the engine's empty floor is 3.5×
the whole budget.

**What the outside evidence says (researched 2026-10-17).** No clean
benchmark of Rust toolkits for a chat app exists; these are the best
signals, each with its caveat:

- **The GPU stack is itself a memory cost.** One app went from 135 MB to
  30 MB moving from egui (GPU) to Slint's *software* renderer — the author
  credits most of it to dropping the GPU stack, not the toolkit
  ([trystan-sarrade.com](https://trystan-sarrade.com/article/rust-gui-135mb-to-30mb-egui-to-slint/)).
  On this machine that means NVIDIA's userspace driver, which maps tens of
  MB into anything that opens Vulkan or GL.
- **A native Discord client in Slint reports < 35 MB and < 0.1 % CPU idle**
  (Litecord, self-reported, one post:
  [tabnews.com.br](https://www.tabnews.com.br/Ak4ai/construi-um-cliente-de-discord-nativo-em-rust-que-usa-apenas-30mb-de-ram-e-0-1-por-cento-de-cpu)).
  Not independent — but it is our exact shape of app, and in our budget.
  Code: [github.com/Ak4ai/Litecord](https://github.com/Ak4ai/Litecord)
  (MIT; Slint 1.9; text, voice with Opus + jitter buffer, QR login, a
  tray "DeepSleep", automatic fallback to software rendering). Voice call
  ~32–45 MB, idle CPU 0.0 % minimised, ~8 MB binary, < 200 ms start. **It
  is lighter than our target can be:** it is a window onto Discord's
  servers, we carry our own node (~34 MB). M12-9 should measure it beside
  the Slint app on this machine — one `mem-bench` run, not their numbers.
  It shows no "Made with Slint" badge or `AboutSlint`; as an open-source
  app it is not on the royalty-free terms we would be.
- **GPUI** peaked at ~125 MiB in a 100k-row scroll bake-off on an M4 Max
  ([zed discussion #63832](https://github.com/zed-industries/zed/discussions/63832)):
  GPU-first and pre-1.0, distributed as forks/snapshots. Over budget on
  its own; out.
- **iced** has a CPU renderer (tiny-skia, selectable with `ICED_BACKEND`)
  said to be lighter than wgpu, but nobody publishes numbers
  ([iced renderers](https://mintlify.com/iced-rs/iced/advanced/renderers)).
  Halloy (an IRC client on iced) shows it carries a real chat app; its RAM
  is unmeasured.
- **Licences.** iced: MIT. Slint: its **Royalty-Free licence allows a
  closed-source desktop (and mobile) app, free**, if the app shows Slint's
  About widget in an About dialog, *or* the download page shows the Slint
  badge; not for embedded systems; the app may not expose Slint's APIs
  ([slint.dev terms](https://slint.dev/terms-and-conditions)). Workable for
  us — an About dialog is cheap — but it is a licence with conditions, so
  it is the owner's call, written in `DESIGN.md` if taken.

**The budget — the arithmetic 10× has to fit (idle, one group open):**

| | MB |
|---|---|
| the node (measured today: 34; after M12a's trims) | 25–30 |
| toolkit + CPU renderer + one window's buffers | 15–25 |
| fonts and the glyph cache | 3–5 |
| messages on screen (only the visible rows) | 1–3 |
| pictures (decoded thumbnails, a capped cache) | ≤ 10 |
| **total** | **≈ 45–73** |

It fits — with no room for a GPU driver, a second process, or a picture
cache without a cap. **Under load it will not stay under 73 MB** (a call
adds Opus and the echo canceller, a full-size picture is width × height ×
4 bytes); the honest claim is **10× at idle, several × under load**, and
the load numbers get published beside the idle ones.

**The milestones.**

**M12a — the measuring stick (S, first, a day).** `scripts/mem-bench.sh`:
the numbers above, the same way every time — idle; under load (a busy
channel scrolled, 50 pictures, a five-minute call); and Discord beside it
when it is running. PSS per process, CPU over a window, threads. Every
later milestone is judged by it, and a regression past the budget fails
the check.

*Idle half built (2026-10-18):* `scripts/mem-bench.sh [names…]` — PSS
summed over each named process and its descendants, threads, CPU over
`SECONDS_IDLE` (30 s). First run, beta build on the onboarding screen
(`scripts/gui/launch.sh`, nobody signed in) beside Discord signed in:
**hit-enter 288 MB, 48 threads, 0.03 % of a core; Discord 724 MB, 156
threads, 0.63 %.** Owed: the load half, the budget check, the Mac
(`footprint`, no `/proc`).

**M12b — trim the node (S, in parallel, a few days).**
- **Threads:** 30 in a node that is idle — tokio with 2 workers, iroh's
  and sqlx's pools capped.
- **Idle wakeups:** presence and sync back off when nothing changes;
  nothing wakes a hidden, quiet app. Goal: Discord's 0.47 % of a core or
  less.
- **Voice only in a call**: Opus, the echo canceller, cpal devices.
- **mimalloc** (MIT) measured against glibc's arenas; kept only if it wins.
- **SQLite `cache_size`** and the blob store's buffers, capped.

*Done when:* the node ≤ 30 MB PSS and ≤ 0.47 % of a core idle.

**Decided (owner, 2026-10-18): Slint, no bake-off.** The idle number
(288 MB on the sign-up screen, WebKit 168 of it) was enough; Litecord is
the existence proof. Slint 1.18, the **winit backend with the software
renderer**, no Qt backend (LGPL). Licence: **Slint Royalty-Free 2.0** —
free for a closed desktop app on condition of attribution; we meet it
with Slint's `AboutSlint` widget in the app's About (the owner may swap
it for a "Made with Slint" badge on hitenter.app). Accepted in
`about.toml` as `LicenseRef-Slint-Royalty-free-2.0`, with that paragraph.
**Same design:** HIT_ENTER_2's look — colours, layout, type — ported as
Slint globals; shadows, blur and animations only where cheap.
**The benchmark runs once, before the first Slint release** (M12-9), not
per screen.

**M12-1 — `hit-app-core` (S). Done 2026-10-18.** `src-tauri/src/state.rs` (already free of
Tauri, M4a) and the event pump move to `crates/hit-app-core`; the pump
emits typed `UiEvent`s to any sink instead of Tauri events. Tauri becomes
a thin shell over it and keeps shipping. Tests move with it.
*Done when:* the Tauri app works unchanged on it; tests and clippy pass.

**M12-2 — the Slint shell (S).** `crates/hit-desktop`, binary
`hit-enter-native` until the switch: a window, the theme, the software
renderer, `AboutSlint`, the tokio runtime beside Slint's event loop, the
licence accepted. *Done when:* an empty window, and its idle PSS.
*Done 2026-10-18:* **24 MB PSS, 8 threads, 0.00 % CPU** on the sign-up
screen — the web window's same screen is 288 MB. The node is not running
yet on that screen in either; with it, add ~34 MB. No OpenGL, Skia-GPU or
Qt in the tree (`cargo tree`), only `tiny-skia`. Slint's licence and
JetBrains Mono's OFL are in `about.hbs`; the About box shows
`AboutSlint`, reachable from the first screen.

**M12-3 — onboarding (S).** Create or import a key, the backup that
cannot be skipped, the name. Enforced in Rust already.
*Done 2026-10-18,* driven by keyboard on a throwaway identity: make a
key → backup by "show the key once" → the last 8 characters → the name;
and import an nsec → the name. **43 MB PSS, 23 threads, 0.07 % CPU
signed in with the node running** (no groups yet). The passwords are
checked *before* the save dialog opens. Not driven: the encrypted-file
backup through the portal's save dialog (a GTK dialog takes no `wtype`,
and must not be screenshotted) and the copy button — the owner's to try.
`wtype -M shift -k Tab` moves focus *forward* in the Slint window; drive
it with Tab only.

**M12-4 — the main window (M).** Rail, channels, messages (a virtualised
`ListView`), composer, member list; live through `UiEvent`. Keyboard
first — focus is ours now, not WebKitGTK's.
*Done 2026-10-18* (`src/chat.rs` is `chat.svelte.ts` ported; the message
rows are patched in place, so arrivals do not move a reader), with the
make/join and invite dialogs pulled forward from M12-5 — nothing can be
tested without them. Driven in two native windows on one machine: Alice
makes a group and an invite; Bob joins with it, sees the history, both
members and who is online; messages both ways, live; 40 messages in a row
followed to the bottom with wrapped rows of varying height; history kept
across a restart. **One window alone, a group open, 43 messages, a peer
connected: 49.5 MB PSS, 23 threads, 0.10 % of a core** (Discord: 724 MB).
Found and fixed: the screen that replaces a focused one must take focus
(after onboarding, nothing had it — WebKitGTK's bug, in Slint); Slint's
lists take focus without showing it, so **Escape returns to the composer**
from anywhere. Owed: Tab is not trapped inside a dialog; no "typing
anywhere goes to the composer"; window focus is not known, so a channel
open in a background window counts as read.

**M12-5 — groups (M).** Add/join by invite, invite dialog, group
settings, member dialog and moderation, unread, presence, reactions.

*Done (2026-10-18), on one machine.* Group settings (rename, channels
with slowmode, private and delete, bans with unban, compact), the member
dialog (roles, mute and lifting it, kick and ban behind the same
warnings), reactions (chips, toggling mine), and a message's bar: delete
mine, remove someone else's as a moderator. Driven in two native windows:
every one of those, with the other window seeing it — Bob's ⏱, his
"muted for another 10 min", his "Moderator", the removed message, the
ban banner. **One window, a group open: 51.1 MB PSS, 23 threads, 0.10 %**
(M12-4: 49.5). M12-4's three owed items, two paid: **Tab stays inside a
dialog** (its last control hands back to the dialog itself), and
**typing anywhere goes to the composer**. Found on the way:
- **Slint's software renderer draws no colour emoji** — the system's Noto
  Color Emoji came out blank. The window ships **monochrome Noto Emoji**
  (OFL, 1.9 MB in the binary; only the glyphs drawn are touched) and
  every emoji text names it. Reactions are line drawings, not Discord's
  colours. Colour would take the Skia renderer — a GPU path, RAM.
- **A new model rebuilds every row**, and a row rebuilt under the keyboard
  takes the focus with it: every list is now made once and patched (as
  the messages already were). Controls that vanish when used (lift the
  mute, unban, delete) move the focus to *close* first — WebKitGTK's
  lesson again.
- **Tab cannot walk into a message's bar** (Slint's focus chain passes
  over it), so a focused message takes **1–5** for the five reactions and
  **Delete** to withdraw or remove; the bar shows the keys.
- A wrapped text's height is measured before it has a width: dialog
  texts carry the card's width, or the card comes out lines short.
- `changed` does not fire for a value set in `init`; bind instead.
Still owed: the member dialog's *message* button (DMs are M12-6); window
focus (a background window still marks its channel read).

**M12-6 — DMs and files (M).** DM view, contact links, files and
pictures (decoded at display size, cache capped), the save dialog.

*Done (2026-10-18), on one machine.* The DM pane (conversations, the
thread, its composer, unread on the rail's *DM*), *message* in the member
dialog, *+ new* with a contact link, files in group messages (attach,
pending chips, send) and in DMs (sent at once, encrypted), pictures
**decoded at most 400 × 300 and kept under a 48 MB cap** that spares what
is on screen (`src/pictures.rs`, tested), *show* above 8 MB, *save* through
the portal. Driven in two native windows: a 1600 × 1000 PNG sent to the
group, fetched by Bob from Alice and shown; saved by Bob, byte-identical; a
DM, a file DM saved by Alice decrypted and byte-identical, a picture DM
shown on both sides; a bad contact link refused with its reason. **One
window, a group open, a picture on screen: 51.5 MB PSS, 22 threads,
0.10 %.** Found:
- **The window was wider than its tile since M12-4**: two 224 px lists
  and an unshrinkable header made a least width of ~820 px in a 753 px
  tile (1.25× scale), and Hyprland cut the member list. Below 880 px the
  member list is now a drawer (Escape closes it); the IP line may elide.
  A hidden view still counts toward its container's least width.
- A picture's width bound to its own row is a binding loop: the row
  passes its width in (`room`). Bubbles too: a long DM widened the pane.
- Tab does reach a file's *save* in a message row (unlike the reaction
  bar); a focused message or DM also takes Ctrl+S and Enter (*show*).
- Debug builds take `HIT_TEST_PICK` / `HIT_TEST_SAVE_DIR` instead of the
  portal's dialogs, so the agent never drives or sees one.
Still owed: a first contact that succeeds, by hand (the contact link is
shown in the profile, M12-8); a picture above 8 MB through *show*; the
cache cap under a real gallery, measured in M12-9's load half.

**M12-7 — voice (M).** Call bar, incoming call, audio settings; the
attention request when a call rings.

*Done (2026-10-18), on one machine, null sinks only.* Voice channels in
the channel list (who is in, lit while speaking; Enter joins), the call
bar (mute, deafen, push-to-talk or voice, leave, audio, the hold
button), the ` key held to talk, a 1:1 call from a DM header, the
incoming-call banner (never focused on arrival; one Tab from the
composer reaches *answer*), the audio dialog, and
`request_user_attention` when a call rings (Slint's `unstable-winit-030`).
Measured on the null sinks, a tone into one window's mic recorded at the
other's speaker: **heard −34 dB; muted −91 dB (silence); push-to-talk
key up −91, held −34; 1:1 call both ways −33.7 / −33.6**; every stream
closed on leave and hang-up. **One window: 50.8 MB idle, 55.0 MB and 27
threads in a call** (1.5 % of a core). Not checked: the urgency hint
(Hyprland's client list does not show it), real devices and people —
`TESTING.md` §6 stays owed. The device list is long on ALSA and scrolls;
names are not elided. The first character typed into a DM opened from
the member dialog can be lost while the pane appears.

**M12-8 — zaps and the profile (S).** Zap dialog with fee and QR, wallet
and address in the profile, key backup to a file.

*Done (2026-10-18), on one machine, against the fake network
(`TESTING.md` §7a).* *you* on the rail (name, npub, device, network, the
contact link, Lightning address, NWC wallet in a password field, DMs while
away, another backup file — it scrolls, and follows the keyboard); ⚡zap
on others' messages (or `z` on a focused one), the dialog quoting before
paying, from the wallet or by QR code — **drawn to pixels in Rust**
(`qrcode`, no SVG renderer), and checked by decoding the screenshot back
to the invoice with `zbarimg`; ⚡ totals on messages, refreshed by zap and
profile events. Results: *1,000 sats to Bob · 10 sats HIT_ENTER fee ·
1,010 total*; the fake network logged `alice → bob 1000 (zap)` then
`alice → hitenter 10, "HIT_ENTER fee"`; by QR at 200: waited, *Zapped 200
sats to Bob*, then the fee's own QR, 2 sats; Bob's message showed ⚡ 1,200,
and still did after a restart. Bob added Alice by her contact link and the
conversation opened (M12-6's owed first contact). A backup file was
written (debug builds take `HIT_TEST_SAVE_DIR` here too). The zap
dialog's first focus is the amount; without it the Tab order was wherever
focus had been. Real sats remain `TESTING.md` §7b's, with the owner.

**M12-9 — the desktop, the benchmark, the switch (M).** Single instance,
`hit-enter:` links, clipboard, packaging (.deb, AppImage, .dmg). Then
**`mem-bench` with its load half, on Linux and the M1, beside Discord**,
published on hitenter.app with the method. Then `web/`, `src-tauri`,
Tauri and WebKit leave the tree, the binary is renamed `hit-enter`, and
it ships as the next beta.
*Done when:* everything beta.2 does, and the number is measured — the
target **≤ 73 MB idle** with a group open.

*First half done (2026-10-18): the desktop, on Linux.*
- **One window per data directory**: a Unix socket in it
  (`src/instance.rs`); a second copy — a clicked link — hands over its
  `hit-enter:` links and exits in ~10 ms; the first opens the join or
  contact dialog with the link filled in (tested; unit-tested too).
  Not on Windows (no Unix sockets in std there).
- **The scheme handler** is written to `~/.local/share/applications` and
  made default with `xdg-mime` — never under `HIT_DATA_DIR`, so test
  windows cannot take it over (an old test build of the web window still
  holds it on this machine).
- **Window focus** (winit's `Focused`): a channel or DM open in a window
  in the background is no longer marked read; the read mark moves when the
  window comes back (tested on `read.json`). M12-4's last owed item.
- **Packaging without Tauri**: `scripts/release-native-linux.sh <version>`
  builds in **Ubuntu 22.04 (glibc 2.35)** —
  `crates/hit-desktop/packaging/linux-build.Dockerfile`, Rust 1.98.1 — and
  assembles a `.deb` (by `ar`/`tar`; package `hit-enter`, `/usr/bin/
  hit-enter`, so it upgrades the Tauri beta) and an AppImage
  (`appimagetool` 1.9.0, pinned by hash; Ubuntu's libopus inside). **The
  Tauri betas were built on Arch and need glibc 2.43**: they cannot run on
  Debian or Ubuntu at all. The new .deb installed and ran (under Xvfb) on
  Ubuntu 22.04 and Debian 12, installed on Debian 13; the AppImage ran
  here with window class `hit-enter`. **AppImage 27 MB (Tauri's: 116 MB)**,
  .deb 19 MB.
*The benchmark's Linux half (2026-10-18), `scripts/bench/run.sh
<native|web>`*, a freshly seeded world each run (Carol, a CLI node: 1,000
messages, 50 photos of 3000 × 2000), PSS over the app's process tree once
a second, this machine, release builds:

| | native (Slint) | web (Tauri, as shipped) |
|---|---|---|
| sign-up screen | 32 MB | 264 MB |
| signed in, no groups | 51 MB | 298 MB |
| a channel of 1,000 messages | 61–62 MB | 620 MB |
| 600 more arriving (peak) | 66 MB | 771 MB, 20 % CPU |
| a channel of 50 photos (peak) | 96–102 MB (112–137) | 734 MB (1,250) |

Discord, idle beside them: 533 MB, 161 threads (724–745 in earlier runs).
**Found by it:** glibc kept 506 MB of freed 24 MB photo decodes in thread
arenas (580 MB with the photos open); a fixed mmap threshold, two arenas
and one decode at a time brought it to ~100 MB. Not measured: the
five-minute call (`scripts/bench/call.sh`, stopped by the owner; M12-7's
short call: 55 MB). Owed: the M1. Seen: rows posted within one second
show out of order; the photo list does not stay at the bottom while
pictures grow it.

*The switch (2026-10-18).* `web/`, `src-tauri`, Tauri and WebKit left the
tree; the Slint binary is `hit-enter`, the workspace `0.1.0-beta.3`.
`scripts/release-linux.sh` is the Slint one; `scripts/release-macos.sh`
assembles the `.app` and `.dmg` by hand (to run on the M1). Checked: the
published **beta.2 does not run on Debian 13** (`GLIBC_2.43 not found`);
beta.3 **upgrades it** there (apt, `0.1.0-beta.2` → `0.1.0-beta.3` — the
first build's `0.1.0~beta.3` sorted *below* beta.2 and was refused) and
runs; it runs under Xvfb on Ubuntu 22.04; the AppImage opens here.
Notices: 742 kB → 640 kB without Tauri's tree.

*Released (2026-10-18): **v0.1.0-beta.3** on `isciuciustin/hitenter`,
pre-release, Linux + macOS from commit `3a0483e`: `hit-enter_0.1.0-beta.3
_amd64.deb` (sha256 `9ad706f4…`), `HIT_ENTER_0.1.0-beta.3_x86_64.AppImage`
(`76da6484…`), `HIT_ENTER_0.1.0-beta.3_aarch64.dmg` (`db32f3dd…`, built on
the M1), `THIRD_PARTY_NOTICES.md`, one `SHA256SUMS`; every file downloaded
back from the public URLs and checked. The notes say the macOS keychain
prompt on update (ad-hoc signature) and why beta.2 failed on Debian/Ubuntu.
hitenter.app still links beta.2 until the site is updated.

Owed in the second half: macOS (a `.app` with `CFBundleURLTypes`, the
`.dmg`, and links — macOS hands a URL to the running app as an Apple
Event, not an argument, and winit does not pass it on); the benchmark's
load half on Linux and the M1; then the switch.

**Risks, said out loud.**
- **Software rendering costs CPU.** Scrolling at 4K or 144 Hz on a
  laptop may stutter or eat battery. M12-9 measures CPU under scroll too;
  if it fails, the GPU path is the fallback and 10× slips to ~5×.
- **Pictures are where RAM goes.** A chat full of images will not sit at
  73 MB under any toolkit; the cap is a choice of what to evict.
- **Two UIs at once** (Tauri and native) through M12-2…M12-8 is double work for
  every new feature. Freeze features until the switch.
- **"10×" has to be honest:** idle, one group, the same machine, the
  method published. A number we cannot reproduce in front of a sceptic is
  worse than none.

**Deliberately not:**
- Trimming the web UI (lazy routes, virtual lists) as a substitute — it
  shaves tens of MB off a floor that is short by hundreds.
- GPUI — GPU-first, pre-1.0, over budget in its own benchmark.
- A new UI in another language (C++/Qt): Qt 6 measured ~215 MiB in the
  same bake-off, and the licence is LGPL — not allowed here.

### M13 — Public communities (the north star's first step)
**Size: L**

`DESIGN.md` §0, rows 1 and 3: a community that can be **found**, **read
before joining**, and **joined without an invite** — plus DMs that arrive
while you are offline, which a Discord-sized community takes for granted.
The owner's answers (2026-10-24):

1. **A newcomer posts at once** — no waiting room. Moderators act after
   (kick, ban, mute, slowmode exist since M5).
2. **Profiles go to public Nostr relays by default.**
3. **The DM inbox is on by default**, with limits: an anchor keeps a DM
   **7 days**, and at most **10 MB of waiting DMs per person**, oldest
   dropped first.
4. **A new group is private**; making it public must take **little
   thought** — one clear choice, not a settings hunt.
5. **The best public relays available** for profiles and announcements.
6. **A newcomer to a public group waits at least 5 minutes before
   posting** — a floor every peer enforces; admins can make it longer,
   never shorter (answered 1 minute, then 10, settled on 5 the same day:
   10 buys little more protection than 5 and costs every newcomer).

**Progress (2026-10-24): step 1 built** — the flag in `39000` (NIP-29's
`public`/`open` or `private`/`closed`, always written; an M3 group reads
private), the owner-only `9002` edit, kept by compaction, the bridge's
own `39000` saying the same, and the window: *private / public* when
making a group, *make public / make private* in settings, each with its
one sentence. **The flag only records the choice** — reading, joining and
finding come with steps 2, 3 and 5.
**Step 2 built (same day):** a `9021` with no code opens a public group
(the group public *at the join's point in the fold*; bans hold); the link
without `&c=` (`public_link`, never expires; the invite dialog hands it
out in a public group, and anyone there may); every self-join is a
newcomer until the next snapshot — by invite too, since links get posted
— and a moderator's `9000` is not; `newcomer_wait` in `39000`, set with
*manage channels*, a public group never under 300 s; the composer counts
down (*"new here — you can post in 4:42"*). Driven in two windows.
**Honest limit:** judged by the dates the newcomer wrote, like slowmode —
a patched client can date its join early. And making a group public puts
anyone who joined in the last five minutes under the floor.
**Step 3 built (2026-10-25):** read before joining. `may_read` gives a
non-member of a public group what a plain member who sees only its open
channels reads — **actions and invites included** (the plan said members
only; a reader's node must fold them, or it refuses every message as "not
a member"); members' profiles readable and storable by anyone, bindings
not. `Node::follow(link)` carries the group **in memory only** and syncs it
from the link's peers (a restart puts it down; joining keeps it). The app:
a code-less link opens the group to read — green banner, *join* button,
composer *"join to post"*; a kicked member of a public group reads it the
same way. The bridge answers anyone without NIP-42 as a fresh key, so it
serves public groups and nothing else; NIP-11 says `auth_required: false`.
Driven in two windows: Bob read Alice's welcome (her name too), joined,
the keyboard stayed in the composer. **Not yet:** a reader's files (the
blob gate should pass — the reader's binding sits in the serving node's
book for the session — untested); putting a read group down without a
restart.
**Step 4 built (2026-10-25):** the app publishes the person's own kind 0
to Damus, nos.lol and Primal (`hit_zap::PUBLIC_RELAYS`, `public.rs`:
connect, `EVENT`, wait for `OK`, all three at once) — at start, on each
name or address change, and every ten minutes to a relay that has not
taken the current one (`published.json`). The *you* dialog has a
**public profile** list (comma-separated, empty for none; the line under
the name says it is public when it is). **Not to the anchors**, against
the plan: they hold members' profiles by sync already. Test windows and
the by-hand recipes set `HIT_PUBLIC_RELAYS=none` — a throwaway profile on
Damus is litter. Tested against the fake relay (once only across a
restart; none means none) and driven in one window against the fake:
the dialog saved two relays, the live one took the profile, the dead one
was logged for retry. **Not yet:** Primal showing a real name — that is
the done-when's, with a real install.

**What to build, in order:**

1. **A public group** — NIP-29's own words on `39000`: no `private` tag
   (anyone may read) and no `closed` tag (anyone may join), where today
   the anchor's bridge always writes both. **Private by default.** Making
   it public is one choice with one sentence under it — *"Public: anyone
   can find this group, read its open channels and join"* — offered when
   the group is created (Private / Public, Private selected) and as a
   single switch in group settings, owner only. Signed by the group key
   like the rest of the state, so it is in the fold and every peer agrees.
2. **Join without an invite** — a `9021` with no `code` is honoured when
   the group is open; bans still hold. A link that names the group and its
   anchors and carries no code: `hit-enter:group?…`.
   **The newcomer wait:** a `newcomer_wait` seconds row in `39000`
   metadata, changed by a `9002` edit with the same permission as
   slowmode. For a public group the fold uses **the larger of it and 300
   seconds** — the floor is in the rule, not the UI, so no edit and no
   client gets under it; a private group has no floor (0 by default).
   Enforced like slowmode: a message dated less than the wait after its
   author's join is refused by every peer. Members the snapshot names have
   no join time left — they are not newcomers. The composer says *"You
   can post in 4:42"* rather than failing, and reading is open the whole
   time.
3. **Read before joining** — for a public group, `may_read` lets anyone
   read its state, the channels every member can view, and the profiles
   of its members (they are public anyway, point 2). Private channels,
   actions and invites stay members-only. A reader syncs from the anchor;
   **live gossip only after joining** — a stranger in the swarm is a
   scale question for later. The anchor's `wss://` bridge serves a public
   group without NIP-42, so Chachi, 0xchat and a web page can read it.
4. **Profiles to public relays** — kind 0 published by the app to a few
   public relays (default list in code, editable) and the group's
   anchors' `wss://`. **This is the app's first outbound WebSocket outside
   zaps**: it reuses `hit-zap`'s relay client, and the rule becomes
   "iroh owns the *chat plane*; public relays are for publishing what is
   public anyway — profiles, public group announcements, zap receipts."
5. **Found: announcements and a "browse communities" list** — the group's
   signed `39000` (name, about, member count, its link) published to the
   same public relays; the app's list reads them back, newest and largest
   first. Plain on purpose: search and ranking come later.
6. **The DM inbox, on by default, with limits** — a new person names
   their groups' anchors in their `10050` at once (today opt-in). An
   anchor drops a wrap **7 days after it received it** — not by
   `created_at`, which NIP-59 randomizes up to two days back, so the
   store needs a *received at* column (a new migration) — and keeps at
   most **10 MB per recipient**, dropping the oldest. The recipient's own
   devices keep their copy forever. Worst case at Unturned scale (5,000
   people × 20 DMs a day × 2 KB × 7 days): **~1.4 GB**.

**Done when:** a stranger with a fresh install finds the group in
*browse communities* — or opens its link — reads `#general` before
joining, joins with one click and posts at once; a moderator bans them and
every peer drops what they posted after; Chachi reads the group through
the anchor's `wss://` without logging in; their name shows in Primal;
and a DM to someone whose app is closed waits at the anchor, arrives when
they open it, and is gone from the anchor seven days later (by test, on a
moved clock), while a flood of DMs to one person never holds more than
10 MB.

**Risks, said out loud:**
- **Raids.** Open joining means anyone can come in, and keys are free.
  The 5-minute floor stops a script that joins and posts at once, and
  gives moderators five minutes to see a wave of joins; it does not stop
  a patient one, who joins with many keys and waits — no fixed wait
  does. **The real answer is a raid mode** (with automod, after M13): the
  wait rises by itself, to 10–30 minutes, when joins spike, and falls
  back after.
- **The floor costs newcomers** — the owner, on joining the Unturned
  Discord: 5 minutes was already annoying. The countdown sits beside the
  open channels, so the wait is spent reading. Lowering the floor later
  is one constant. Slowmode and bans are per person; a raid is many people.
  Next after M13: automod — `DESIGN.md` §0's moderation row.
- **Public means public.** A public group's messages are on the open
  internet through the bridge and anyone's sync. The UI must say so when
  the switch is turned on, and turning it off cannot recall what was read.
- **The outbound WebSocket** is a real change to a rule kept since M2;
  it stays in one crate, publishes only, and never carries chat.

**Deliberately not:** live gossip for non-members; search, ranking or
categories in the browse list; a web client (the bridge plus existing
Nostr clients is the web, for now); voice over 8 (§0's own row).

**The relays** (owner: "the best available"): `wss://relay.damus.io`,
`wss://nos.lol`, `wss://relay.primal.net` — large, long-running, and the
ones S4 already saw receipts on — plus each group's anchor. A constant
list in one place, checked again before beta.5.

**Open questions:** none from the owner; the build will find some.

---

## 4. What that adds up to

| | sizes |
|---|---|
| M0–M2 — the idea proven (Nostr over iroh, a relay in every app) | M + S + L |
| M3–M4 — a usable app, v0.1.0 | L + L |
| M5–M8 — Discord-shaped: moderation, anchors, DMs, files | M + M + M + M |
| M9–M10 — voice and zaps, v1.0 | L + M |
| M11 — the business | M |

Roughly **one and a half times HIT_ENTER_2**. Less identity work than
HIT_ENTER_2 had (Nostr is the identity plane); much more distributed-systems
work (replication, authority, sync); and two genuinely new domains (real-time
audio, Lightning).

**The risk that matters most is still stopping.** The second is building the
fun parts first: voice and zaps are the demo, but a group that forgets who is
banned is not a product. The order above puts the boring, load-bearing parts
first on purpose.

---

## 5. If you want something to show people sooner

```
M0 → M1 → M2 → M3 → M4 → M10 (zaps)        → a zappable group chat
```

Zaps are only a medium milestone and they are the thing nobody else in
Discord-shaped chat has, so a "v0.2: zap a message" release is a strong
early story — **provided M6's `wss://` door exists** or zap receipts go to a
public relay. What it costs: no moderation (friends-only groups), no files,
no voice, and "no public relays" becomes "one public relay, for receipts".
Say that in the release notes.

---

## 6. Deliberately not in v1

- **Group E2EE** — MLS on Nostr (Marmot) is the real design. A milestone of
  its own after 1.0; until then private groups say "members and your anchor
  can read this".
- **Video and screen sharing.** Voice first; `iroh-live` exists for later.
- **A voice forwarding node (SFU)** — v1 voice is a mesh of 8.
- **Mobile** — a phone cannot be a relay (`DESIGN.md` §15 q7).
- **A browser client** — iroh in a browser is relay-only at best.
- **Bots, webhooks, an app platform** — the bloat you left Discord over.
- **A wallet, a token, ads.** Never, not "later".
- **Sparse replicas for 100k-member groups** (`DESIGN.md` §13).
- **The other bando apps** (YouTube, Reddit, SoundCloud, hakr-news…). They
  will reuse `hit-node`, `hit-files` and `hit-zap` almost unchanged — which
  is the best argument for keeping those crates clean now — but not one of
  them starts before v1.0.

---

## 7. The first thing to do

Build `callme`, call your friend, and zap them 21 sats from a script.

Those are S3 and S4: the two parts of this plan that nobody in the previous
HIT_ENTERs has touched, and the two most likely to change it. Then S1, and
watch two apps become two relays.
