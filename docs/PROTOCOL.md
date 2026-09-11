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
  M5 stops changing shapes (PLAN §9).

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
server → client:   message, …                  (events, until the session ends)
```

After `ready` the client never writes on this stream again.

### Request streams

One bi-directional stream per RPC. The client writes a request, reads a
response, and closes:

```
client → server:   send | backfill | invite    (exactly one frame)
server → client:   ok | messages | invite | error
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
 "enrolled":true}
```

Enough to paint the whole app without a second round trip. `enrolled` is always
`true` here — every path into a session enrols the device, which is what makes
the *next* connection passwordless.

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

### `backfill`

```jsonc
→ {"t":"backfill","channel_id":"…","before":"01a0…","limit":50}
← {"t":"messages","messages":[ … ]}
```

Newest first. `before` is an exclusive cursor — pass the oldest id you already
have to page backwards without re-receiving it or skipping one. Omit it to
start from the newest message. A cursor is just a message id, because ids are
UUIDv7 and sort chronologically.

### `invite`

```jsonc
→ {"t":"invite","expires_in":86400,"max_uses":10}
← {"t":"invite","code":"K7QP-2M4X-9WTZ","expires_at":1789,"max_uses":10}
```

Both request fields are optional: no `expires_in` never expires, no `max_uses`
is unlimited. Any account may invite; the row records who did, which is what
the owner's "revoke invite" tool (M6) works from.

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
| backfill `limit` | 1–200, default 50 |

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

---

## 8. Not yet on the wire

`hit-enter/0` carries exactly what is documented above. These are specified in
PLAN §9 and land in later milestones; a client must not send them and a server
answers `PROTOCOL` if it receives one:

| frame | milestone |
|---|---|
| `edit`, `delete` requests · `edited`, `deleted` events | M5 |
| `resume` request (per-channel cursors, so reconnect is not a reload) | M5 |
| `typing` request · `presence`, `revoked` events | M5 |

Adding any of them updates this file in the same commit. None of them breaks an
existing frame, so the ALPN stays at `0`.
