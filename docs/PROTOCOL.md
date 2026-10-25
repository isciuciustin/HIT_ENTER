# HIT_ENTER — wire protocol and event kinds

Every event kind HIT_ENTER emits, and every rule a peer applies to it.
**Other Nostr software will see these events**, and closed code is a
promise users cannot check (`DESIGN.md` §15 q1), so this file is how anyone
inspects what leaves their machine. It changes in the same commit as the
code that changes the wire.

Standard kinds we use as specified (NIP-01 text, NIP-17 DMs, NIP-29 groups,
NIP-57 zaps) are listed when their milestone lands. This file covers what
is ours.

| kind | name | since | status |
|---|---|---|---|
| `0` | profile | M4 | NIP-01's, as specified; carried only between people who share a group (below) |
| `5` | withdrawal of one's own message or reaction | M4 | NIP-09's; our `h` and `channel` tags |
| `14` / `13` / `1059` | DM: rumor / seal / gift wrap | M7 | NIP-17 and NIP-59, as specified; only `1059` is ever stored or sent |
| `15` | DM file — the rumor inside a wrap | M8b | NIP-17's; AES-GCM, our `hit-blob:` address and `name` |
| `10050` | inbox list: the anchors that keep a person's DMs | M7 | NIP-17's kind; our `anchor` rows |
| `7` | reaction | M4 | NIP-25's; our `h` and `channel` tags |
| `20436` | presence — "I am here" | M4 | **ours, provisional**; ephemeral, gossip only |
| `9` | group chat message | M3 | NIP-29's kind; our `channel` tag |
| `9000` / `9001` | put / remove a member; `9001` also bans and unbans | M3, M5 | NIP-29's kinds; **our authority** and our `ban` / `unban` tags (below) |
| `9002` | change the group: name, channels and slowmode, overrides, roles | M5 | NIP-29's kind; our edit tags |
| `9005` | a moderator removing messages or reactions | M5 | NIP-29's kind; our `channel` tag |
| `9030` | mute a member until a time | M5 | **ours, provisional** |
| `9007` | the owner's half of a group's creation | M3 | NIP-29's kind, our meaning |
| `9009` | invite | M3 | NIP-29's kind; our `code` tag |
| `9021` / `9022` | join / leave | M3 | NIP-29's kinds |
| `39000`–`39003` | group state: metadata and channels, admins, members, roles | M3 | NIP-29's kinds, **signed by the group key, with `seq`** |
| `30436` | device binding / revocation | M1 | **provisional** — `DESIGN.md` §15 q4 |
| `9734` / `9735` | zap request / zap receipt | M10 | NIP-57's, as specified; the request goes to the recipient's Lightning address server, the receipt comes back on the group's anchor's relay — **neither is stored or synced as itself** |
| `9436` | a zap receipt carried into a group | M10 | **ours, provisional** (below) |
| `39436` | a public group announcing itself on public relays | M13 | **ours, provisional**; addressable, signed by the group key, **never stored or synced in a group** (below) |

Kind `1` was M2's test traffic and is no longer carried.

---

## Session — `hit/nostr/1` (M2)

One iroh ALPN, `hit/nostr/1`. A node is a relay and a client on it, with no
difference in code. Implemented in `crates/hit-node/src/relay.rs` (answering)
and `sync.rs` (dialling); framing in `crates/hit-proto`.

**Framing.** Every frame is a `u32` little-endian length, then that many
bytes of UTF-8 JSON. At most 1 MiB, checked before allocating. One QUIC
bi-directional stream per session.

**1. Bindings first, both ways.** The dialler's first frame is

```json
["HIT-BINDING", { …its kind 30436 binding… }]
```

The listener authenticates it against the `EndpointId` **read off the
QUIC connection** (steps 1–9 under kind 30436 below). If that fails, it sends
`["NOTICE", "refused: <why>"]`, finishes the stream and closes the
connection with code `403`. Otherwise it answers with its own `HIT-BINDING`,
and the dialler checks it the same way. NIP-42 `AUTH` is not used: a binding
authenticates a *device*, not a challenge.

**Authenticating is not authorizing.** Since M3 a session is not refused
for who the peer is: every request is judged per event, as the person the
binding named, by `hit-group` (below). A stranger gets a session in which
nothing is served and nothing it sends is stored — unless it hands over a
join an invite honours. That is how a joiner reaches a member. What a
stranger learns: the npub of the device it dialled (the listener's hello).

Each side keeps the other's binding if they share a group — offered again
when the session ends, since a session can make the dialler a member.

**2. NIP-01 and NIP-77**, unchanged:

| from the dialler | the listener answers |
|---|---|
| `EVENT` | `OK` — `true`, or `false` with `invalid:` (signature, binding) or `blocked:` (whatever `hit-group` refused: not a member, kind not carried, lacks a permission, not valid against the group's state, …) |
| `REQ` | the stored matches **the asker may read**, newest first, then `EOSE`. What it may not read is left out silently — the same answer as "nothing here". **One-shot**: no live subscription (live fan-out is gossip). `CLOSED` with `invalid:` for a malformed request |
| `NEG-OPEN` / `NEG-MSG` / `NEG-CLOSE` | NIP-77 negentropy over the filter, narrowed to what the asker may read; `NEG-ERR` if refused or malformed |
| `CLOSE` | ignored (subscriptions are one-shot) |
| anything else | `NOTICE` `error: unsupported message` |

**3. The dialler finishes its side; the listener finishes its own.**

A sync is: negentropy over every kind the node carries, **each side
narrowing its set to what the other may read** and leaving out the two
parties' own bindings (the hello carried them); `REQ` by id for what the
dialler lacks (in chunks of 200); `EVENT` for what the listener lacks. Both
directions apply events in dependency order — the group key's state, the
owner's `9007`, actions by time, bindings, messages — and what is refused is
retried while anything new gets in, since one second cannot order two
actions. Every event, from any direction, passes the same gate: signature,
then `hit-group`, then — for bindings — the checks below, then the store.

**Limits**, enforced by the listener: 8 filters per `REQ`; 500 events per
filter (a larger or missing `limit` becomes 500); 4 negentropy sessions per
stream; negentropy messages of 128 KiB before hex; subscription ids of 64
bytes. NIP-50 `search` is not supported and matches nothing.

**Replacement.** Kinds 0, 3 and 10000–19999 keep the newest per (kind,
pubkey); 30000–39999 per (kind, pubkey, `d`); 20000–29999 are never stored.
Newest `created_at` wins, a tie goes to the lower id — except that a group's
state (`39000`–`39003`) is ordered by **`seq` first**, and a revocation beats
a binding (below).

**What a node stores** (`hit_group::Groups::judge`): events of groups it
carries — created there, or joined — that `hit-group` accepts, and the
bindings of people it shares a group with. Nothing else. An event dated more
than 10 minutes ahead of the node's clock is refused.

---

## Groups (M3)

NIP-29's kinds with **our authority** (`DESIGN.md` §6, Design C): no relay
signs the state; the group key does, and admins' actions take effect on
every peer that can check them. Implemented in
`crates/hit-event/src/group.rs` (shapes) and `crates/hit-group` (meaning).

**The group's address is its public key, `G`.** Every group event carries
`["h", G]` (hex), except the group key's state, which carries `["d", G]`.

### State — signed by the group key

Four addressable events, `d` = `G`, signed by `G`, each with
`["seq", "<N>"]`. A snapshot is all four at one `seq`; whoever holds the
group key publishes all four together. **Per kind, the highest `seq` wins**,
whatever the clocks say. A state event not signed by the group it names is
not state.

| kind | tags |
|---|---|
| `39000` metadata | `["name", …]`, `["public"]` + `["open"]` or `["private"]` + `["closed"]` (M13, NIP-29's words — always one pair; **public only with `public` and without `private`**, so every earlier group, which wrote neither, stays private), `["newcomer_wait", seconds]` (M13; absent is 0, at most a day), `["channel", id, name, "text"\|"voice", slowmode]`… (slowmode in seconds, since M5; absent is 0), `["override", channel, role, allow, deny]`… (bits as decimal strings) |
| `39001` admins | `["p", owner, "owner"]`, and `["p", member, role…]` for every member holding roles beyond the default |
| `39002` members | `["p", member]`…, `["ban", person]`… and `["mute", member, until]`… (since M5) |
| `39003` roles | `["role", id, name, perms, position]`… — `perms` a decimal bitset, `position` higher = higher rank. The role every member holds is `member`; `owner` is reserved |

Permission bits are wire-stable (`crates/hit-group/src/perm.rs`, moved from
HIT_ENTER_2): `0` view, `1` send, `2` manage messages, `3` manage channels,
`4` create invite, `5` manage invites, `6` kick, `7` ban, `8` mute, `9` manage
roles, `10` manage group, `11` attach files. Unknown bits are dropped.

### People's events

| kind | tags | valid when (checked by every peer) |
|---|---|---|
| `9` | `["h", G]`, `["channel", id]` | the author is a member with *view* and *send* in an existing channel — and, a newcomer, dated at least the **newcomer wait** after their join: the group's `newcomer_wait`, and never under **300 s in a public group** (M13). Reactions too. Judged by the dates the author wrote, as slowmode — a patched client can date its join early |
| `9007` | `["h", G]` | signed by the person `39001` names as owner. **Only then are they the owner** — the group key cannot make someone the owner of something on its own, as a device cannot be bound without its own proof |
| `9000` | `["h", G]`, `["p", target, role…]` — the target ends up with **exactly** these roles | the actor is a member; the target is not the owner; adding needs *create invite*; changing roles needs *manage roles*, outranking every role added or removed, outranking the target, and holding every bit granted |
| `9001` | `["h", G]`, `["p", target]` | *kick*, and the actor outranks the target. Nobody acts on the owner, the owner included |
| `9001` + `["ban"]` | as above | *ban*, and outranks the target. The target need not be a member — banning someone who left keeps them out. A ban stops any join or put of the target until lifted |
| `9001` + `["unban"]` | as above | *ban*; the target is banned. Adds nobody |
| `9030` | `["h", G]`, `["p", target, until]` — `until` 0 lifts a mute | *mute*, outranking a member target; `until` after the action's own time |
| `9002` | `["h", G]` and one or more of: `["name", N]`; `["public"]` or `["private"]` (M13); `["newcomer_wait", seconds]` (M13); `["channel", id, name, kind, slowmode]` (create or replace); `["channel-delete", id]`; `["override", channel, role, allow, deny]` (0 and 0 removes it); `["role", id, name, perms, position]` (create or replace); `["role-delete", id]` | **every edit or none**, each judged against the state *before* the event: the name needs *manage group*; public or private **only the owner** — no role grants it; the newcomer wait *manage channels*, at most a day; channels and slowmode (≤ 6 h) *manage channels*; overrides *manage roles*, outranking the role, only channel bits survive, and granting nothing the actor lacks in that channel; roles *manage roles*, outranking the role before and after, position > 0, granting nothing the actor lacks; the default role only by the owner, at position 0, never deleted; deleting a role takes its assignments and overrides with it. Labels 1–64 characters |
| `9005` | `["h", G]`, `["channel", C]`, `["e", id]`… | *manage messages* in `C` — HIT_ENTER_2's rule, no rank. Removes only messages and reactions **in `G` and `C`**, by anyone |
| `9009` | `["h", G]`, `["code", hex]`, optional NIP-40 `["expiration", t]` | the author may *create invite* — checked again when a join uses it |
| `9021` | `["h", G]`, `["code", hex]` — or no `code`, to a public group (M13) | not yet a member, not banned; some `9009` with that code, dated no later than the join, unexpired at the join's time, by someone who may invite **at that point in the fold** — or, without a code, the group public **at that point in the fold**. Whoever joins this way is a **newcomer** until the next snapshot |
| `9022` | `["h", G]` | a member, not the owner |

### The fold — what every peer computes

`crates/hit-group/src/state.rs` has the full rules; in short:

1. The snapshot: each state kind at its highest `seq`. **Without all four
   there is no state**, and nothing in the group but the group key's own
   events is accepted.
2. The cutoff: the newest `created_at` among the four.
3. Actions (`9000`, `9001`, `9002`, `9030`, `9021`, `9022`) **dated at or after the cutoff**
   are applied in time order, each only if valid against the state before
   it. Within one second they are applied in id order until nothing more
   applies; across seconds the order is strict.
4. Actions dated before the cutoff are folded — not applied, not synced. A
   compaction is therefore dated **one second after** everything it folds.
   An action its signer had not seen, dated before it, is lost — visibly, and
   it can be redone. **An invite (`9009`) is folded only once it has expired
   *before the cutoff*** — not merely expired: a join is judged against the
   invite as it was at the join's time, so a peer that arrives later needs
   the expired invite to apply the joins it honoured, until a snapshot folds
   them (2026-10-24: a fresh anchor refused four members, and all they
   wrote, without it).
5. A peer stores an action only if folding it with everything it holds
   applies it.
6. **Silences** (M5). A kick, a ban or a leave silences its target from its
   own time until they are a member again; a mute, for its span. Every peer
   refuses the target's messages and reactions dated inside a silence, and
   **drops** any it took before the silencing action reached it — a ban
   arriving after the banned user's next message. A ban silences even a
   target who is not (yet) a member: in the same second as their join, the
   fold may apply the ban first.

### Enforcement, and its honest limits (M5)

- **A ban** means every honest peer stops storing, forwarding and showing
  the person's events, stops serving them the group, and refuses their
  joins. Their own client can still sign; nobody carries it.
- **A removal (`9005`) or withdrawal (`5`)** is applied where the target is
  held and **refuses the target if it arrives again**, so a peer that still
  holds it cannot push it back. Copies already on a disk elsewhere stay
  there.
- **Slowmode** is judged by each peer against the messages it holds: a
  message within `slowmode` seconds of another by the same author in the
  same channel is refused. It stops honest clients and bursts; **it does
  not stop a patched client that dates its messages apart**, since
  `created_at` is the sender's claim. Which of two too-close messages
  survives on a peer depends on which arrived first there.
- **A mute's end** is a time the moderator chose and every peer reads off
  the event; a patched client can post-date a message just past it, within
  the 10-minute clock skew every peer allows.
- **A removed person may read exactly one thing**: the action that removed
  them, so their own app learns it. Their gossip is cut the moment the
  removal lands, so it must come by sync.

### Who reads what

- A **member** reads the group's state, its live actions and invites, and
  the messages, reactions and withdrawals of every channel they can *view*. A non-member reads nothing
  of a private group — not its name, not its member list.
- **A public group (M13) reads to anyone** as to a plain member who sees
  only its *open* channels (those the default role can view; "mods only"
  ones stay members'): its state, its actions **and invites** — a
  reader's own node folds them to know who is a member, or it would refuse
  every message — and those channels' messages, reactions and
  withdrawals. Its members' **profiles** too; **not their bindings**, which
  name devices to dial. Readers sync; a group's gossip is still members'.
  The honest cost: an invite read while the group was public still works
  if it goes private before the invite expires.
- **Bindings and profiles** are read by the person they belong to and by
  people who share a group with them — and stored, by the same rule, only
  for them. Neither goes on a group's gossip: they belong to a person, not
  a group, and travel by sync. **An expired binding is not offered** — every
  peer's book refuses it.
- The same rule answers a `REQ`, builds a negentropy set, and decides what
  goes on gossip.

## Gossip — `hit/gossip/1/<G>` (M3)

Live fan-out, `iroh-gossip` 0.101 (spike S2). **One instance, and ALPN, per
group**: `hit/gossip/1/` followed by `G` in hex. Topic id: `G`'s 32 bytes.
Every payload is one event's JSON, verified like any other on arrival.

- **The gate is an endpoint hook** (`after_handshake`) that sees **every**
  connection on a gossip ALPN, *incoming and outgoing*. The connection's
  `EndpointId` is mapped to persons through the binding book; one must be a
  member of `G`. Otherwise the connection is closed with `403`. Gating only
  incoming connections is not enough: gossip dials its neighbours too.
- When `G`'s state changes, live gossip connections whose device no longer
  speaks for a member are closed (QUIC code `403`, reason `revoked`).
- A node joins `G`'s gossip only once its own fold says it is a member.
- What goes on gossip: `G`'s state, actions, the owner's `9007`, and
  messages **in channels every member can view**. A private channel's
  messages travel by sync only, until it has its own instance (open).

## In a channel — kinds `9`, `7`, `5` (M3, M4)

All three carry `["h", G]` and `["channel", C]`, and are read only by
members who can view `C` — by `REQ`, negentropy and gossip alike. On
gossip only when **every** member can view `C`.

- **`7`, a reaction** (NIP-25): exactly one `["e", <message id>]`, plus
  `["p", <its author>]` and `["k", "9"]`; the content is the emoji, 1–32
  characters. Judged as a message (`VIEW_CHANNEL` and `SEND_MESSAGES`):
  there is no separate permission to react. HIT_ENTER's own green
  reactions are NIP-30-style shortcodes — `:hit_gg:`, `:hit_lol:`,
  `:hit_hype:`, `:hit_love:`, `:hit_fire:`, `:hit_pog:`, `:hit_sad:`,
  `:hit_rage:`, `:hit_wow:`, `:hit_clap:` — **without** NIP-30's `emoji`
  tag (the pictures ship in the app; there is no URL to name). Other Nostr
  apps show the shortcode as text.
- **`5`, a withdrawal** (NIP-09): one or more `["e", <id>]`. Stored from any
  member who can view `C`; **applied only to its author's own events, and
  only to kinds `9` and `7`**. A withdrawal naming somebody else's message
  removes nothing, and one naming a group action (a join, say) is ignored
  for it — the fold is not NIP-09's to rewrite. Once held, it also refuses
  the withdrawn event if it arrives again, so a peer that still has it
  cannot push it back. A moderator removing *someone else's* message is
  M5's `9005`, not this.
- What a withdrawal **cannot** do, said plainly in the app: undo what a
  member already saw, or a copy someone kept outside the app.

## Kind 20436 — presence (M4)

`["h", G]`, empty content, signed by the person. **Ephemeral** (NIP-01's
20000–29999): never stored, never synced, sent only on `G`'s gossip, where
only members are admitted. Refused from a non-member, and refused if
dated more than 120 s ago (`hit_group::PRESENCE_MAX_AGE`) — an old one
replayed is the cheapest way to say someone is here who is not. The app
sends one every 30 s and counts a person as online for 75 s after the last
one, or after a device of theirs completed a sync with ours.

## Voice (M9) — kind 20437 and `hit/voice/1`

A voice channel is a channel whose `39000` tag says `"voice"`. Nothing is
stored in it; two things go over the wire.

**Kind 20437 — voice presence.** `["h", G]`, `["channel", id]`,
`["state", "in" | "out"]`, empty content, signed by the person.
**Ephemeral**, on `G`'s gossip only, like 20436 — and judged like it, plus:
the channel must exist and be a voice channel, and the author must be a
member who may *view* it (`VIEW_CHANNEL`). Refused if older than 120 s. A
node in a call sends `in` every 5 s and `out` when it leaves (and when the
app quits); everyone counts a person as in the channel for 15 s after
their last `in`. A sibling of 20436 rather than a field in it: presence is
"this app is running", this is "this person is in that room", and the two
change at different rates.

**`hit/voice/1` — a voice channel's call.** One QUIC connection per pair of people in
the call (a mesh, at most 8 people — `hit_event::group::MAX_VOICE`). Of
each pair, **the device with the lower `EndpointId` dials**; nobody
coordinates.

1. The dialler opens one bi-stream and writes a JSON line:
   `{"group": "<hex>", "channel": "<id>"}` (a join line is at most 8192
   bytes — room for a 1:1 call's binding, below).
2. The listener **gates** it and answers `{"ok": true}` or
   `{"ok": false, "why": "…"}`, then closes on a refusal. The gate: the
   listener is in that call; the dialling device — **read from the
   connection, never from the line** — is bound to a person who is a member
   allowed to view the channel; and the call has room.
3. Audio then goes as **QUIC datagrams**, one per 20 ms frame:

   | byte | |
   |---|---|
   | 0 | version, `1` |
   | 1–2 | sequence number, big-endian `u16`, wrapping |
   | 3 | flags: bit 0 = the sender's voice activity said *speech* |
   | 4… | one Opus packet: 48 kHz mono, 20 ms, VoIP mode, 32 kbit/s, in-band FEC on |

   A datagram that does not parse is dropped. Nothing is retransmitted; a
   lost frame is recovered from the next one's FEC, or concealed.

**A 1:1 call (from a DM)** uses the same ALPN and the same datagrams, with
no presence and no group. The caller dials **every device** the callee has
a binding for and writes `{"dm": <the caller's binding, kind 30436>}` — the
callee may share no group with them and so hold none. The callee:

1. **Authenticates** the binding against the connection's `EndpointId`
   (`BindingBook::authenticate`: it must name this device, verify, and not
   be superseded by a revocation the callee holds).
2. **Decides whether this person may ring at all**: someone the callee
   shares a group with, or has **written to** in a DM (a DM the callee
   *sent*). Not a stranger who learned a device id; not someone who merely
   wrote to them — consent runs one way. Otherwise `"why": "not someone who
   may call this person"`, and the app never rings.
3. Answers `"busy"` if in a call or already ringing; otherwise **rings**
   for up to 45 s and answers when the person does: `{"ok": true}`, or
   `"declined"` / `"no answer"`. The caller closing the connection stops
   the ring.

The first device to answer `ok` is the call; the caller drops its other
connections, which stops them ringing. When either side's connection goes,
the call ends. A callee whose devices the caller holds no binding for is
unreachable; with no group in common, the bindings come from DMs and
contact links (below).

## Reaching someone with no group in common (M9)

Bindings normally travel between people who share a group, and
`hit-group` stores no one else's. Two things carry them outside a group:

- **Every DM carries its sender's device binding**, sealed in the rumor
  as `["device", <kind 30436 as JSON>]` (kinds 14 and 15). The recipient
  keeps it only if its author is the DM's sender, after the binding
  book's checks (signature, device proof, expiry, replacement) — and
  stores it, so it survives a restart. Only the recipient sees it; the
  seal hides it from an anchor. Every DM refreshes it, so people who write
  to each other stay reachable past the binding's 7-day life.
- **A contact link** — `hit-enter:contact?p=<npub>&d=<device hex>[@<ip:port>]…`
  (`hit_proto::contact`) — names a person and their devices, and no
  binding (that would expire within days). Opening one dials each device
  once over `hit/nostr/1`: the hello's binding, checked against the
  connection, proves the device speaks for the person named, and is kept
  as above. It lets the holder write to them; it does not let them ring
  their app (only someone they wrote to may). It reveals their device ids,
  so whoever holds it can learn their IP address — the app says so.

So, with no group in common: Carol opens Bob's contact link and writes; her
DM reaches his device directly and carries her binding. Her call is refused
until Bob writes back; then it rings. Names stay npubs: profiles (kind 0)
are still shared only within groups.

**Who is heard** is decided by each listener, not the speaker: a person's
audio is played only while they may `SEND_MESSAGES` in the channel and are
not muted by a moderator (`9030`). Checked at the handshake and **every
second** after, so a mute or a ban mid-call silences them within a second
on every peer that has it. A person who may view but not speak can join
and listen; their app knows it and sends nothing.

**A voice channel not everyone can view** keeps who is in it from those
who cannot see it. Its presence never goes on gossip — gossip reaches every
member, and `hit_group::Groups::gossip_group` keeps a restricted channel's
events off it. Instead the announcing node hands each 20437 directly to
every device of every member who may view the channel, over `hit/nostr/1`
(one `EVENT`, the way a DM reaches its recipient), on every heartbeat; each
receiver judges it like any event. The arithmetic: a person in the room
opens one short session per eligible device every 5 s — ten eligible
devices is two a second, fine for a staff room, wasteful for a big one.
A room is restricted when not every *current* member may view it; the app
marks a channel 🔒 when the Member role is denied `VIEW_CHANNEL`.

**What it does not hide.** The connection is direct when it can be, so the
people in a call learn each other's IP addresses — as the group already
can (`DESIGN.md` §8). Audio is encrypted per hop by QUIC, which in a mesh is
end to end. Voice presence tells every member who is in which room.

## Kind 39436 — a public group's announcement (M13)

How a stranger finds a public group: *browse communities* reads these from
the public relays (the same list a profile goes to). Addressable, `d`
empty, **signed by the group key** — its `pubkey` is the group, so only a
holder of the key can announce it.

| Tag | Meaning |
|---|---|
| `["d", ""]` | one per group |
| `["name", N]` | the group's name, 1–64 characters |
| `["members", n]` | its member count — **the group's own word**, shown, never trusted |
| `["link", L]` | its public link (`hit-enter:join?g=…&p=…`, no `c`) |
| `["private"]` | instead of the three above: withdrawn — the group went private |

**A reader drops** anything with a bad signature, a `d` that is not empty,
a name empty or over 64 characters, a link over 2 KB, or a link naming
another group or carrying an invite code. Each group's newest wins; **a
withdrawal wins a tie** (hiding a group by mistake is the cheaper error).
The list is largest first, then newest, at most 200.

**Who sends it:** any app holding the group key — today the owner's —
when the group is made public, renamed, or made private (the withdrawal),
and every ten minutes if its words changed (a member count). Each is
dated after the last one sent: a relay keeping one per group settles a
same-second tie by NIP-01's lowest id. **A group never announced is never
mentioned** — no withdrawal for a group that was always private, which
would publish its existence. `published.json` records what went where.

**Not NIP-29's `39000`:** ours is the whole state (members, roles) and
for members' nodes; and a list of `39000`s would catch every NIP-29
relay's groups, which this app cannot join. Implemented in
`crates/hit-event/src/announce.rs`.

## Kind 0 — profile (M4)

NIP-01's metadata event, unchanged: `content` is a JSON object, and the app
reads `name` (else `display_name`), trimmed, control characters dropped, at
most 64 characters. Every other field is kept as it was when the app
writes a new name — a person who brings an existing Nostr identity keeps
its `picture`, `about` and `lud16`. Replaceable (NIP-01), so a new one
replaces the old. **The app dates a new profile at least one second after
the one it replaces**: two in the same second are settled by NIP-01's
lowest-id rule, which is not "the newest" (found by a test, M4).

**An anchor says so** (M6): its kind 0 carries NIP-24's `"bot": true`.
Apps show it as *anchor*, and invite links name its devices (below).

**Zaps (M10) add two fields:**

- `lud16` (NIP-57's, as everyone writes it) and **`zap_pubkey`** (ours):
  the hex key the person's Lightning address server signs zap receipts
  with — its LNURL `nostrPubkey`, read by the person's own app when they
  set the address. **Every peer counts a receipt for them only if it is
  signed by this key.** NIP-57 has the sender fetch it over HTTPS; a third
  member cannot (no HTTP in the chat plane), so the recipient says it,
  signed. A profile written by another client without it gets zaps that
  show nowhere until their HIT_ENTER app saves the address again.
- An anchor's **`relay`** (ours): its public `wss://` URL, where a
  Lightning address server can deliver a receipt for its groups. Apps
  name only these relays in a zap request (below). `ws://` is accepted
  only to loopback (tests).

**Published to public relays (M13)**, unchanged: the app sends a person's
own kind 0 — and nothing else of theirs — to `wss://relay.damus.io`,
`wss://nos.lol` and `wss://relay.primal.net` by default
(`hit_zap::PUBLIC_RELAYS`), or to the list the person set (empty: none),
so other Nostr apps show their name. Publish only: no subscription, no
chat. Sent at start, on each change, and every ten minutes to a relay that
has not said `OK` to the current one (`published.json` in the data
directory records who has). **Not to the group's anchors**, though the
plan said so: an anchor already holds its members' profiles by sync, and
its bridge serves them. An anchor's own kind 0 is never published.
`HIT_PUBLIC_RELAYS=none` turns it off on a device (windows driven in
tests); a comma-separated list replaces the defaults. A profile a relay
already has stays there — Nostr has no recall. Implemented in
`crates/hit-event/src/profile.rs` and `hit-zap/src/public.rs`.

## Invite links (M3)

```text
hit-enter:join?g=<G as npub>&c=<code>&p=<device hex>[@<ip:port>]&p=…
```

At most 8 `p`, **the group's anchors' devices first** (M6: members whose
profile says `bot`), then the inviter's: an anchor is there at 4am. A
joiner tries them in order and reports every refusal. `@ip:port` is for
relays off (a LAN, a test). The URL
scheme `hit-enter:` is registered with the desktop by the app (M4); a link
arriving that way opens the join dialog and never joins by itself. A link is a
capability: anyone holding it can join until the invite expires. It holds
no key and no member list. Implemented in `crates/hit-proto/src/invite.rs`.

---

## Kind 30436 — device binding

*"npub P speaks from, and can be dialled at, iroh device D."* Signed by the
person. Addressable (NIP-01: kinds 30000–39999), so per (pubkey, `d`) only
the newest event counts. Implemented in `crates/hit-event/src/binding.rs`.

```json
{
  "kind": 30436,
  "pubkey": "<person, hex>",
  "created_at": 1800000000,
  "content": "",
  "tags": [
    ["d", "<device EndpointId, 64 lowercase hex chars>"],
    ["expiration", "1800604800"],
    ["proof", "<ed25519 signature by the device key, 128 hex chars>"],
    ["alt", "HIT_ENTER device binding"]
  ]
}
```

| tag | meaning |
|---|---|
| `d` | the device's iroh `EndpointId` (an ed25519 public key), **lowercase hex only**. iroh also parses base32; we refuse it, because two spellings of one device would be two replaceable slots, and a revocation in one would leave a binding alive in the other |
| `expiration` | NIP-40. **Required** on a binding. We sign 7 days; a verifier refuses more than 30 |
| `proof` | the device key's ed25519 signature over the bytes `hit-enter/device-binding/1:` ‖ the person's 32-byte x-only public key. Proves the device agreed to speak as this person, so nobody can publish "my npub is at *your* device" |
| `alt` | NIP-31, so other clients show something readable |

### Revocation

The same kind and `d`, a newer `created_at`, a `revoked` tag, and **no
`expiration` and no `proof`**: a lost device cannot sign, so revoking must not
need it, and a revocation must outlive the binding it revokes.

```json
{ "kind": 30436, "tags": [["d", "<device>"], ["revoked"], ["alt", "HIT_ENTER device binding"]], "content": "" }
```

A newer binding for the same device after a revocation re-admits it. That
takes the person's key, so it is a deliberate act — and it is also why
revoking a device does **not** protect a person whose nsec was on that
device: whoever holds the nsec can sign a new binding. Device revocation is
for a device that is lost, sold or retired, not for a stolen key. Nostr has
no key rotation; that is `DESIGN.md` §3's problem, not this event's.

### Verification — what every peer does

In this order; the first failure refuses the event:

1. kind is `30436`;
2. NIP-01 id and Schnorr signature verify;
3. `created_at` is at most 10 minutes in the future;
4. `d` parses as an `EndpointId` and is its canonical lowercase hex;
5. if `revoked`: stop here — a valid revocation;
6. `expiration` present, in the future, and at most 30 days after `created_at`;
7. `proof` verifies under the key in `d`.

**To authenticate a connection** (the first frame on `hit/nostr/1`,
`DESIGN.md` §4), additionally:

8. `d` equals the `EndpointId` **read off the iroh connection** — never one
   from a message body;
9. the newest event the peer holds for (pubkey, `d`), after storing this
   one, is not a revocation and passes 1–8.

There is no entry point that answers "who is this" from a signature alone.

### Replacement

Newest `created_at` wins (NIP-01). On a tie: **a revocation beats a
binding**, then the lower event id wins. So a re-sign and a revocation made
in the same second cannot leave a revoked device alive.

### Renewal

A device re-signs its own binding once less than half its lifetime is left
(`needs_renewal`). Expiry is the backstop for peers that never hear a
revocation, which is why it is short.

---

## Files (M8)

A file is a kind 9 message (or an addition to one) with a NIP-92 `imeta`
tag per file, at most 10:

```text
["imeta", "url hit-blob:<blake3 hex>", "m <media type>", "x <sha256 hex>",
          "size <bytes>", "name <file name>"]
```

The BLAKE3 hash is what iroh fetches and verifies by (`/iroh-bytes/4`,
`iroh-blobs` 0.103); the SHA-256 is NIP-94's `x`, so an anchor could mirror
to a Blossom server later. Limits, checked by every peer as part of judging
the message (one bad `imeta` and the message is refused): both hashes 64
lowercase hex, 1 byte to 50 MB, a name of 1–255 characters with no `/`, `\`
or NUL, a type of at most 128. **No provider list**: anyone who may read the
message may be asked for its files.

**Serving** `/iroh-bytes/4`, two gates: the connecting device must speak for
someone (a binding names it — read off the connection); and each requested
hash must be attached to a held message one of those persons **may read**
(`hit-group`'s one rule) — a private channel's files are as private as the
channel. `get`, `get_many` (all or nothing) and `observe` are asked about;
`push` is refused. A stranger's probe never reaches the store.

**Fetching**: the whole blob (`execute_get`, full range — never
`Remote::fetch`), verified by hash, from the first of the message's author,
the group's anchors and its other members to answer — dialled all at once,
so an uploader who has gone costs nothing.

**Keeping**: a member's app keeps its own files, then the most recently used
up to 1 GB; an anchor keeps every file any held message carries. Nothing is
deleted directly: the store's collector asks what to keep before each sweep,
read fresh from the event store, so a withdrawn, removed or silenced
message's files go with it, eventually.

## DMs — NIP-17 (M7)

As NIP-17 specifies, with `nostr`'s NIP-44 and NIP-59 doing every byte of
cryptography: a kind 14 **rumor** (unsigned; `["p", recipient]`; the text)
is **sealed** (kind 13, NIP-44 to the recipient, signed by the sender) and
**gift-wrapped** (kind 1059, NIP-44 to the recipient, signed by a
throwaway key, `created_at` blurred up to two days back, `["p",
recipient]`). The sender wraps the same rumor to themselves too. A rumor's
id is the message's identity in both wraps.

**Who keeps a wrap** (`hit-group`): its recipient (the one `p`), and the
anchors their **inbox list** names. Nobody else — not the members of a
group they share, not the sender's anchor. The sender keeps the wrap
addressed to the other person too, as an outbox, until it is delivered.
Size cap: 64 KiB of content. **Who may be handed one**: the same two. A
wrap is never put on a group's gossip.

**How long, and how much (M13)** — every node, one rule, though only an
anchor and a sender ever hold wraps addressed to someone else:

- A wrap addressed to someone else is dropped **7 days after it arrived**
  here (`received_at`, a store column since migration 0005 — not
  `created_at`, which NIP-59 blurs up to two days back). Checked at most
  once a minute by the sync loop. Your own DMs are kept for good.
- A node **refuses a wrap for someone else dated over 7 days ago**
  (`Stale`): the recipient's devices keep theirs forever and would hand an
  expired wrap straight back. A wrap's date is at or before its sending,
  so the anchor's own copy always outlives this; a wrap first reaching an
  anchor 5–7 days late may be refused there.
- At most **10 MB of wraps per recipient** (`INBOX_CAP_BYTES`, by JSON
  length), checked as each arrives: the earliest to arrive go first.
  Worst case at Unturned scale (5,000 people × 20 DMs a day × 2 KB × 7
  days): ~1.4 GB.
- The sender's outbox copy for the other person follows the same week:
  with no inbox anchor between them, a DM must be delivered within 7 days
  of sending, or only the sender's own copy remains.

**The inbox list** (kind 10050, replaceable, a person's own like their
profile: kept and served between people who share a group). **On by
default since M13:** the app keeps it naming exactly the anchors of the
groups its person is in (checked every 30 s; a new list only on change),
unless they turned it off, which `app.json` remembers. Format:
`["anchor", <anchor hex>, <device id>]` per anchor. The device id is how a
sender who shares no group with the anchor dials it. NIP-17's `["relay",
url]` rows are for public relays, which nothing here publishes to yet.

**Delivery:** on send, the wrap is handed straight to the recipient's
devices and to their inbox anchors' devices; the sync loop retries, and
also syncs with the inbox anchors named by everyone we share a group with,
so a DM sent while everyone was away reaches one. An anchor learns that
*someone* wrote to the person it serves, and when (blurred); not who, not
what (`crates/hit-node/tests/m7_done_when.rs` scans its database bytes).

**Files in a DM (M8b)** — NIP-17's kind 15 rumor, sealed and wrapped like
a kind 14. The file is encrypted on the sender's side with **AES-256-GCM**
(RustCrypto's `aes-gcm`), a fresh 32-byte key and 12-byte nonce per file;
only the ciphertext is stored or sent, as a blob like any file. The rumor:
content `hit-blob:<blake3 of the ciphertext>`; `["file-type", type]`,
`["encryption-algorithm", "aes-gcm"]`, `["decryption-key", hex]`,
`["decryption-nonce", hex]`, `["x", sha256 of the ciphertext]`, `["ox",
sha256 of the original]` (checked after decrypting), `["size", bytes]`, and
our `["name", file name]`. A node serves the ciphertext to **the two people
only** — recorded by their own nodes when they send or receive it (a
`blob_grants` table), since no group says who may read a sealed message. An
anchor keeping the wrap cannot open it, so it cannot know the file: a DM
file comes from the sender's devices or the recipient's own, not an anchor
— the honest limit of M8b. The window is never given the key; it asks the
Rust side for the decrypted bytes.

**Limits:** you can DM someone you share a group with (you need their
bindings); DMs to and from other Nostr apps need opt-in publishing to
public relays — not built. An anchor stores wraps for those it serves from
anyone who can reach it: rate limits are later.

## Zaps (M10) — NIP-57, and kind 9436

Someone zaps a message or a file (both kind 9). What travels, in order
(`DESIGN.md` §11; the code is `hit-zap`, the only crate with an HTTP or
WebSocket client):

1. **Quote.** The sender's app fetches the recipient's `lud16` LNURL-pay
   endpoint (HTTPS; `http://` only to loopback) and refuses it unless
   `allowsNostr` and its `nostrPubkey` equal the recipient's profile's
   `zap_pubkey` — otherwise the zap would be paid and counted by nobody.
2. **Zap request, kind 9734** — NIP-57's, signed by the sender, or by a
   throwaway key with `["anon"]`: `["p", recipient]`, `["e", message]`,
   `["amount", msat]`, `["lnurl", bech32]`, and `["relays", …]` naming
   **only the group's anchors' `relay`s** — never a public relay, so a zap
   in a group does not announce it. A group with no such anchor cannot be
   zapped from the app (said in the dialog). The content is the comment,
   at most 280 characters.
3. **Invoice.** The callback's bolt11 must be for exactly the amount —
   checked before anything pays it. NIP-57 also asks that it commit to the
   request (description hash = SHA-256 of its JSON); **we do not require
   it** (2026-10-17): Primal's invoices carry one fixed hash whatever the
   request, and refusing them refused every zap to a Primal user. A
   mismatch is logged and paid.
4. **Payment**, through the sender's own wallet over NIP-47 (its relay,
   NIP-44 if its info event lists it, else NIP-04) — or by QR code.
5. **Receipt, kind 9735**, published by the address server to the
   anchor's relay. **The anchor accepts a 9735 without NIP-42**: it checks
   it (step 6) and carries it in; it does not keep or serve the 9735
   itself. The sender's app also reads any relay that serves it and
   carries what it finds.

**Kind 9436 — a receipt carried into a group.** Signed by the member (or
anchor) who brings it: `["h", G]`, `["channel", C]` (the zapped
message's), `["e", message]`, `["p", recipient]`, `["k", "9735"]`; the
content is the receipt's JSON, at most 16 kB. A channel kind: read,
gossiped and synced as a reaction is. **Every peer stores one when** its
author can view `C` and the receipt inside passes NIP-57's checks: a valid
signature; a `description` that is a validly signed 9734; the bolt11's
amount equal to its `amount`; the receipt's `p` and `e` equal to the
request's and to the carrier's. **Not** the bolt11's description hash
against the `description` (NIP-57 asks it; Primal's invoices fail it, see
step 3) — the signer check below is what a total rests on. **A peer counts it toward a total only when** the message's
author is the recipient and the receipt's signer is the recipient's
`zap_pubkey` — checked when counting, because a profile may arrive after
the carrier. One zap per receipt id, however many members carried it. The
bolt11's own signature is not checked: NIP-57 does not ask it, and a
receipt is the provider's word either way.

**The fee** (1%, `DESIGN.md` §11) is a plain LNURL-pay payment to
HIT_ENTER's address with the LUD-12 comment `HIT_ENTER fee`: no 9734, no
9735, no message id, no pubkey. No fee address is set yet, so no fee is
charged (`hit-zap/src/fee.rs`).

## An anchor's `ws://` — what NIP-29 clients see (M6)

An anchor may also listen on `ws://` (with a TLS proxy in front for
`wss://`), as an ordinary NIP-01 relay. It is a **bridge** onto the same
node, with the same rules:

- **NIP-11** on a plain `GET`: `supported_nips` `[1, 11, 29, 42, 57]`,
  `pubkey` the anchor's own, `limitation.auth_required: true`.
- **NIP-42 first.** The relay sends `AUTH` on connect; a `REQ` before a
  valid one gets `CLOSED` `auth-required:`. A valid AUTH is a kind 22242
  naming the challenge, at most 10 minutes from now, and — if the operator
  set the public URL — naming that relay. From then on the client is
  answered **exactly as `hit/nostr/1` answers that npub**: a member reads
  what a member may, a stranger nothing.
- **Group state is translated, not passed through.** The group key's own
  `39000`–`39003` are never served here. Instead the anchor signs, with its
  own key, NIP-29-shaped ones for each group the reader is a member of:
  `d` = the group key in hex (the value of every message's `h`); `39000`
  with `name`, `about`, `private`, `closed`; `39001` the owner (`owner`)
  and role holders (the role's name); `39002` every member (and their
  name); `39003` the role names. Served only over `ws://`; never stored, never
  synced — a peer of the mesh would rightly refuse them.
- **Messages, reactions and the rest pass through** as signed, per the
  reader's rights, and live subscriptions get new ones.
- **Posting from a NIP-29 client is refused for now**: its kind 9 has no
  `channel` tag, which the group requires — the `OK` says so.
- **Zap receipts (M10) are taken without authentication** — see Zaps.

## Open

- **Posting from NIP-29 clients.** Their messages have no `channel`; a
  "no channel means the first channel" rule would let them post, and would
  change every peer's judgement — a protocol decision, not an anchor's.
- **NIP-29 interoperability.** The kinds are NIP-29's; the authority is
  not. M6's anchor translates for reading (above). A NIP-29 client would expect a relay key to sign `39000`–`39003` and
  would not know `seq`, `channel`, `override`, `role`, or what our `9007`
  means. Exposing a group to such clients through an anchor (`DESIGN.md`
  §2) needs a translation, not a pass-through.
- **The `channel` tag** is ours and multi-letter, so a NIP-01 filter cannot
  select by it; nodes filter after the query.
- **Messages from someone later removed**, dated *before* the removal, are
  judged by membership now, so a late one is refused by a peer that has the
  removal and kept by one that took it earlier. Messages dated *after* it
  are dropped everywhere (silences, M5). Judging by membership at the
  message's time would close this — and let a removed person backdate
  messages into the past; M5 chose the second risk away.
- **9030** is ours; NIP-29 has no mute. It may want proposing.
- **9436** (a carried zap receipt) is ours and unchecked against the NIP
  registry. A NIP-29 client reading through an anchor sees it as an
  unknown kind; serving the 9735 inside it to them would be a translation.
- **`zap_pubkey`** is ours. A provider that rotates its key makes old
  zaps to that person stop counting until they save their address again.

- **Actions in the same second.** The fold orders actions by
  `(created_at, id)`, so two from one person in one second — a toggle and
  its undo — applied in id order, which is chance. A node now dates its
  own edit, put, remove or mute **one second after its own last action of
  the same kind** in that group, if that one is not already older. Only its
  own, and only the same kind: dating an invite after the group's creation
  put it ahead of the join that used it, and every join failed. Two people
  acting in the same second are still ordered by id.
- **20436** is ours, chosen as 30436's ephemeral twin, and **20437** (voice
  presence, M9) its sibling; neither is checked against the NIP registry.
- **`hit/voice/1` has no codec negotiation** and no way to say "I am on a
  bad link, send less". A version 2 would add both; version 1 is Opus at
  32 kbit/s, which is fine on anything that can hold a call.
- **The kind number.** 30436 is clear of every kind `nostr` 0.45 knows, not
  checked against the full NIP registry. Check before v0.1.0, and decide
  whether to propose the binding as a NIP (`DESIGN.md` §15 q4).
- **The device proof's message format** is ours. If this becomes a NIP, it
  should be argued about there.
