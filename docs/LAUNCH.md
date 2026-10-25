# Getting HIT_ENTER known — fast and free

Written 2026-10-25, after beta.5. beta.4 had 11 app downloads, some of
them ours. The recommendation is to do these in this order.

## 0. Before telling anyone: the first strangers must not hit a broken app

You get one first impression on Hacker News. Still untested: two people on
two different networks (`TESTING.md` §2), a call between cities (§6), and
M13's final check. Run them with a friend first. If the first 200
installers cannot reach each other, the thread says "doesn't work", and
that sticks.

## 1. Nostr first: the warmest audience there is

They already have the identity, the wallets for zaps and the appetite for
this. Our profiles land on Damus and Primal, so they see our users there.

- Post from the owner's npub on Primal (draft below), with a 20–30 s
  screen recording: a channel, a reaction, a zap, a browse search.
- **A public "HIT_ENTER" group on the anchor must exist before the
  post**, so people have somewhere to land that isn't empty.
- Tell the NIP-29 people (Chachi, 0xchat): their clients can read our
  groups through the anchor's bridge. **Run `TESTING.md` §5 before saying
  so.**
- Apply for an **OpenSats** grant: free money, and the listing is
  publicity.

## 2. Show HN, once Nostr has shaken out the obvious bugs

Post Tuesday to Thursday, 8–10 am US Eastern, and stay in the comments.
The same week: r/selfhosted (the anchor on a Pi or a $5 VPS), r/rust
(Slint + iroh), r/opensource.

## 3. One real community: the north star in small form

Chat apps grow by groups, not downloads. Move one small community that is
unhappy with Discord (an Unturned server, a modding group, a dev channel),
with the owner as its admin and support. Fifty people who live in it are
worth more than 5,000 downloads, and they are the proof for the next
community.

**Deliberately not:** Product Hunt (little lasting effect for open
source), paid ads, many social accounts, ranting against Discord (the
numbers say it better).

---

## Draft: the Nostr post

> **HIT_ENTER: Discord-style communities on Nostr, without a server.**
>
> Groups with channels and roles, text, files, voice calls, DMs and zaps.
> Your npub is your account and your groups are NIP-29. Every running app
> is a relay for the groups its user is in, and devices talk peer to peer
> over iroh (QUIC).
>
> No sign-up, no email. Import your nsec or make a new one. It lives in
> your OS keychain and never leaves it.
>
> 💬 Public communities (new in beta.5): make a group public, browse and search them, read one before joining
> 🔒 DMs are NIP-17, end-to-end encrypted
> ⚡ Zaps on any message (NWC or QR), wallet to wallet; we never hold funds
> 🎙 Voice, up to 8 people, peer to peer
> 🪶 32 MB of RAM on the sign-up screen, 62 MB with a 1,000-message channel open. Discord idle on the same PC: 533–745 MB.
>
> Want a group online when you're not? Run an "anchor": the same node,
> headless, on a Pi or a $5 VPS. It can also serve your group to other
> NIP-29 clients over wss://.
>
> It's a beta. Linux, macOS (Apple silicon) and Windows. Open source, MIT,
> written in Rust.
>
> ⬇️ https://hitenter.app
> 🧑‍💻 https://github.com/isciuciustin/HIT_ENTER
> 👋 Come say hi in our public group: [link]
>
> Bugs, ideas and rants all welcome, here or on GitHub.
>
> #nostr #grownostr #opensource #rust

## Draft: Show HN

**Title** (76 characters, under HN's 80 limit):
`Show HN: HIT_ENTER – Discord-like chat where every app is a relay, in 60 MB`

**URL:** `https://github.com/isciuciustin/HIT_ENTER`

**First comment** (post it right after submitting):

> Hi HN. I've been building a Discord-style chat app with no company
> server in the middle: groups with channels and roles, text, files, voice
> and DMs.
>
> **How it works:**
> - Identity is a Nostr key (secp256k1), kept in the OS keychain. No
>   account, no email, no phone number.
> - The data model is Nostr events. Groups follow NIP-29, DMs use NIP-17
>   (end-to-end encrypted).
> - The transport is iroh (QUIC, with hole punching and relay fallback),
>   not WebSockets. Every running app stores and serves the groups its
>   user is in, and peers sync by difference with negentropy (NIP-77).
> - There's no central authority, so every peer checks every event
>   itself: signature first, then permissions. A permission check is a
>   pure function over the group's history, the same on every peer. A
>   client that lies gets ignored by everyone else.
> - For groups that should stay online while everyone sleeps, you can run
>   an "anchor": the same node, headless, on a Pi or a VPS. It can also
>   bridge to other Nostr clients over wss://.
>
> **Why it's small:** the first version was Tauri and used about 300 MB
> signed in. I rewrote the window in Slint with a software renderer.
> Measured on the same Linux PC (whole-app PSS): 32 MB on the sign-up
> screen, 51 MB signed in, 62 MB with a 1,000-message channel open, 55 MB
> in a voice call. Discord idle: 533–745 MB. On an M1 it's 64 MB idle.
>
> **What's honestly not there yet:**
> - It's a beta, and so far it has mostly been tested on my own machines.
>   Calls between cities in particular haven't been tested properly yet.
> - Voice is a mesh capped at 8 people. There's no echo cancellation on
>   Windows yet, so use a headset there.
> - No mobile app, no Intel Mac build, and the builds aren't code-signed,
>   so expect SmartScreen and Gatekeeper warnings.
> - It's peer to peer, so people in your group can see your IP address, as
>   in any P2P app.
> - Group messages are not end-to-end encrypted. They're plaintext to the
>   members and anchors who hold them, and the app says so. DMs are
>   encrypted.
> - Zaps (Lightning tips) go wallet to wallet, and the app never holds
>   funds. There's a 1% fee, sent as a separate payment on top of the zap
>   and shown before you send. That's the only revenue idea so far.
>
> It's Rust and MIT licensed. The design docs say what I chose and what I
> gave up: `docs/DESIGN.md` and `docs/PROTOCOL.md`.
>
> I'd especially love feedback on the authorization model (no server,
> every peer judges) and on what would stop you from moving a community
> off Discord.

**About the fee:** users cannot turn it off. `fee_on` in
`hit-app-core/src/state.rs` means only that the build has a fee address,
and release builds always do. So the comment says "a 1% fee", not "an
optional 1% fee". HN will ask why someone cannot just build it without
one. The honest answer is that they can: it is MIT.
