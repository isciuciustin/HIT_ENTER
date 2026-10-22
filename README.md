# HIT_ENTER 1.0

A privacy-first, Discord-shaped chat app with **no server anybody has
to run.** Group chats with channels, DMs, text, files, voice channels — and
**zaps**: tip a message or a file in sats, straight from one person's wallet
to another's (HIT_ENTER adds a 1% fee, shown before you send).

**Open source, [MIT licence](LICENSE).** Download the app at
[hitenter.app](https://hitenter.app).

- **Nostr** is who you are: a keypair nobody issued and nobody can ban. Your
  existing Nostr identity works here.
- **iroh** is how machines talk: peer-to-peer QUIC through home routers, no
  port forwarding, no public IP.
- **Every running app is a relay** for the groups its user is in. A group is
  alive while any member is online, not just its owner. An always-on
  **anchor** — the same program, headless — keeps it alive when nobody is.

**Beta: v0.1.0-beta.4** — Linux, macOS (Apple silicon) and Windows. Start
here:

| | |
|---|---|
| [`docs/DESIGN.md`](docs/DESIGN.md) | why "every person is a relay" works, and the four things it does not give you for free |
| [`docs/PROTOCOL.md`](docs/PROTOCOL.md) | every event kind the app emits — what leaves your machine |
| [`docs/PLAN.md`](docs/PLAN.md) | the milestones |
| [`docs/SELF_HOSTING.md`](docs/SELF_HOSTING.md) | running your own anchor |
| [`docs/TESTING.md`](docs/TESTING.md) | building, testing and trying it by hand |

```bash
cargo run --release -p hit-desktop    # the app
cargo test --workspace                # the tests
```

The third HIT_ENTER. The first proved iroh between two cities; HIT_ENTER_2
built global accounts, roles, attachments and paid hosting. This one
replaces the accounts with Nostr and the host with everybody.

`#WAR_ON_CENTRALIZATION`
