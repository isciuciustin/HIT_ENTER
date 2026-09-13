# HIT_ENTER wire protocol — `hit-enter/0`

The exact bytes that cross a connection. Written during M2 and kept current:
**every change to a type in `he-proto` updates this file in the same commit**,
and a breaking change bumps the ALPN (PLAN §14).

Nothing here is HTTP. iroh gives us authenticated, encrypted, multiplexed QUIC
streams; layering HTTP on top would add a dependency and buy nothing (PLAN §9).

---

## 1. Transport

| | |
|---|---|
| ALPN | `hit-enter/0` |
| Transport | QUIC + TLS 1.3, via [iroh](https://www.iroh.computer/) 1.1 |
| Address | the server's `EndpointId` — an ed25519 public key |
| Encryption | mandatory, and not ours to disable: it is what QUIC *is* |

A server is dialled by public key. Its IP may change under an open connection
and iroh re-paths rather than dropping it, so nothing in this protocol carries
or caches an address.

### The client's identity is not in this protocol

iroh proves during the TLS handshake that bytes claiming to come from
`EndpointId` *X* came from the holder of *X*'s secret key. The server reads
that from the connection (`Connection::remote_id`). **No message body has a
field for it**, deliberately: trusting a self-declared device identity is the
classic authentication bypass (PLAN §11).

### Accepting a connection is not authorization

Anyone on the internet who knows the `EndpointId` can complete a QUIC handshake
with a server. All that buys them is the right to send one `hello` and be told
no.

---

## 2. Framing

Every frame, on every stream:

```
+--------+--------+--------+--------+----------------------------+
|  u32 little-endian body length    |  body: UTF-8 JSON          |
+--------+--------+--------+--------+----------------------------+
```

- **Maximum body: 1 MiB** (`limits::MAX_FRAME_BYTES`). The length is checked
  against it *before a buffer is allocated*, so four bytes of header cannot
  make a peer reserve a gigabyte.
- A zero length is not a valid frame.
- JSON while the protocol churns, because a frame can be read off the wire and
  pasted into a bug report. A `postcard` codec goes behind a feature flag once
  profiling justifies it (PLAN §9).

Every frame is a JSON object with a `"t"` field naming its type. Optional
fields are **absent**, not `null`.

---

## 3. Stream shapes

QUIC streams are cheap, so nothing is multiplexed by hand and there is no
request-id field to correlate.

### Control stream

One bi-directional stream per connection, **opened by the client first** and
held open for the life of the session.

```
client → server:   hello                       (exactly once, first frame)
server → client:   ready | error               (exactly once, in reply)
server → client:   message | edited | deleted  (events, until the session ends)
                   | presence | typing
                   | channels | members | revoked
```

After `ready` the client never writes on this stream again.

### Request streams

One bi-directional stream per RPC. The client writes a request, reads a
response, and closes:

```
client → server:   send | edit | delete | backfill | resume | typing | invite
                   | create_channel | delete_channel | kick | set_banned
                   | revoke_device | devices | invites | revoke_invite
server → client:   ok | messages | resumed | channel | devices | invites
                   | invite | error
```

A refused request does not end the session — one bad RPC is not a reason to
drop a connection that flaky wifi made expensive to establish.

---

## 4. Handshake

### `hello` — client → server

```jsonc
{"t":"hello","proto":0,"auth":{"k":"device"}}
{"t":"hello","proto":0,"auth":{"k":"device","username":"justin"}}
{"t":"hello","proto":0,"auth":{"k":"password","username":"justin","password":"…"}}
{"t":"hello","proto":0,"auth":{"k":"register","invite":"K7QP-2M4X-9WTZ",
                               "username":"justin","password":"…"}}
```

| field | |
|---|---|
| `proto` | protocol revision, currently `0`. The ALPN already refuses an incompatible peer; this catches a same-ALPN skew during development. |
| `auth.k` | `device`, `password` or `register` |

The three `auth` kinds are the whole login story (PLAN §3):

- **`register`** — once per server. Always invite-gated; there is no exception.
  (The owner account is the one account that cannot be invited, so it is
  created **locally**, by the process that already owns the machine, the
  database and the secret key. It never crosses this protocol.)
- **`password`** — once per machine. Verifies the password and **enrols this
  `EndpointId` as a device**.
- **`device`** — every time after that. No password. iroh's key exchange is the
  whole proof. `username` is needed only when one device holds several accounts
  on one server.

The password travels in this frame in plaintext *inside* TLS 1.3. That is the
moment it is most exposed and exactly what transit encryption is protecting
(PLAN §10). It is hashed with Argon2id on arrival and never stored.

### `ready` — server → client

```jsonc
{"t":"ready",
 "server_name":"Justin's Space",
 "user":    {"id":"…","username":"justin","display_name":null,"is_owner":false},
 "channels":[{"id":"…","name":"general","topic":null,"position":0}],
 "members": [{"id":"…","username":"alice","is_owner":true}],
 "online":  ["…"],
 "enrolled":true}
```

Enough to paint the whole app without a second round trip. `enrolled` is always
`true` here — every path into a session enrols the device, which is what makes
the *next* connection passwordless.

`online` is the ids of the members who have a live session at this instant —
**including the reader**, who is online by the time they read the frame. A list
that named you only when a second device of yours happened to be connected
would be an inconsistency every client had to paper over. `presence` events
(§7) keep it current from there. It is a snapshot because
presence is not stored anywhere: a database row saying "online" would be a lie
every time the host's machine lost power, and the truth is already in the set
of open connections.

### `error` — server → client

```jsonc
{"t":"error","code":"BAD_CREDENTIALS"}
{"t":"error","code":"RATE_LIMITED","retry_after":30}
```

On the control stream this ends the connection. On a request stream it answers
that one request.

| code | meaning |
|---|---|
| `BAD_CREDENTIALS` | wrong username **or** wrong password — deliberately indistinguishable, so a stranger cannot enumerate accounts |
| `INVITE_INVALID` | unknown, expired **or** exhausted invite code — also deliberately one answer |
| `USERNAME_TAKEN` | that name exists. Safe to distinguish: the client just supplied it |
| `DEVICE_REVOKED` | the owner kicked this device. Stop reconnecting and forget the space |
| `DEVICE_NOT_ENROLLED` | this `EndpointId` has no enrolment here — log in with a password once |
| `DEVICE_AMBIGUOUS` | this device holds several accounts here; say which with `auth.username` |
| `RATE_LIMITED` | backing off; `retry_after` is in seconds |
| `INVALID` | the request broke a rule in §6 |
| `NOT_FOUND` | no such channel, message or user |
| `FORBIDDEN` | the account is who it says it is and still may not do that — editing somebody else's message, or using an owner's tool without being the owner. Distinct from `NOT_FOUND` because a client can already see the author of every message and who the owner is, so there is no oracle here to protect |
| `BANNED` | the owner banned this account. Stop reconnecting and say so |
| `PROTOCOL` | unreadable frame, wrong order, or wrong `proto` |
| `INTERNAL` | the server broke. Never carries detail: an error message is an oracle, and the detail belongs in the host's log |

---

## 5. Requests and responses

### `send`

```jsonc
→ {"t":"send","channel_id":"…","content":"hit enter","nonce":"01a0…"}
← {"t":"ok"}
```

The message itself comes back on the **control stream** as a `message` event —
the same event every other member gets, so a client has one code path that
renders a message.

`nonce` is chosen by the client, unique per composed message, and never stored
by the server. The client renders optimistically the instant you hit enter,
tagged with that nonce; the server echoes it on the resulting event and the
client swaps in the authoritative row. This is why the app feels fast.

### `edit`

```jsonc
→ {"t":"edit","id":"01a0…","content":"hello"}
← {"t":"ok"}
```

**Only the author may edit**, and only a message that has not been deleted.
Anyone else gets `FORBIDDEN`; a deleted one answers `NOT_FOUND`, because as far
as everybody else is concerned there is nothing there to rewrite. Moderating
somebody else's message is the owner's tool and lands in M6.

The result comes back on the **control stream** as an `edited` event — the same
event every other member gets, so a client has one code path that applies a
change to a message.

### `delete`

```jsonc
→ {"t":"delete","id":"01a0…"}
← {"t":"ok"}
```

Only the author may, and deleting twice is not an error: a client that missed
the event and tried again is not wrong.

A **soft** delete. The row survives, because every client with the message on
screen has to be *told* to take it off — a row that simply vanished would stay
on every screen that already had it. The **content does not survive**: it is
overwritten with an empty string in the database, and is absent from the
`deleted` event. Keeping the text while calling the message deleted would make
"deleted" mean "hidden", and the host reads this database in plaintext by
design (PLAN §10) — the only way a delete means anything here is if the words
are actually gone.

### `typing`

```jsonc
→ {"t":"typing","channel_id":"…"}
← {"t":"ok"}
```

Fire and forget: nothing is stored, and `ok` says the frame was read, not that
anybody saw it. It fans out as a `typing` event (§7) to every session *except*
the one that sent it.

Throttled to one per `TYPING_THROTTLE_SECS` **per connection, on the server**,
and a frame inside the throttle is answered `ok` and dropped. A client is the
one thing that cannot be trusted to rate-limit itself, and this is the cheapest
frame to send and one of the more expensive ones to deliver — it is a broadcast
to every member.

### `backfill`

```jsonc
→ {"t":"backfill","channel_id":"…","before":"01a0…","limit":50}
← {"t":"messages","messages":[ … ]}
```

Newest first. `before` is an exclusive cursor — pass the oldest id you already
have to page backwards without re-receiving it or skipping one. Omit it to
start from the newest message. A cursor is just a message id, because ids are
UUIDv7 and sort chronologically.

### `resume`

```jsonc
→ {"t":"resume","cursors":{"<channel_id>":"<newest id held>"},"since":1789}
← {"t":"resumed","messages":[ … ],"truncated":["<channel_id>"]}
```

**Reconnect is not a reload.** The client names the newest message id it holds
per channel and gets back the gap — a flaky connection costs a few hundred
bytes rather than a re-download of every channel (PLAN §9).

`messages` is **oldest first across every channel**, in id order, so a client
applies them in the order they happened rather than channel by channel.

`since` is when the client last finished a sync, in unix seconds, and it is the
half that is easy to forget: a message **edited or deleted** while the client
was away keeps its id, so it is *older* than every cursor and no amount of
"give me what is new" would ever mention it. Without `since` a withdrawn
message stays on the returning client's screen, with its original text,
forever. Rows whose `edited_at` or `deleted_at` is `>= since` come back too —
`>=`, not `>`, because both stamps are whole seconds and a change in the same
second as the mark would otherwise fall through the gap.

`truncated` names the channels whose gap was bigger than one answer may carry.
A client must not treat those as caught up: it pages them with `backfill`
instead. The caps are in §6.

### `invite`

```jsonc
→ {"t":"invite","expires_in":86400,"max_uses":10}
← {"t":"invite","code":"K7QP-2M4X-9WTZ","expires_at":1789,"max_uses":10}
```

Both request fields are optional: no `expires_in` never expires, no `max_uses`
is unlimited. Any account may invite; the row records who did, which is what
the owner's "revoke invite" tool (M6) works from.

The response carries the code alone, because the server cannot know where the
client should tell people to look: only the process that owns the space's
endpoint knows its own relay and direct addresses. A client turns the code into
a link (§8) using the address it dialled — with hints if it is the host, and
with the bare key otherwise, which discovery resolves.

---

### Owner tools

Every one of these is reachable only over this socket, so **every one is
authorised on the server**. A client that hides a button is a convenience; the
refusal is the security boundary.

| request | who may |
|---|---|
| `create_channel`, `delete_channel` | the owner |
| `kick`, `set_banned` | the owner |
| `invites`, `revoke_invite` | the owner |
| `revoke_device` | the owner, for anyone; everyone else, for their own |
| `devices` | the owner, for anyone; everyone else, for their own |

#### `create_channel` / `delete_channel`

```jsonc
→ {"t":"create_channel","name":"scratch","topic":"temporary"}
← {"t":"channel","channel":{"id":"…","name":"scratch","topic":"temporary","position":1}}

→ {"t":"delete_channel","id":"…"}
← {"t":"ok"}
```

A delete takes **every message in the channel** with it, by the schema's
cascade. History in a channel nobody can open is not kept, it is stranded.

The server **refuses to delete the last channel** (`FORBIDDEN`). A space with
no channel has nowhere to put a message, so the next person to speak would
find a dead end and the owner would have no way back but the database.

Both fan out a `channels` event (§7) carrying the whole new list.

#### `kick` / `set_banned`

```jsonc
→ {"t":"kick","user_id":"…"}
← {"t":"ok"}

→ {"t":"set_banned","user_id":"…","banned":true}
← {"t":"ok"}
```

These are two different things and the difference is the point:

- **`kick`** revokes every device enrolled for the account. The password still
  works, so the member can enrol a machine again and come back. It is the
  proportionate answer to a laptop left in a pub.
- **`set_banned`** revokes every device *and* refuses the password, so the
  account cannot get back in at all. It is **reversible** — an irreversible
  action one misclick away is worse than one that can be undone — and the
  account and its messages survive it, so un-banning is not a
  re-registration.

Neither may target **the owner** (`FORBIDDEN`). A space is administered through
the owner's own client; locking it out would leave the space running with
nobody able to reach its tools.

Both send `revoked` (§7) to the sessions they concern, then close those
connections. `set_banned` also fans out `members`.

#### `revoke_device` / `devices`

```jsonc
→ {"t":"revoke_device","user_id":"…","endpoint_id":"…"}
← {"t":"ok"}

→ {"t":"devices"}                       // your own
→ {"t":"devices","user_id":"…"}         // the owner, asking about anyone
← {"t":"devices","devices":[
     {"endpoint_id":"…","user_id":"…","label":"Justin's laptop",
      "enrolled_at":1789,"last_seen":1789,"current":true}]}
```

Revoking **one enrolment** is not a kick: the account's other machines stay
connected. Revoking your own is how you log a machine out, and `current` says
which entry that is — the client cannot know which connection it is reading on.

Revoked devices stay in the list, with `revoked_at` set. A device that vanished
is one the owner cannot tell they revoked.

A revoked key is **not blacklisted**: the password can enrol it again. What was
taken away is the passwordless login, which is the property that matters
(PLAN §3).

#### `invites` / `revoke_invite`

```jsonc
→ {"t":"invites"}
← {"t":"invites","invites":[
     {"code":"K7QP-2M4X-9WTZ","created_by":"…","created_at":1789,
      "max_uses":10,"uses":1}]}

→ {"t":"revoke_invite","code":"K7QP-2M4X-9WTZ"}
← {"t":"ok"}
```

The listing carries the codes: an owner who cannot read a code back cannot tell
which invite they are revoking. Revoking one kills the code and **leaves the
accounts already made with it alone** — they are members now, and an invite is
a door, not a lease.

---

## 6. Limits

Validated on **both** sides, by the same functions in `he-proto::limits`, so
the two can not drift apart. A client validates for fast feedback; a server
validates because it can never trust a client.

| | |
|---|---|
| frame body | ≤ 1 MiB |
| message content | 1–4000 characters, not all whitespace |
| username | 3–32 characters of `[a-zA-Z0-9_.-]`, compared case-insensitively |
| password | 8–1024 **bytes** |
| channel name | 1–64 characters |
| id | 1–64 characters of `[a-zA-Z0-9-]` |
| nonce | 1–64 bytes |
| invite code | 1–64 bytes of `[a-zA-Z0-9]`, `-`, `_` or space; formatting ignored |
| backfill `limit` | 1–200, default 50 |
| `resume` cursors | at most 200 channels — one query each |
| `resume` messages | at most 500 in total, at most 200 from any one channel |
| typing | one per connection per 3s, on the server; indicators expire after 8s |
| channel topic | 0–200 characters |

An invite code's *shape* is checked by both sides; whether it is **real** is
answered only by the server, in constant time, so that trying is not an oracle.
Formatting is deliberately forgiving — a code is read aloud, retyped and pasted
out of chat messages, so `K7QP-2M4X-9WTZ`, `k7qp2m4x9wtz` and one with a stray
space are the same code.

The server also caps, globally and before anyone has proved who they are:
live connections, handshakes in flight (lower, because a handshake can cost
~100 ms of Argon2id), and request streams per session. Failed logins earn
exponential backoff keyed on **both** the `EndpointId` and the username — an
`EndpointId` is free to generate, so neither key alone is enough.

---

## 7. Events

Server → client on the control stream, after `ready`.

### `message`

```jsonc
{"t":"message",
 "message":{"id":"01a0…","channel_id":"…","author_id":"…","author_name":"alice",
            "content":"hit enter"},
 "nonce":"01a0…"}
```

`nonce` is present **only on the connection that sent the message**. Everyone
else gets the identical event without one, because only the sender has an
optimistic bubble to reconcile. Note that this is per *connection*, not per
account: a second client logged into the same account sees it as somebody
else's message, which is correct — it did not compose it.

`author_name` is denormalised so that rendering a backfilled page needs no
second lookup and a client mirror can answer offline.

`content` is plaintext here and at rest, by design (PLAN §10) — and encrypted
for its entire journey by QUIC + TLS 1.3, including past any relay.

### `edited`

```jsonc
{"t":"edited","message":{ …the whole row, with "edited_at" set… }}
```

Carries the **whole message** rather than a patch, so a client that never saw
the original still ends up with the right text. Applying it is the same
idempotent write as storing a new message.

### `deleted`

```jsonc
{"t":"deleted","id":"01a0…","channel_id":"…","deleted_at":1789}
```

No content, because there is no longer any. Clients are *told* rather than the
message being silently skipped: one already on somebody's screen has to be
taken back off it.

### `presence`

```jsonc
{"t":"presence","user_id":"…","online":true}
```

Per **account**, not per connection. A member with a laptop and a phone comes
online when the first of the two connects and goes offline when the second
disconnects — which is what the dot next to their name is claiming. The
connection that *caused* an arrival is not told about it; it already knows, and
a second device on the same account still is.

### `typing`

```jsonc
{"t":"typing","channel_id":"…","user_id":"…","username":"alice"}
```

Never sent back to the connection that said so. Nothing is stored and nothing
is guaranteed to arrive: an indicator that missed its renewal expires on its
own after `TYPING_TIMEOUT_SECS`, which is why there is no "stopped typing"
frame to lose.

`username` is denormalised for the same reason as `message.author_name`: the
indicator has a name to show before any member list has loaded.

### `channels` / `members`

```jsonc
{"t":"channels","channels":[{"id":"…","name":"general","topic":null,"position":0}]}
{"t":"members","members":[{"id":"…","username":"alice","is_owner":true}]}
```

Both carry the **whole list**, exactly as `ready` does, and a client
**replaces** rather than merges. A merge can only ever add, and the changes
worth announcing — a channel deleted, a member banned — are the ones a merge
would silently drop.

`members` goes to every session whenever the roster changes: after
`set_banned`, and after a `hello` that **registers** an account — to every
session except the one that just registered, whose `ready` already carried the
same list.
Nothing is sent for a change that happened while a client was disconnected;
its next `ready` carries the current lists, and a client applies those the
same way.

### `revoked`

```jsonc
{"t":"revoked"}
```

Sent to the sessions it concerns and to nobody else, immediately before the
server closes their connections. It is the last frame on that stream and is
flushed before the close, because a QUIC close discards unacknowledged stream
data — and "you were kicked" arriving as "connection lost" is precisely the
thing a client would retry through.

It carries **no reason**. The two cases a client might tell apart — this device
was revoked, this account was banned — both mean "you are out, and retrying
will not help", and a client that treats them differently is a client that
retries one of them.

**A client that receives it must stop reconnecting.** Not back off; stop. The
answer will not change until somebody decides otherwise, and a client hammering
a space it was ejected from is indistinguishable from the thing being ejected
for.

---

## 8. Invite links

Not a frame — a link is the one string a person sends another person, and it is
what a client turns into the address it dials and the `invite` it puts in a
`hello`. It lives in `he-proto` (`ticket.rs`) because both sides need it and
neither may own it (PLAN §5).

```
hitenter://join?t=<iroh endpoint ticket>&c=K7QP-2M4X-9WTZ
```

| | |
|---|---|
| `t` | an iroh `EndpointTicket`: the space's `EndpointId`, plus the relay and direct addresses it knew about **itself** when the link was minted |
| `c` | the registration code, gating account creation (§4, PLAN §11). **Optional** |

Both values are restricted to characters that need no percent-encoding, so a
link survives being pasted through a chat client. `hitenter:join?…` — one
slash — parses identically, because which form comes back depends on the chat
client. An **unknown parameter is ignored**, so a link from a newer version
still joins rather than failing to parse.

### What the two halves are for

- **Without `c`** the link is an address and nothing else: enough to point a
  second machine *of your own* at a space, where the password enrols the device
  and no invite is involved (PLAN §3). Not enough for a stranger to register.
- **With `c`** it is a complete invite: everything needed to find the space and
  be allowed in.

### Hints go stale; the key does not

The addresses inside `t` are a snapshot of where the space was reachable when
the link was made. They make the first connection fast. When they are wrong,
discovery re-resolves from the `EndpointId` and the link still works — just
more slowly. **Mint a fresh link rather than storing one.**

A link minted before the hosting endpoint has reached a relay carries only
local addresses, which is the difference between "a friend in another city
joins" and "it worked on my machine". `he-serverd --invite` and the desktop
app both wait, briefly and boundedly, for a relay before printing one.

### There is nothing to verify

The address **is** an ed25519 public key. A link that has been tampered with
does not point at an impostor's server; it points at a key nobody holds, or at
no readable ticket at all. There is no fingerprint for two people to compare
and nothing to pin (PLAN §5).

### What a client accepts

A client is liberal about what it reads, because the alternative is telling
somebody who pasted the two halves of an invite that their invite is not a
link. `InviteLink::parse_relaxed` takes:

- a whole `hitenter://join?…` link;
- a bare `endpoint…` ticket, optionally followed by a code;
- a bare `EndpointId`, optionally followed by a code.

It is strict about what it *writes*: the canonical form above, always.

---

## 9. Delivery guarantees

Worth stating plainly, because the honest answer is not "exactly once".

**A message is delivered at least once.** A client writes to its outbox before
it sends, and clears the entry when the server answers `ok` or echoes the nonce
back — whichever happens first. If the connection dies *between* the server
storing the message and either of those reaching the client, the entry is still
in the outbox and the next reconnect sends it again, producing a duplicate.

The alternative is dropping anything we are unsure about, which loses words the
user typed. This way round the failure is visible and the user can delete the
duplicate; the other way round there is nothing to see and nothing to fix. The
window is one round trip wide and the outcome is a repeated message, so it is
the right trade — but it is a trade, not an accident.

`nonce` does not close it: the server never stores one, so it has nothing to
compare a redelivery against. Making it dedupe would mean keeping every nonce
for as long as a client might retry, which is a table that only grows to
prevent something the user can already fix in one click.

---

## 10. What is not on the wire

`hit-enter/0` carries exactly what is documented above, and a frame that is not
documented above is answered `PROTOCOL`.

Everything PLAN §9 specified is now on the wire. What is *not* here, and is
deliberately not planned before 1.0:

| frame | why not |
|---|---|
| renaming or re-ordering channels | `create` and `delete` cover the need; re-ordering is a drag handle and a `position` write, and it is M7's problem |
| roles beyond `owner` / `member` | a permission matrix is explicitly out of scope (PLAN §1) |
| a reason attached to `revoked` | see above: a client that can tell the cases apart is a client that retries one of them |

Any addition updates this file in the same commit, and only a change that
breaks an existing frame bumps the ALPN past `0`.
