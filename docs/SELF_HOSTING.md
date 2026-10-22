# Self-hosting an anchor

An **anchor** is the group's always-on peer (`DESIGN.md` §9): the same node
as the app, headless, on a Raspberry Pi at home or a €4 VPS. With one, a
group is alive at 4am — a new member can join from an invite and read the
history while every member's app is closed — and, if you want, NIP-29
clients (Chachi, 0xchat) can read the group through it.

What an anchor is, plainly:

- **A member of each group it pins.** It joins with an invite link like
  anyone, shows in the member list as *anchor*, and **can read everything
  the group can** — in plain text, on its disk (`DESIGN.md` §7). Run it
  where you would keep the group's history yourself.
- **Not an authority.** It judges every event by the same rules as every
  member's app; it cannot ban, edit or post unless you give it a role.
- **Optionally the holder of a group key**, so the group is compacted while
  its owner is away. Whoever holds the group key has the group's last word.
- **Optionally a Nostr relay** on `ws://` for NIP-29 clients, behind your
  own TLS proxy for `wss://`.

What it costs: a few MB of RAM per group at rest, a UDP port, and the disk
for the history. A Pi Zero 2 is plenty for a group of friends.

---

## 1. Build

On the machine (or cross-compile and copy):

```bash
cargo build --release -p hit-anchor
sudo install -m 755 target/release/hit-anchor /usr/local/bin/
sudo useradd --system --home /var/lib/hit-anchor --create-home hit-anchor
```

## 2. Its identity — a key that never sits in a plain file

A server has no desktop keychain, so the anchor reads its key from **systemd
credentials**: encrypted at rest with the host's key (or a TPM), handed
decrypted only to the running service, in memory. `init` writes the key to
standard output **only into a pipe** — never to a terminal:

```bash
sudo -u hit-anchor hit-anchor --data /var/lib/hit-anchor --credentials \
    init --name "Friends' anchor" \
  | sudo systemd-creds encrypt --name=NPUB - /etc/credstore.encrypted/NPUB
```

`init` prints the npub on stderr; use it for `NPUB` in both places (the
credential is named after it — run `init`, read the npub from the terminal,
then repeat the pipe with the right name if you guessed wrong: delete
`/var/lib/hit-anchor/anchor.json` first). Back the encrypted credential up
somewhere else if losing the anchor's identity would matter; usually it
does not — a new anchor can be pinned in a minute.

## 3. Pin a group

A member (the owner, say) makes an invite link in the app and sends it to
you. Then:

```bash
sudo systemd-run --pipe --wait -p User=hit-anchor \
  -p LoadCredentialEncrypted=NPUB:/etc/credstore.encrypted/NPUB \
  hit-anchor --data /var/lib/hit-anchor --credentials pin 'hit-enter:join?g=…'
```

From then on the anchor is a member. Invite links made afterwards name it
first. **An invite works at 4am only if it reached the anchor before its
maker went offline** — make the link with the app open for a few seconds
after.

## 4. Run it as a service

`/etc/systemd/system/hit-anchor.service`:

```ini
[Unit]
Description=HIT_ENTER anchor
After=network-online.target
Wants=network-online.target

[Service]
User=hit-anchor
LoadCredentialEncrypted=NPUB:/etc/credstore.encrypted/NPUB
# A group key, if you hold one for compaction (section 6):
# LoadCredentialEncrypted=GROUP_NPUB:/etc/credstore.encrypted/GROUP_NPUB
ExecStart=/usr/local/bin/hit-anchor --data /var/lib/hit-anchor --credentials \
    --port 7600 run --ws 127.0.0.1:7777 --ws-url wss://anchor.example.org
Restart=on-failure
# The anchor needs nothing else from the system.
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/hit-anchor
ProtectHome=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload && sudo systemctl enable --now hit-anchor
journalctl -u hit-anchor -f
```

**Open UDP 7600** on the firewall (and forward it on a home router) so
members reach the anchor directly; without it they still reach it through
iroh's relays, more slowly. It stops cleanly on `systemctl stop`
(SIGTERM): it hangs up rather than vanishing.

## 5. `wss://` for NIP-29 clients (optional)

The anchor serves plain `ws://` on `127.0.0.1:7777` and leaves TLS to a
proxy. With Caddy (`/etc/caddy/Caddyfile`), which also fetches the
certificate:

```caddyfile
anchor.example.org {
    reverse_proxy 127.0.0.1:7777
}
```

Pass the same public URL as `--ws-url`: an AUTH naming another relay is
then refused. In Chachi or 0xchat, add the relay `wss://anchor.example.org`
and sign in **with the same key you use in HIT_ENTER** (import your nsec
there, or use a signer): the anchor asks every client to authenticate
(NIP-42) and answers as that person — a member reads the group, anyone else
reads nothing. **Reading only, for now**: the group refuses messages posted
from those clients, because they have no channel, and says so.

## 6. Compaction while the owner is away (optional)

If the owner hands the anchor the group's key, the anchor compacts the
group every `--compact-every-hours` (default 24) when anything happened —
new members don't replay the group's whole moderation history. On the
owner's machine:

```bash
hit-cli group export-key "Friends" \
  | ssh anchor 'sudo systemd-creds encrypt --name=GROUP_NPUB - /etc/credstore.encrypted/GROUP_NPUB'
```

`export-key` refuses to print to a terminal. Add the
`LoadCredentialEncrypted=` line for it to the unit and restart.
**Whoever holds a group key has the group's last word** (`DESIGN.md` §6):
hand it only to a machine you control.

## 7. Your own iroh relay (optional)

By default the anchor and the apps use n0's relays and discovery: they carry
encrypted bytes they cannot read (`DESIGN.md` §1). To stop depending on
n0's relay, run n0's `iroh-relay` (its own binary, MIT/Apache-2.0) on the
VPS and point the anchor at it:

```bash
cargo install iroh-relay --features server
iroh-relay --config-path /etc/iroh-relay.toml   # see iroh-relay's docs for TLS
hit-anchor … --relays https://relay.example.org run …
```

The apps keep using n0's relays until they get the same setting (later).

## 8. Upgrades and backups

- **Upgrade:** build, `install`, `systemctl restart hit-anchor`. The store
  migrates itself; migrations are append-only.
- **Back up:** `/var/lib/hit-anchor` (the history, and `device.key` —
  this machine's device identity) and the encrypted credentials. Losing the
  data directory loses nothing the members do not also hold.
- **On a Pi:** an SD card wears out; keep `/var/lib/hit-anchor` on a USB
  SSD if the group is busy.

## What an anchor does not do (yet)

- Fetch and keep files — that is M8, with files.
- Serve DMs as an inbox (kind 10050) — M7.
- Let NIP-29 clients post, or join from them.
