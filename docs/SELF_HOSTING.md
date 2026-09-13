# Hosting a HIT_ENTER space

You need: a computer, and somebody to talk to. Not a server, not a domain, not
a port forward, not an account with anybody — including us, because there is
no us.

There are two ways to host, and the first one is the answer for almost
everybody.

---

## 1. From the app

Open HIT_ENTER, click **host a space**, and give it a name, a username and a
password.

That is the whole thing. Your machine is now serving a space that people can
join from anywhere.

### What just happened

Worth thirty seconds, because it explains every surprise later:

1. A `server.db` was created in your data directory. Creating it generated the
   **space's** identity — a keypair that *is* the space's address. It is a
   different key from the one this installation of the app already had for
   itself (PLAN §3).
2. Your **owner account** was created locally. It is the one account an invite
   cannot gate — there was nobody to invite you — so it never crosses the
   network.
3. Your own client then **dialled the space over the network**, by its public
   key, with the password you just set. Exactly as a stranger's client will.

That last step is not a technicality. There is no "host mode" in this app: the
owner's client uses the same handshake, the same enrolment and the same mirror
as everybody else, on every single run. A bug in any of them is your bug first,
which is the point.

### Inviting people

**invite** in the channel list footer, then **copy link**. Paste it into
whatever you already use to talk to that person.

The link carries two things: where the space is, and a code that permits making
an account. Registration is invite-gated without exception, so the code is what
keeps strangers out — and anyone who has the link can join once, so treat it
like a door key rather than a URL.

You can see and revoke every code you have ever minted in the same dialog.
Revoking one kills the code; the people who already used it stay members.

> **Mint the link, do not save it.** The addresses inside go stale as your
> machine moves between networks. The public key never does, so an old link
> still works — it just takes longer to connect while discovery catches up.
> A fresh link is one click.

### What your friends need

Nothing but the link and the app. No account with anybody, no invite to a
platform, no port forwarding on *their* router either.

---

## 2. Headless, with `he-serverd`

For a machine with no desktop — a spare laptop in a cupboard, a Raspberry Pi, a
VPS you happen to have. Same server, same database, no window.

```bash
# once: create the space and the owner account
he-serverd --data-dir ./space --owner yourname

# once per invite
he-serverd --data-dir ./space --invite --max-uses 5

# serve. ^C to stop.
he-serverd --data-dir ./space
```

The password comes from `HE_PASSWORD` or from standard input, **never from an
argument** — arguments are visible to every account on the machine through
`ps`.

`--data-dir` defaults to `./he-data`. Everything the space is lives in there:
the database, the accounts, the messages, and the secret key.

### Running it as a service

```ini
# /etc/systemd/system/hit-enter.service
[Unit]
Description=HIT_ENTER space
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=hitenter
ExecStart=/usr/local/bin/he-serverd --data-dir /var/lib/hit-enter
Restart=on-failure
RestartSec=5
# The database holds the space's secret key. Nothing else needs to read it.
UMask=0077
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
ReadWritePaths=/var/lib/hit-enter

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now hit-enter
journalctl -u hit-enter -f
```

There is no port to open, because there is no port. iroh uses an outbound UDP
socket and negotiates the rest.

---

## 3. Back up `server.db`, or lose the space

One file matters more than all the others:

```
<data-dir>/server.db
```

It holds `server_meta.secret_key`, and **that key is the space's address**.

- **Lose it** and every invite link ever issued points at nothing. Your members
  cannot reconnect. There is no recovery: a new key is a new space, and
  everybody has to join again from scratch.
- **Leak it** and somebody else can impersonate your space to your members.

Treat it exactly like an SSH host key. It is `0600` on disk and never appears
in a log, and it must never appear in a bug report either.

```bash
# stop the server first: copying a live SQLite file can catch it mid-write
systemctl stop hit-enter
cp -a /var/lib/hit-enter/server.db /somewhere/safe/
systemctl start hit-enter
```

Everything else in that file — accounts, messages, invites — is recoverable
only from this backup too, but only the key is *irreplaceable*.

---

## 4. What your members can see, and what you can

Say this to people before they join, not after:

- **You can read every message in your space.** All of it, in plain text, in an
  ordinary SQLite file. There is no end-to-end encryption, deliberately
  (PLAN §10) — it is what makes offline search, real moderation and one-file
  backups possible, and it means there is no key anybody can lose and take
  their history with them.
- **You cannot read their passwords.** Ever. They are Argon2id hashes and no
  code path in this project turns one back into a password.
- **Nobody in the middle can read anything.** Every connection is QUIC +
  TLS 1.3, including when it goes through a relay. A relay forwards bytes it
  cannot read.
- **Their copy is theirs.** Every member mirrors the whole conversation to
  their own disk. If you switch your machine off, nobody loses their history —
  they just cannot add to it until you are back.

```bash
# what "you can read everything" means, concretely
sqlite3 /var/lib/hit-enter/server.db \
  'select u.username, m.content from messages m join users u on u.id = m.author_id'
```

---

## 5. Relays and discovery

By default a space uses relays and DNS discovery operated by
[n0](https://www.iroh.computer/), the authors of iroh. They are free,
rate-limited, and documented by n0 as having **no uptime guarantee**. They are a
convenience, and they are replaceable — which is the only reason depending on
them is acceptable.

What each one does:

- **Discovery** turns a public key into an address. Without it, a link whose
  address hints have gone stale cannot be resolved at all.
- **A relay** forwards traffic when two machines cannot punch a direct path
  through their networks. It sees who talks to whom and when. It cannot see
  what they say.

Both are in **settings** (`Ctrl + ,`), and the whole ladder is available:

| rung | what it needs | what it costs |
|---|---|---|
| LAN only | nothing at all | works with the internet unplugged; nobody outside your network can join |
| default | n0's relays and DNS | zero configuration; best-effort infrastructure you do not control |
| your own relay | a machine with a public address | see below |
| no discovery | you distribute fresh links yourself | links go stale when your address changes |

### Running your own relay

The relay is open source and ships in the iroh repository. It is a separate
program from this one and we do not wrap it.

```bash
cargo install iroh-relay --features server

# try it: plain HTTP on port 3340, no certificate needed
iroh-relay --dev
```

That is enough to point a couple of machines on your own network at
`http://localhost:3340` and watch it work. For anything real it wants a
hostname and TLS — the one place in this whole document a certificate appears,
and it is the *relay's* certificate, not the space's:

```toml
# relay.toml
http_bind_addr = "[::]:80"      # Let's Encrypt does its HTTP-01 check here

[tls]
https_bind_addr = "[::]:443"
hostname = "relay.example.com"
cert_mode = "LetsEncrypt"
contact = "you@example.com"
```

```bash
iroh-relay --config-path relay.toml
```

`cert_mode = "Manual"` with `manual_cert_path` and `manual_key_path` uses
certificates you already have instead.

Then in **settings → relays → your own relay**, one URL per line:

```
https://relay.example.com
```

Two things to know before you do this:

- **Everyone who joins your space needs the same relay list.** A relay is not
  discovered; it is configured. If you use your own and your members use n0's,
  the pairs that cannot punch a direct path will not connect at all.
- **A relay only matters when hole punching fails.** Most connections never
  touch one. Check the connection dot in the app: green is direct, amber is
  relayed, and amber is *working* — slower, not broken.

Consult iroh's own documentation rather than this file for the relay's
options; it is their program and it changes on their schedule.

---

## 6. When something is wrong

**Nobody can join, and the link is fresh.**
Check the dot next to your space. If it never leaves "connecting", your network
is blocking outbound UDP — some corporate and hotel networks do. A relay is the
fallback for exactly this, so check that relays are not switched off in
settings.

**A link only works on your own network.**
It was minted before the space reached a relay, so it carries local addresses
and nothing else. The app warns about this when it happens. Wait a moment and
mint another.

**A member is stuck at "connecting" and everybody else is fine.**
Their network refuses both a direct path and, if they have turned relays off,
the fallback. Ask them what settings they are on.

**The window never opens on Linux.**
```
Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
```
WebKitGTK's dmabuf renderer and the proprietary NVIDIA driver. The packaged
builds and the binary both set `WEBKIT_DISABLE_DMABUF_RENDERER=1` for you; if
you built it yourself and ran the binary directly, set it by hand.

**I want to start over.**
Delete the data directory. A new space gets a new key and every old invite is
dead — which is the same sentence as "back up `server.db`", read from the other
end.

---

## 7. Everything is a file you can open

There is no admin API, no dashboard, and no support channel, because there is
nothing here you cannot read yourself:

```bash
sqlite3 <data-dir>/server.db 'select username, is_owner, banned_at from users'
sqlite3 <data-dir>/server.db 'select endpoint_id, revoked_at from devices'
sqlite3 <data-dir>/server.db 'select code, uses, max_uses from invites'
cat <data-dir>/settings.json
```

Editing `settings.json` by hand is a supported way to use this. A corrupt one
starts the app with the defaults rather than refusing to open.

The wire protocol is written down in [`PROTOCOL.md`](PROTOCOL.md), and
`he-cli` speaks it, because there is no curl for a QUIC protocol:

```bash
he-cli --server <link> members
he-cli --server <link> invites
he-cli --server <link> --help
```
