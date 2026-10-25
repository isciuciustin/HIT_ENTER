# HIT_ENTER 1.0 — System design

**The next Discord, for public communities**, with **no server anybody has
to run**: group chats with channels, DMs, text, files, voice channels, and
zaps. Open source (MIT). **Nostr** is the identity and the data model; **iroh** is the wire.
Every running copy of the app is a relay for the groups its user is in.

This is the third HIT_ENTER, and it is a different product from both of the
first two:

| | HIT_ENTER (the original) | HIT_ENTER_2 | **HIT_ENTER 1.0** |
|---|---|---|---|
| who is a person | a row in one space's database | an account on *our* identity server | **a Nostr keypair nobody issued** |
| where a group lives | on its host's machine, only | on its host (home or paid) | **on every member's machine** |
| who is the authority | the host process | the host process | **signed events every peer checks** |
| host offline | the space is down | the space is down | **the group is up while any member is** |
| licence | MIT OR Apache-2.0 | proprietary | **MIT** (§15 q1) |
| DMs, voice, money between users | no | no | **yes** |

The pages below say what that costs. It costs a lot; the price is mostly in
§6, "authority without a server".

---

## 0. The north star — public communities (2026-10-22)

**The owner's direction, replacing "privacy-first" as the headline:**
HIT_ENTER is the next Discord, a better one, for **public communities**
running on decentralized software (Nostr + iroh). The dream it is measured
against: **the Unturned Discord community runs on HIT_ENTER.**

What this keeps: no server anybody has to run, nobody can switch a
community off, keys people own, zaps, open source. DMs stay end-to-end
encrypted (NIP-17) — a public community still needs private conversations.

What it changes: group content is mostly public, so privacy stops steering
trade-offs; **reach, scale and moderation** do. What the design does not
yet give a community that size, measured against the sections below:

| a big public community needs | today | where |
|---|---|---|
| to be **found**, and joined without an invite | invite links only | §8 |
| **tens of thousands of members** | full replication; ~1,000 is the v1 target, sparse replicas post-v1 | §13 |
| to be **read before joining** (and linked from the web) | members only | §5, §8 |
| **voice for more than 8** (events, stages) | mesh, 8 per channel | §10 |
| **moderation at scale**: bots, automod, raids, spam | roles, ban, mute, slowmode | §6 |
| to **stay up** with nobody online | anchors (hosted anchors = the business) | §9, §12 |

Each row is a decision to make on its own, in `PLAN.md`, before building it.

---

## 1. Could "every person is a relay" work? Yes — for a precise reason

The whole design rests on one property of Nostr, and it is worth saying
before anything else:

> **A Nostr event is self-authenticating.** Its id is the hash of its
> content, and its signature is by the author's key. Whoever hands you an
> event, you check it yourself — you never have to trust the messenger.

That is what makes untrusted home machines able to relay for each other.
HIT_ENTER and HIT_ENTER_2 needed a trusted host because the *host* said what
was true: who is a member, what was said, in what order. Here, what was said
is signed by whoever said it. A peer that forwards a message cannot forge it,
cannot alter it, and cannot pretend it came from somebody else. The worst a
dishonest peer can do is **withhold** — and withholding is defeated by having
more than one peer, which is the point of everybody being one.

iroh supplies the half Nostr lacks. A Nostr relay is a WebSocket server at a
public DNS name — **somebody has to rent a machine with a public IP**, which
is exactly the infrastructure you do not want to depend on. iroh lets a laptop
behind a home router accept a connection addressed to a public key, with hole
punching and a relay fallback. So:

- **Nostr** gives us the person (a keypair), the message (a signed event), a
  vocabulary of standard event kinds (the NIPs), and the payment rail (zaps).
- **iroh** gives us the connection between two machines that have no public
  address, plus `iroh-blobs` for files, `iroh-gossip` for live fan-out, and
  QUIC datagrams for voice.

**Your intuition is right, and HIT_ENTER already proved the harder half of
it**: the thing that worked between you and your friend in another city was
iroh getting through two home routers. What this design adds is that the
data no longer needs a single machine that everyone trusts.

### The four things that are *not* free, stated up front

1. **A relay that sleeps is not a relay.** A group is alive while at least
   one member's app is open. A DM to someone who is offline, or a new member
   joining at 4am when nobody is on, needs a machine that is always on. That
   machine is an **anchor** (§9): the same binary, headless, on a Raspberry
   Pi, a VPS — or one we host and charge for (§12).
2. **Authority is the hard problem, not transport.** "Who is banned" and
   "who is an admin" have no server to decide them. §6 is the answer, and it
   is the part of this design most likely to be wrong in an interesting way.
3. **"No dependency on public infrastructure" is true for Nostr relays and
   not quite true for iroh.** Hole-punching falls back to iroh relay servers,
   and address lookup uses n0's DNS/pkarr by default. Those carry encrypted
   bytes they cannot read, they are self-hostable, and an anchor can run
   one — but on day one they are n0's. Say so in the docs; do not claim
   otherwise.
4. **Money is not peer-to-peer, and zaps prove it.** A zap touches the
   Lightning network, the recipient's LNURL server over HTTPS, and a
   WebSocket relay where the receipt is published (§11). That is fine — it is
   what zaps *are* — but it is the one part of the app that cannot work on a
   LAN with the internet unplugged.

---

## 2. Where this sits in the Nostr world

Nostr has roughly tens of thousands of daily users, most of them in
Twitter-shaped clients. The bando page's diagnosis is right: the network
does not need another X clone. It needs different kinds of apps.

**What HIT_ENTER 1.0 takes from Nostr, and what it does not:**

| takes | does not take |
|---|---|
| the keypair as the person (npub / nsec) | public relays as a *requirement* |
| NIP-01 events and the relay protocol's message shapes | WebSocket as the transport between peers |
| NIP-29's group event kinds (a Discord-shaped vocabulary others already speak) | NIP-29's "the relay is the authority" (§6) |
| NIP-17 DMs (gift-wrapped, genuinely end-to-end encrypted) | NIP-04 (the leaky legacy DM) |
| NIP-57 zaps, NIP-47 wallet connect | a wallet of our own |
| NIP-77 negentropy for syncing two relays | — |
| kind 0 profiles, NIP-25 reactions, kind 5 deletions | — |

The consequence worth planning for: **peers speak Nostr, over iroh, to each
other.** A normal Nostr client (Damus, Amethyst, 0xchat) cannot dial an iroh
endpoint, so the mesh is a separate network that uses Nostr's language. Two
bridges connect it back, both optional and both on anchors:

- an anchor can **also listen on `wss://`** as an ordinary relay, which makes
  its groups joinable from NIP-29 clients (Chachi, 0xchat, Nostrord) and
  gives zap receipts somewhere to land;
- a user can **opt in** to publishing their profile and DM inbox to public
  relays, so people outside the app can find them and DM them.

Nothing in the core loop — sign up, make a group, invite a friend, talk,
share a file, talk out loud — touches a public Nostr relay.

---

## 3. Identity — four layers, again

HIT_ENTER_2's `IDENTITY.md` had four layers, and they survive with one huge
change: **the identity server is gone.** `he2-idp`, `he2-idpd`, email codes,
passwords, Argon2id, the issuer key, the keyset — all of it is replaced by
the user holding their own key. That is roughly a third of HIT_ENTER_2 that
does not need to exist.

| layer | what it is | who makes it |
|---|---|---|
| **device** | an iroh `EndpointId` (ed25519), one per install, `device.key` | the app, on first launch |
| **person** | a Nostr keypair (secp256k1 Schnorr), the npub | the user, or imported |
| **binding** | a signed event: *"npub P is reachable at, and speaks from, device D"* | the person, for each device |
| **membership** | npub P in group G, with roles | the group's authority (§6) |

### Why the device key is not the Nostr key

They are on different curves (ed25519 vs secp256k1), so they *cannot* be the
same key — but even if they could, they should not be. The device key is hot:
it sits on disk and answers every connection. The person's key is the whole
identity, has no recovery, and must be kept somewhere better (the OS
keychain, later a NIP-46 signer). HIT_ENTER's rule — **the space's key and
the device's key are different keys** — becomes: the person's, the group's,
and the device's keys are three different keys.

### The binding — HIT_ENTER_2's assertion, with no issuer

An addressable event (kind `30436`, provisional — §15 q4), `d` tag =
the device's `EndpointId` in lowercase hex, signed by the npub, carrying an
expiry, **and countersigned by the device**: a `proof` tag holds the device
key's signature over the npub, so nobody can publish "my npub is at *your*
device" as a discovery record (added in M1). The exact shape and every
verification step are in `PROTOCOL.md`. It replaces HIT_ENTER_2's
assertion, and the rule that made the assertion safe carries over word for
word:

- **Always read the peer's `EndpointId` off the iroh connection, never from a
  message body.** A peer is "npub P" on a connection only if a valid, unexpired,
  unrevoked binding says P speaks from *that* connection's `EndpointId`.
  A binding copied to another machine is worthless there — that machine has
  a different `EndpointId`.
- **Verification is offline and has no signature-only entry point.**
  `hit_event::verify_binding(event, connection_endpoint_id)` takes the
  device as a required argument, exactly as `he2_id::verify` did.
- **Revocation is a newer binding with `revoked`** (addressable events
  replace by `d` tag), spread by gossip. Like HIT_ENTER_2's revocation, it
  is eventually consistent, and `IDENTITY.md` §6's honesty applies: a peer
  that has not heard yet will still believe the old one until it expires. Keep
  expiries short (days) and re-sign automatically.

The binding does **two** jobs, which is why it is worth its own event: it
authenticates a connection (the job NIP-42 `AUTH` does over WebSocket), and it
is how you find someone — *"npub P can be dialled at D"* is the discovery
record (§8).

### Messages are signed by the person, on the device

Every chat message is a Nostr event signed by the npub, so every device needs
to sign as the person. v1 does what most Nostr clients do: **the nsec lives on
each device, in the OS keychain** (`keyring` crate — Secret Service on Linux,
Keychain on macOS, Credential Manager on Windows), never in a plain file.
Adding a device means importing the key. NIP-46 remote signing (a "bunker")
is post-v1: signing every chat message via a round trip to a phone is too
slow to be the default, and NIP-26 delegation is deprecated.

### Onboarding and recovery, honestly

**There is no "forgot password".** Lose the nsec and the identity is gone;
leak it and someone else is you, everywhere on Nostr, forever. The app must:

- generate the key on first launch, and **refuse to let the user into a group
  until they have saved a backup** — a NIP-49 `ncryptsec` (password-encrypted
  key) file, or the key shown once for a password manager;
- accept an existing `nsec` / `ncryptsec` for people already on Nostr — your
  existing followers and profile come with you, which is the pitch;
- never log, display after setup, or send the nsec anywhere. This is
  HIT_ENTER's "never log a password", with higher stakes.

---

## 4. The wire

Four ALPNs on one iroh endpoint:

| ALPN | carries | from |
|---|---|---|
| `hit/nostr/1` | NIP-01 relay messages — `EVENT`, `REQ`, `CLOSE`, `EOSE`, `OK`, `CLOSED`, `NOTICE` — plus NIP-77 `NEG-*` | new; framing from `he-proto` |
| `iroh-gossip` | live fan-out of new events within a group | iroh |
| `/iroh-bytes/4` | file bytes, content-addressed | HIT_ENTER_2 M7, as-is |
| `hit/voice/1` | Opus audio over QUIC datagrams (or `iroh-roq`, §10) | new |

### `hit/nostr/1` is a Nostr relay speaking over QUIC

The message shapes are NIP-01's JSON arrays, unchanged, so the embedded
relay can be tested with existing Nostr tooling. The framing is HIT_ENTER's
length-prefixed JSON with the 1 MiB cap — already correct, already fuzzed by
use. What changes versus a WebSocket relay:

- **No `AUTH` event.** The first frame on a connection is the peer's binding;
  the relay checks it against the connection's `EndpointId`. Every later
  request is answered as that npub. Accepting a connection is still not
  authorization. *(M2: both ways — the listener answers with its own binding, so
  the dialler knows whom it reached. Exact frames in `PROTOCOL.md`.)*
- **A peer serves only what the asker may see.** A `REQ` for a group's
  events is answered only if the asker is a member, and only for channels
  they can view — the same three-place `VIEW_CHANNEL` rule as HIT_ENTER_2 M5
  (list, write, *and* fan-out), now enforced by every peer instead of one host.
- **A peer stores only what it has a reason to store**: events of groups its
  user is in, DMs addressed to its user (or, on an anchor, to users it serves),
  and the profiles and bindings of people it talks to. It is not an open relay.
  An open relay on a home connection is a spam and legal liability.

### Sync: negentropy, not "send me everything since"

HIT_ENTER's reconnect used `resume` + `since`, which works because one host
has one ordered history. Here, two peers each hold a partial copy of a set,
with no global order. **NIP-77 negentropy** (range-based set reconciliation)
tells two peers exactly which event ids each is missing, in a few round trips
and proportional to the *difference*, not the history. It is built for
relay-to-relay sync, which is precisely what two HIT_ENTER apps are. This is
the single most load-bearing borrowed idea in the design.

On connect: negentropy over the group's events, fetch the missing ones,
verify each. While connected: gossip for anything new. Gossip loses messages
under churn; periodic negentropy repairs whatever gossip dropped. **Gossip is
for speed, negentropy is for truth.**

### Live fan-out: `iroh-gossip`, gated by ALPN (spike S2, 2026-10-10)

**Answered: yes, a gossip topic can be gated by `EndpointId` — but the unit
of gating is the ALPN, not the topic.** `iroh-gossip` has no membership hook,
but `Gossip::handle_connection` is public and says to check the ALPN
yourself, so each gossip instance is wrapped in our own `ProtocolHandler`
that reads `conn.remote_id()` and either forwards the connection or closes
it. A refused stranger — told the topic and a member's id — never joins and
receives nothing. What that changes:

- **One gossip instance (and ALPN) per gated set of peers.** A connection
  carries every topic, so on a shared ALPN any allowed peer can join *any*
  topic whose id it knows. Per group: ALPN `hit/gossip/<group>`, allow-list =
  the group's members. A private channel inside a group needs its own, with
  the allow-list = those who may view it — the same `VIEW_CHANNEL` rule,
  evaluated by `hit-group`, not a second list. A public channel shares its
  group's instance. Cost is one actor and roughly one connection per
  (peer, gated set), not measured at scale.
- **Every member's gate must admit the others**: links are bidirectional.
- **A list is not revocation.** Removing a peer stops *new* connections; the
  open one keeps delivering. Revocation = remove **and close** the live
  connections (we hold clones). Tested: after that the peer receives nothing
  and cannot rejoin.
- **Gate both directions** *(M3, found by its kick test)*. Gossip also
  *dials* its neighbours, and a connection we opened to a removed member is
  not one an accept handler ever sees — the kicked member kept hearing the
  group. The product's gate is an endpoint hook (`after_handshake`), which
  sees every connection, incoming and outgoing, keeps a weak handle, and
  closes it when the group's state changes.
- **Don't subscribe before you are a member.** A peer refused once did not
  recover by retrying (20 s, resubscribe, `join_peers`); a *member* dialling
  it, or restarting its gossip instance, did. Cause not isolated. The app
  learns membership over `hit/nostr/1` first, so this is a rule, not a
  problem.
- Gossip is still best-effort: gossip for speed, negentropy for truth.

Do not paper over any of this with encryption of the gossip payload — that
is "partial encryption", which HIT_ENTER's rules forbid for good reason.
Detail and the limits of the test: `spikes/gossip-gate/README.md`.

---

## 5. Groups, channels, messages — NIP-29's vocabulary

NIP-29 is Nostr's standard for relay-based groups, and several clients
already speak it. We use its **event kinds** so the data is recognisable and
an anchor can expose it to those clients (§2), and replace its **authority**
(§6).

| what | kind | signed by |
|---|---|---|
| chat message | `9` with an `h` tag (group) and our channel tag | the author |
| reaction | `7` (NIP-25) | the author |
| edit | a new kind `9` referencing the old one (§15 q6) | the author |
| delete own message | `5` (NIP-09) | the author |
| create group | `9007` | the owner |
| add / remove member, set roles | `9000` / `9001` | an admin with the permission |
| edit metadata, channels | `9002` | an admin with the permission |
| moderator deletes a message | `9005` | an admin with the permission |
| create invite | `9009` | an admin with the permission |
| join / leave | `9021` / `9022` | the joining/leaving user |
| group metadata, admins, members, roles | `39000`–`39003` | **the group key** |

**The group's address is its own npub.** `nevent`/`naddr`-style invite links
carry the group's npub, an invite code, and a handful of `EndpointId`s of
peers to bootstrap from (the owner's device, an anchor). HIT_ENTER's rule —
**a space's secret key is its identity, treat it like an SSH host key** —
applies to the group's nsec unchanged.

**Channels** are declared in the group's metadata (`39000`, our own tags),
each with an id, a name, a kind (`text` / `voice`), and permission
overrides. There is no channel event per channel; a group with 40 channels
is one replaceable metadata event.

---

## 6. Authority without a server — the hard part

In HIT_ENTER and HIT_ENTER_2, the host process decided everything, and "who is
banned" was a row it read. In NIP-29, the relay decides and signs the result.
Here there is no one machine to ask. Three designs were considered:

| design | how | why not / why |
|---|---|---|
| **A. one authority peer** (NIP-29 as written) | the owner's device signs all group state | owner closes the laptop → nobody can join, nobody can be banned. HIT_ENTER's single point of failure, reintroduced |
| **B. a fully replicated state machine** (Matrix-style state resolution) | every admin action is an op; every peer resolves conflicts deterministically | correct in theory; Matrix spent years on "state resets" bugs doing exactly this. Too big for v1 |
| **C. signed actions, a rank rule, and a sequencer that is allowed to be late** | below | **chosen** |

### Design C

1. **The group key is the root.** `39001`–`39003` (admins, members, roles)
   are signed by the group's nsec and carry a `seq` counter. The group nsec
   lives on the owner's devices and on any anchor the owner hands it to.
   Nobody else holds it.
2. **An admin's action is effective the moment a peer sees it.** A `9000`
   ban by admin A is valid on every peer that can check, from state it
   already has, that A holds `BAN_MEMBERS` and outranks the target. No
   sequencer needed; the owner can be asleep.
3. **The rank rule eliminates the conflicts that make B hard.** Carried
   straight from HIT_ENTER_2 M5: *you may not touch a role at or above your
   own rank, and you may not grant a permission you do not hold.* Two admins
   of equal rank cannot ban each other, so the classic "A bans B while B bans
   A" race has no valid second half. The owner is outside the role system
   and cannot be acted on — including by themselves.
4. **The group key compacts, later.** Whenever a holder of the group nsec is
   online, it folds the admin actions it has seen into a new `39001`–`39003`
   snapshot with `seq + 1`. A peer's view of the group is **the highest-`seq`
   snapshot plus every valid action newer than it.** Snapshots exist so a new
   member does not replay the group's whole moderation history, and so the
   owner has the last word on anything the rank rule allowed but they
   disagree with.
5. **`seq`, never `created_at`, orders snapshots.** Clocks on home machines
   lie; a counter signed by one key does not.

*As built in M3* (`crates/hit-group/src/state.rs`, `PROTOCOL.md` "The
fold"): actions dated **at or after** the snapshot's newest `created_at`
apply on top of it; earlier ones are folded. A compactor therefore dates its
snapshot one second after everything it folds. Timestamps are whole seconds,
and two actions in one second have no order, so within a second actions are
applied until nothing more applies; across seconds the order is strict, so
an old refused action never jumps past a newer kick. **The honest limit:**
an action the compactor had not seen, dated before its snapshot, is lost —
visibly (it is not in the state its author expected), and it can be redone.
A sequencer that is allowed to be late is allowed to miss things.

*As built in M5:* bans, unbans and mutes are `9001` and `9030` actions in the
same fold, channels and roles are edited by `9002`, a moderator removes by
`9005`. A kick, ban or mute leaves a **silence** — a span in which the
target's messages are invalid — so a peer that took the banned user's next
message before the ban drops it when the ban arrives. Done-when: a banned
user's patched client (`hit-cli lie`) posts, and no honest peer keeps it
(`crates/hit-node/tests/m5_done_when.rs`, `scripts/m5-by-hand.sh`). Limits:
`PROTOCOL.md`, "Enforcement".

### Joining when the owner is offline

*Built and tested in M3: the owner killed, a fourth person joined from
another member's invite.*

A `9009` invite, signed by an admin, says *"a join presenting code X is
valid until T."* A `9021` join event carrying X is then valid on every peer
that has the invite — **no one has to be online to admit you**, only someone
to sync with. The honest limit: an invite's *maximum uses* cannot be enforced
without a sequencer (two peers can each admit the "last" joiner). v1 says so
in the UI: "up to about N uses". The compaction step trims the excess.

### What enforcement means when everybody holds a copy

Say this in the docs and in the moderator UI, because it is different from
Discord and different from the first two HIT_ENTERs:

- A ban means **every honest peer stops storing, forwarding, and showing**
  the banned user's new events, and stops serving the group to them. A banned
  user's own modified client can still sign events; nobody will carry them.
- A moderator delete (`9005`) means honest peers delete the event and refuse
  it if it comes back. **Copies already on disk elsewhere cannot be recalled.**
  That was always true of every chat app with a screenshot key; here it is
  also true of the database.
- A private channel's history is on the disk of **every member who can view
  it**, in plaintext (§7). Removing someone from a channel stops them getting
  new messages; it does not reach into their disk.

### Permissions — HIT_ENTER_2's `roles.rs`, turned into a pure function

The bitset, the per-channel overrides, the escalation rules, and the rule
that **every authorization decision goes through one module** all carry over
from HIT_ENTER_2 M5, with the 49 refusal tests as the starting suite. What
changes is its input: not SQL tables in a host's `space.db`, but the group
state a peer has assembled from signed events. It becomes
`hit_group::effective(state, npub, channel) -> Perms` — no I/O, and **every
peer runs it on every event it receives**, not just one host. Permission bit
positions stay wire-stable and are never renumbered.

---

## 7. What is encrypted, and what is not

HIT_ENTER's rule stays: **plaintext or a real end-to-end design, nothing in
between.** Applied to a mesh:

| what | at rest | who can read it |
|---|---|---|
| public group | plaintext | **everyone** — anyone may join and sync. Treat it as published |
| private group / private channel | plaintext on members' disks | members who can view it, and any anchor the group uses |
| **DMs** | **end-to-end encrypted (NIP-17: NIP-44 + NIP-59 gift wrap)** | the two people. An anchor storing a DM sees neither the content nor the sender |
| DM files | encrypted blob (NIP-17 kind 15, key in the sealed message) | the two people |
| voice, mesh | QUIC/TLS per hop, which is end to end in a mesh | the people in the call |
| voice, through a forwarding node (post-v1) | the forwarder can hear it | disclosed in the UI |
| transit | QUIC + TLS 1.3, always | — |

**DMs are answered.** HIT_ENTER_2 left DMs undecided because plaintext DMs
on a host were uncomfortable (`DESIGN.md` §14 q5). Here they are necessary,
and the reason is the mesh: a DM to an offline person is stored by *someone
else's* machine. NIP-17 already is a real E2EE design, standardised, with
interoperating clients — so we adopt it rather than invent one, and the rule
is satisfied without us writing a line of cryptography.

**Group E2EE is not in v1.** The serious design for it on Nostr is MLS
(the Marmot protocol / what was NIP-EE). It is real, and it is a milestone
of its own after 1.0 (`PLAN.md` §6). Until then, a private group's lock icon
must say "members and your anchor can read this", not imply more.

**Your IP address.** In a mesh you connect directly to many peers, so every
group member can learn your IP — a real problem in a Discord-shaped world of
gamers and grudges. HIT_ENTER_2's spike found a client **cannot refuse a
direct path per connection**, only for the whole endpoint at bind time. The
proposal here: **run two endpoints** — a direct one for DMs, calls and small
private groups, and a relay-only one (`clear_ip_transports()`) for public
groups — each with its own binding. `PLAN.md` M4 decides.

**Spike S5 (2026-10-10): it works, it is cheap, and the keys must differ.**
In one process a relay-only endpoint advertised zero IPs (also on the real
network, where the direct one advertised two public ones), and a stranger
held no IP path to it in either direction. The second endpoint costs
**under 1 MB, two sockets and no threads**; the endpoints close and rebind
independently, and nothing in iroh 1.3 assumes one per process. **One key
on two endpoints fails silently:** the relay routes every dial to the newest
registration, and the older endpoint is unreachable by relay with no error.
So it is two device keys and two bindings, and **each group lives on exactly
one endpoint** — its `hit/nostr/1`, gossip, blobs and voice all go over it.
Mixing them would hand your IP to the peers it was hidden from. What is left
to weigh in M4 is complexity, not memory. Limits (loopback paths, no
cross-network peer, the relay can link the two ids):
`spikes/two-endpoints/README.md`.

---

## 8. Finding people and groups

- **An invite link is enough to start.** It carries the group's npub and
  bootstrap `EndpointId`s, as HIT_ENTER's ticket did. Two friends need no
  public infrastructure beyond iroh's own.
- **Inside a group, you find members through each other.** Bindings and kind 0
  profiles of members are group events like any other; syncing the group
  syncs the address book.
- **To DM someone you share no group with**, you need their bindings. The
  user may *opt in* to publishing their profile, bindings and DM inbox list
  (kind 10050) to a few public Nostr relays — which is also what makes them
  findable by, and able to DM with, other Nostr apps. Off by default.
- iroh addressing (`EndpointId` → current IP/relay) uses iroh's discovery.
  Publish **relay only**, as HIT_ENTER_2 found is iroh's default.

---

## 9. Anchors — the always-on peer

An anchor is **the same node, headless** (`hit-anchor`), with a pin policy:

- pins one or more groups — stores everything, fetches every file, stays
  online so the group is alive at 4am and joiners have someone to sync with;
- is a DM inbox for users who name it in their 10050 list — stores gift wraps
  it cannot read;
- optionally holds a group's nsec, so compaction (§6) happens even when the
  owner is away;
- optionally listens on `wss://` — a real Nostr relay for NIP-29 clients and
  for zap receipts (§11);
- optionally runs an `iroh-relay`, so a community can stop depending on n0's.

**One binary, two roles** (HIT_ENTER's first rule) becomes *one node, many
roles*: the app, an anchor, and the CLI are the same `hit-node` with
different policies. Never add an "anchor-only" code path for something a
member's app also does.

Anchors are optional, anyone can run one, and a group can have several. They
are also the business (§12).

---

## 10. Voice

Voice channels, Discord-style: click to join, push-to-talk or voice
activation, mute, deafen, a speaking indicator, and 1:1 calls from a DM.

- **Proven by someone else:** n0's `callme` does Opus audio over iroh
  (`iroh-roq`, RTP over QUIC), with `cpal` for devices and optional echo
  cancellation via the WebRTC audio processing library. We build on that,
  not from scratch. Spike S3 runs it between your machine and your friend's.
  **S3 status (2026-10-10):** Linux ↔ Linux on one LAN, USB headsets —
  clear, low latency, direct path, ~1 playback underrun per 17 s. Speakers,
  load and cross-network are untested; Bluetooth (HFP) headsets sounded
  robotic and drifted once a second, so M9 needs clock-drift compensation
  (`spikes/voice/README.md`).
- **Topology, by arithmetic.** Opus speech at ~32 kbit/s. In a full mesh each
  speaker sends one stream per listener: 8 people → 7 × 32 = 224 kbit/s up,
  fine on any home line; 25 people → 768 kbit/s up and 24 decoders, not fine
  on a weak uplink. **v1 caps a voice channel at 8 in a mesh.** Beyond that,
  a forwarding node (an SFU — an anchor or the member with the best uplink)
  receives one stream per speaker and fans it out: post-v1, and the only
  place voice stops being end to end.
- **Presence in a voice channel** is an ephemeral event (kind `20437`, not
  stored) on the group's gossip topic.
- **As built (M9, 2026-10-11):** not `iroh-roq` — it exists only for iroh
  0.33 — but our own `hit/voice/1`: a JSON handshake on one bi-stream, then
  one Opus frame per QUIC datagram (`PROTOCOL.md`). The lower device id of
  each pair dials. The listener gates the dialler by the device the
  connection proves, and decides for itself, every second, whether a
  person may be heard. WebRTC's echo canceller (bundled, BSD-3-Clause) runs
  with noise suppression, delay-agnostic, on by default. **1:1 calls** ride
  the same ALPN: the caller rings every device of the callee's, presenting
  its binding; the callee's app rings only for someone it shares a group
  with or has written to — so a DM from a stranger does not buy a ringing
  phone. **Clock drift** (S3's first inheritance): each device buffer is
  held at 40 ms by nudging the resampler at the device's edge, at most
  ±0.5 %; the jitter buffer skips ahead once if 120 ms beyond its delay
  pile up; drops and underruns are counted and shown in the call bar.
- **Echo cancellation is the make-or-break detail.** Without it, anyone on
  laptop speakers hears themselves back. Headphones hide the problem during
  testing; a test with speakers is part of M9's done-when.
- **The webview does not do audio.** WebKitGTK's WebRTC support on Linux is
  not something to build on. Audio is native Rust in `hit-voice`; the window
  only shows state and sends button presses.

---

## 11. Zaps

Users zap **messages and files** (both are events, so it is one feature),
and see zap totals on each.

### The NIP-57 flow, and what it touches

1. The recipient has a Lightning address (`lud16`) in their kind 0 profile —
   from **their own** wallet (Alby Hub, Coinos, Primal, Wallet of Satoshi, …).
   HIT_ENTER does not provide or hold a wallet (§12 says why that matters).
2. The sender's app fetches the recipient's LNURL-pay endpoint over
   **HTTPS**, and checks it `allowsNostr`.
3. The app builds a **kind 9734 zap request** — `p` = recipient, `e` = the
   message or file event, amount, and a `relays` tag — signs it, and sends it
   to the LNURL callback, which returns a Lightning invoice.
4. The app pays it through the user's wallet via **Nostr Wallet Connect
   (NIP-47)** — a connection string from their wallet, stored in the OS
   keychain. Fallback: show the invoice as a QR code for any wallet.
5. The recipient's LNURL server publishes a **kind 9735 zap receipt** to the
   relays named in step 3.
6. The sender's app (and the recipient's anchor) fetches that receipt,
   **validates it** (NIP-57's rules: signed by the LNURL server's
   `nostrPubkey`, invoice amount matches the request; **not** the
   description hash, which Primal's invoices do not commit — `PROTOCOL.md`) and injects it into the group, where it syncs like any event.
   **As built (M10):** a receipt is signed by a provider, not a member, and
   has no `h`, so it travels inside a member-signed carrier (kind 9436,
   `PROTOCOL.md`). And a third member cannot ask the LNURL server for its
   `nostrPubkey` — no HTTP in the chat plane — so **the recipient's own
   profile names it** (`zap_pubkey`, written by their app when they set the
   address), and every peer counts a receipt only if that key signed it.
   A forged receipt (a valid signature, the wrong key) is stored and shown
   nowhere.

### The consequences

- **Step 5 needs a `wss://` relay**, because an LNURL server cannot dial
  iroh. It is the group's anchor if it has one listening on `wss://`,
  otherwise a public relay. This is the one place v1 genuinely needs a
  public-facing relay, and it is honest about it. **As built (M10): only
  the group's anchors**, which say their `wss://` in their profile and take
  a receipt without NIP-42; a group with no such anchor cannot be zapped
  from the app. Every group is invite-only today, so every group gets the
  private-group rule below. The public-relay fallback is the owner's call
  (§15).
- **Zap receipts are not proof of payment** (NIP-57 says so): they are the
  recipient's wallet provider's word. A receipt from a validated provider is
  good enough to show a total; it is not good enough to unlock anything.
- **HTTP lives in exactly one crate.** HIT_ENTER_2 kept axum in `he2-edge`
  alone; here an HTTP *client* (`reqwest`) lives in `hit-zap` alone. The chat
  plane never speaks HTTP.
- **Nutzaps (NIP-61, Cashu ecash)** fit a mesh better — the payment *is* the
  event, no LNURL server, no receipt relay — at the cost of trusting a mint.
  Post-v1, behind the same "zap" button.
- **A zap is public by nature.** A 9735 receipt names the sender, the
  recipient, the amount and the message, and the LNURL server publishes it
  to whatever relays the request named. For a **private group**, the app
  names *only the group's own anchor* in the `relays` tag — never a public
  relay — so a zap in a private room does not announce that room to the
  world. The zap dialog offers an **anonymous zap** (NIP-57's `anon` tag,
  signed by a throwaway key) for anyone who does not want their name on it.

### The HIT_ENTER fee — 1% of every zap (decided 2026-09-27)

**Decided by the owner:** HIT_ENTER takes **1%** of each zap sent from the
app. How it is taken is what keeps it legal and honest, so the mechanism is
as fixed as the number:

1. **A second, separate payment, on top, from the sender's own wallet.**
   Zapping 1,000 sats pays **1,000 to the recipient** and **10 to
   HIT_ENTER's Lightning address**, both through the sender's NWC
   connection. The recipient always gets exactly what the sender chose; the
   fee is never taken *out of* their zap. **HIT_ENTER is never in the path
   of the zap itself** — it never receives the recipient's money to pass on,
   which is the line between "a service charging a fee" and "a money
   transmitter" (§12).
2. **Shown before sending, every time.** The dialog reads
   *"1,000 sats to @friend · 10 sats HIT_ENTER fee · 1,010 total"*. A fee
   the user discovers afterwards is a fee they tell their friends about.
3. **`fee = floor(amount / 100)`, and nothing below 100 sats.** Lightning
   cannot send a fraction of a sat, and a 21-sat zap's fee would be 0.21.
   Zaps under 100 sats carry no fee; the threshold and the percentage are
   two constants in `hit-zap`, not scattered through the code.
4. **The zap first, the fee second, and the fee is best-effort.** If the
   zap fails, nothing is charged. If the zap succeeds and the fee payment
   fails (a route not found, a wallet budget exhausted), **the zap stands**,
   and the fee is retried once later, then dropped. A recipient must never
   lose a zap because HIT_ENTER's fee had a bad route.
5. **The fee payment is a plain Lightning payment, not a zap.** No 9734,
   no 9735, no message id and no pubkey in its memo — a fixed description
   like `HIT_ENTER fee`. A zap receipt would publish, for ever, which
   message every fee was for; the project does not need to know that, so it
   is not told. The only thing HIT_ENTER learns is that *some* wallet paid
   *some* fee.
6. **The QR fallback charges it too**, as a second QR after the first is
   paid, marked as the fee.
7. **Zaps from other Nostr apps pay no fee.** Someone zapping a HIT_ENTER
   message from Amethyst never touches our code. That is fine and must stay
   fine: nothing in the protocol tries to stop it.

The arithmetic, so expectations are set by numbers: 1,000 active users
zapping 5,000 sats a month each is 5,000,000 sats, and 1% of that is
**50,000 sats a month — about $50** at $100k per BTC. The fee grows with the
network and costs nothing to run; the hosted anchors (§12) are what pay the
bills first.

---

## 12. Money — how the project gets paid

Zaps pay **users**. They do not, by themselves, pay **you**. Three ways the
project earns, in the order they will matter:

1. **Hosted anchors** — the HIT_ENTER_2 business, reused. "Your community
   online 24/7, every file kept, DMs waiting for you, a `wss://` address
   NIP-29 clients can join." A few dollars a month per group, payable **in
   sats** (a Lightning invoice, or a NWC subscription) or by card through the
   Lemon Squeezy seam HIT_ENTER_2 M9 already built. HIT_ENTER_2's signed
   grants, lifecycle states and `hosting.json` apply to an anchor almost
   unchanged. **This is the only one that makes recurring money early.** The
   app itself is free to download; anybody may still run their own anchor
   instead, which keeps the offer honest.
2. **The 1% zap fee** — decided; §11 has the mechanism, and the mechanism is
   the part that matters: a separate payment on top, from the sender's
   wallet, shown before sending, never in the zap's path.
3. **Paid groups** — creators charge sats to join a group or a channel
   ("digital creators fully rewarded for their craft"). The creator is paid
   directly; the project takes a visible fee the same way as §11's, as a
   second payment. Post-v1: it needs the receipt-is-not-proof problem solved
   properly, because here a receipt would unlock something.

Grants from OpenSats and the HRF Bitcoin Development Fund are **not** on
this list any more: they fund open-source work, and HIT_ENTER is closed
(§15 q1).

### What never to build

- **A custodial wallet** — holding users' sats makes the project a money
  transmitter, with licensing in most countries. Every design above has the
  sender's wallet pay the recipient's wallet directly; **HIT_ENTER never
  touches user funds**, and that is the sentence that keeps it legal. (Not
  legal advice — `OPERATIONS.md` in HIT_ENTER_2 already says to ask an
  accountant before taking money; that still holds.)
- A token, ads, or anything that sells what the mesh knows about its users.

---

## 13. What breaks at scale, and when to care

- **Full replication.** Every member stores every event of every channel
  they can see. A kind 9 message is ~500 bytes with its id, pubkey and
  signature. A busy 1,000-member group at 10,000 messages a day is ~5 MB/day,
  ~1.8 GB/year — fine. A 100,000-member group is not a v1 target; it needs
  **sparse replicas** (members keep recent history, anchors keep all), which
  is post-v1.
- **Files are not fully replicated.** A member fetches a file on view and
  keeps it in an LRU cache with a size cap; anchors pin everything. The
  uploader plus any anchor are the durable copies.
- **The member list.** HIT_ENTER_2 M6's lesson — *`ready` must not carry the
  whole member list* — applies to `39002` too. Membership is the fold of
  `9000`/`9001` events; the snapshot is a checkpoint, and the UI pages
  through members rather than holding them all.
- **Gossip** is fine to thousands of online peers per topic; negentropy
  cost scales with the difference between two peers, not the history.
- **Voice**: 8 per channel in v1, by §10's arithmetic.

---

## 14. Rules that are not negotiable

Carried forward, reworded for a mesh:

- **Always read the peer's `EndpointId` from the iroh connection, never from
  a message body.** A binding is valid only for the connection it arrived on.
- **Verify every event you did not sign**, before storing it, forwarding it,
  or showing it: signature, then the group's permissions for its author.
  A peer that forwards an unverified event lends its credibility to a forgery.
- **Every authorization decision goes through `hit_group`.** One pure
  function, called by every peer, for every event.
- **The person's key, the group's key, and the device's key are three
  different keys.** Never show one where another belongs.
- **The nsec is never logged, displayed after onboarding, sent, or written
  outside the OS keychain** (or an `ncryptsec` the user asked for).
- **Plaintext or a real E2EE design, nothing in between.** DMs are NIP-17;
  groups are plaintext and say so.
- **A blob store is never served unauthenticated** — and never with
  `Remote::fetch` on the upload path (HIT_ENTER_2 M7's existence oracle).
  Public-group files are the one exception, and it is explicit per group.
- **iroh owns the chat plane.** No WebSocket or HTTP carries chat.
  `wss://` exists only on an anchor that opted in; an HTTP or WebSocket
  client exists only in `hit-zap` — for zaps, and since M13 for what is
  public anyway on public relays: a person's profile and public groups'
  announcements sent, announcements read back (stored ones only).
- **HIT_ENTER never holds user funds.**
- **The fee is a second payment, never a cut of the first** (§11). No code
  path routes a zap through a HIT_ENTER wallet, and the fee payment carries
  no message id and no pubkey.
- **The mirror is written before the UI is told** — disk first, screen
  second, as in every HIT_ENTER.
- Wire types live in `hit-proto`/`hit-event`; changing one updates
  `PROTOCOL.md` in the same commit. Our own event kinds are documented there
  before they ship, because other Nostr software will see them.

---

## 15. Open questions

Ordered by how much they hurt if answered late.

1. ~~**Licence, and where the code comes from.**~~ **Answered again
   2026-10-22: open source, MIT** — the owner's decision, published to the
   public `isciuciustin/HIT_ENTER`. It reverses the first answer below,
   kept for the record. MIT means forks may drop the 1% fee (§11); the
   official builds and hosted anchors are what pay. The dependency rules
   still hold (no GPL family, `cargo about`).

   *First answer, 2026-09-27: closed source, all rights reserved* — the
   same licence as HIT_ENTER_2. What followed from it:
   - **HIT_ENTER_2's code may move here.** Both are the owner's and both are
     proprietary, so nothing is relicensed by the move. Still record each
     piece that moves in `docs/SESSION_LOG.md`, so the history says where it
     came from.
   - **The original HIT_ENTER's code may move here too.** It is MIT OR
     Apache-2.0, which permits use in closed software, provided its copyright
     notice travels with it. It goes in `THIRD_PARTY_NOTICES.md`.
   - **Every dependency's licence is an obligation**, as in HIT_ENTER_2:
     `cargo about` with an accepted-licence list, and no GPL/AGPL/LGPL in
     the tree. iroh, `rust-nostr`, Opus and the WebRTC audio processing
     library are all permissive; check `callme`'s before copying from it.
   - **The repo stays private** (`github.com/isciuciustin/HIT_ENTER_0.1` is),
     and every crate is `publish = false`.
   - **The uncomfortable part, said once:** a privacy promise from closed
     code is a promise users cannot check. Three things make it checkable
     anyway, and they are worth doing because privacy is the pitch:
     **publish `PROTOCOL.md`** (every event kind we emit, so anyone can
     inspect what leaves their machine); **use standards for anything
     secret** (NIP-17 and NIP-44 are public specs, verifiable on the wire
     by any Nostr tool); and, before a 1.0 that tells people their DMs are
     private, **pay for an independent security review** and publish its
     summary.
2. ~~**Can a gossip topic be gated by `EndpointId`?**~~ **Answered
   2026-10-10 (S2): yes, by ALPN** — one gated gossip instance per set of
   peers, so private channels get their own (§4). Still open underneath:
   what N instances cost on a phone, and whether a member who *introduces* a
   stranger to the swarm can be stopped (it cannot, by this mechanism).
3. ~~**The zap fee.**~~ **Answered 2026-09-27: 1%**, on top, as a separate
   payment (§11). Still open underneath it: whether the 100-sat threshold
   is right once real zap sizes are measured in M10.
4. **Our event kind numbers** (binding, channel tags, voice presence) — and
   whether to write the binding up as a NIP. It is useful to anyone doing
   Nostr-over-iroh; proposing it makes the app a citizen, not a squatter.
   *M1 (2026-10-10) took 30436 for the binding, provisionally*: it is clear
   of every kind `nostr` 0.45 knows but not checked against the full NIP
   registry. Check before v0.1.0.
5. **Two endpoints for IP privacy** (§7) — worth the complexity, or one
   device-wide private mode as HIT_ENTER_2 did? *S5 (2026-10-10) took cost
   off the table*: the second endpoint is under 1 MB and works alongside
   the first, as long as it has its own key. Still open, for M4: whether two
   bindings per device and a per-group endpoint choice are worth it.
   **Recommendation: yes.** The alternative makes every public group either
   leak your IP or relay *everything*, DMs and calls included.
6. **Edits.** Nostr has no settled standard for editing a chat message.
   Pick one and document it before M4 ships messages anyone relies on.
7. **Mobile.** A phone cannot be a relay in the background — iOS will not
   allow it and Android barely. A phone is a *client of an anchor*. That
   makes anchors more important than they look, and it is a post-v1 design.
8. **What happens to HIT_ENTER_2?** Its identity plane is superseded by
   Nostr, its hosting and billing live on as hosted anchors. Retire it by
   decision rather than by neglect.
9. **Zaps in a group without a `wss://` anchor** (M10). Today the app
   refuses: naming a public relay would publish, for ever, who zapped whom,
   how much, and the message id of something said in an invite-only
   group. The alternative — a public relay, with the dialog saying so —
   makes zaps work for every group on day one. **Recommendation: keep
   refusing** until hosted anchors (§12) make a `wss://` anchor one click;
   that is also the business.
10. **HIT_ENTER's fee address** (M10). The mechanism is built and tested;
    `hit_zap::FEE_ADDRESS` is `None`, so no fee is charged or shown until
    the owner names a Lightning address (a plain one is enough: the fee is
    a plain payment, not a zap).
