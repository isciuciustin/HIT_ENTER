# Testing by hand

What the automated tests cannot show: real processes, real kills, and —
the part that matters most — **two machines on different networks**. As
HIT_ENTER_2's `TESTING.md` found, the bugs live where the network is off or
far away, not on loopback.

`cargo test --workspace` covers the logic. Each milestone's "done when" is
an automated test too (`crates/*/tests/m*_done_when.rs`). This file is the
by-hand half.

## 1. On one machine — M2 and M3

```bash
eval "$(mise env -s bash)"
cargo build -p hit-cli
scripts/m2-by-hand.sh       # ~30 s
scripts/m3-by-hand.sh       # ~70 s
scripts/m5-by-hand.sh       # ~40 s
scripts/m6-by-hand.sh       # ~45 s (also builds on hit-anchor)
scripts/m8-by-hand.sh       # ~45 s
```

Both use relays off and fixed ports. Identities and group keys go into the
keychain under `hit-enter-m2-test` / `hit-enter-m3-test` and are cleared on
exit. Commands that only sign (`group create`, `invite`, `say`, `kick`,
`promote`, `compact`) write to the store and run beside `run`; `run` carries
what they signed on its next sync — **by sync, not gossip**, so here a
message takes up to one sync period (2 s). Live gossip is covered by the
automated test.

**M2** (three processes, sharing one group since M3): A and B sync every
2 s; A says 5; **A is `kill -9`ed**; B says 20; A restarts and must end with
exactly 25 distinct messages, the log showing one sync with
`fetched 20 (dup 0)`. Then C presents A's binding, copied — refused, *"names
a different device"* — and its own — served nothing: `fetched 0, pushed 0`.

**M3** (five processes): A creates *Friends*; B and C join from A's link;
A says hello (B and C: 1 message each); **A is `kill -9`ed**; B and C each
say something (3 each); D joins from **B's** link, with A gone, and speaks
(C: 4, and C lists D); A restarts (4 messages, and lists D); A compacts to
seq 1 (B: seq 1, still 4 messages and 4 members); E, in no group, asks B
for the group's events — **0 served** — for everything — **0 served** —
and syncs — **fetched 0**.

**M5** (`scripts/m5-by-hand.sh`, ports 7401–7404, four processes): A owns
the group, B is promoted to moderator, C and M are members; M says
something; B bans M; M's `run` is stopped and her **patched client** —
`hit-cli lie <device> <group> <text> [--ago N]`, which signs a message and
hands it straight to a device without judging it — sends a message dated
now and one backdated a minute to each of A, B and C. Expect six times
`refused — blocked: not a member`, M's old invite to fail, 0 lies on every
honest peer, and M's message from before the ban still there.

**M6** (`scripts/m6-by-hand.sh`, ports 7601–7603 and 7690): A (`hit-cli`)
owns a group; N (`hit-anchor`) pins it; A's next invite must name N's
device first; A is `kill -9`ed; D, knowing only where N is, joins from A's
link at "4am", has the history, says something; A comes back and has D's
message and three members (A, N, D); N answers NIP-11 on
`http://127.0.0.1:7690`. **The invite must reach N before A goes** — the
script waits six seconds after minting it: an invite made the moment before
its maker goes offline does not work at 4am, and the first version of the
script failed one run in three on exactly that (the join error now names
every peer's answer, which is how it was found).

**M8** (`scripts/m8-by-hand.sh`, ports 7801–7803): A, B, C in a group; A
posts 400 KB of random bytes with `hit-cli post-file` and serves them for 20
s; B fetches them (`get-file`) and then `run`s, its cache serving; A goes;
C fetches the same hash — from B — and both copies `cmp` equal to the
original.

Results on 2026-10-11 (the desktop): every expectation above, all five
scripts; M6's eight times in a row, M8's twice. The joins took one negentropy round, 6–10 ms. The M5 script's
first run caught a real bug: B — the moderator who banned M — took two of
her lies, because `ban` was signed into B's store by a separate CLI process
and B's running node never re-read the group's state. A running node now
refolds a group whose events in the store changed.

## 2. Between two machines — **not done yet**

The real "done when" of M2 and M3 is between your machine and a friend's.
With n0's relays (the default), a peer is just its device id, and an
invite link needs no `--addr`:

```bash
# machine 1 (the owner)
hit-cli --data ~/hit init
hit-cli --data ~/hit group create Friends
hit-cli --data ~/hit invite Friends --hours 24     # send the link to machine 2
hit-cli --data ~/hit run                           # leave running

# machine 2
hit-cli --data ~/hit init
hit-cli --data ~/hit join '<the link>'
hit-cli --data ~/hit run                           # leave running

# either, from another terminal
hit-cli --data ~/hit say Friends "hello from <city>"
hit-cli --data ~/hit log Friends
```

Then M2's half: stop one side (Ctrl-C, or pull the network), say twenty on
the other, bring the first back — same messages on both, no duplicates, a
`fetched 20 (dup 0)` line. M3's half needs a third install (a laptop, a
VM): the owner quits, the other two keep talking, a fourth joins from a
non-owner's invite, the owner returns. On a real network also check the
path: S5 says a relay-only endpoint leaks no IP, but nobody has looked at
what the default endpoint does across NATs.

To test a copied binding across machines: `hit-cli binding > a.json` on
one, copy it to a third install, `hit-cli sync --present-binding a.json`.
To see what a stranger is served: `hit-cli req <device id> --group Friends`
from an install in no group with it.

## 3. The app — two windows on one machine (M4)

**From the agent, use `scripts/gui/`** (its README): the recipe below,
plus onboarding, focus-guarded typing, screenshots and null sinks, as
scripts. The manual recipe stays for a person at the keyboard.

HIT_ENTER_2's recipe, re-plumbed. Each window is a device: its own data
directory, its own port. Identities go under a test keychain service.

```bash
eval "$(mise env -s bash)"
(cd web && npm install && npm run build)
cargo build -p hit-app --features custom-protocol   # serves web/dist
export HIT_KEYCHAIN_SERVICE=hit-enter-m4-test HIT_RELAYS=none
HIT_DATA_DIR=/tmp/hit-a HIT_PORT=7301 ./target/debug/hit-enter &
HIT_DATA_DIR=/tmp/hit-b HIT_PORT=7302 ./target/debug/hit-enter &
# afterwards
secret-tool clear service hit-enter-m4-test
```

Without `--features custom-protocol`, a debug build loads Vite's dev server
(`cd web && npm run dev`, port 1420) instead — one dev server serves any
number of windows. Running the binary directly bypasses
`.cargo/config.toml`; the binary sets `WEBKIT_DISABLE_DMABUF_RENDERER`
itself.

**With relays off, two windows find each other again after a restart only
if told where**: each logs `this device: <id>` at start; restart them with
`HIT_PEERS=<the other's id>@127.0.0.1:<its port>`. With relays on (the
default), discovery does this and `HIT_PEERS` is not needed.

The run, all from the keyboard:

1. A: *make a new identity* → *show the key once* → type its last 8
   characters → a name → *make or join a group* → a group name → say
   something.
2. A: *invite* → *make link* (it is copied).
3. B: a new identity → backed up → a name → *make or join a group* → *join
   with a link* → paste → B has A's message, A's name, "1 of 1 reachable".
4. B says something: it appears in A **live**, under B's name, and A's
   member list shows B's name.
5. **The network off:** `kill -9` A, close B. Start A alone: the history is
   there, from disk, and the header says *nobody reachable*. Say something
   — it shows at once. Start B (with `HIT_PEERS` as above): within one sync
   B has A's message, with nobody pressing anything — `fetched N` in B's
   log.

Results on 2026-10-11 (the desktop, driven with `wtype`, every keystroke
behind a check that the HIT_ENTER window had focus): steps 1–5 as above.
Driving it found three defects, all fixed: the key-confirmation field was
not focused once the key was revealed (WebKitGTK sends a page with nothing
focused no keys); A's member list kept showing B's key after B's name
arrived; and two apps with relays off could not find each other after a
restart (`HIT_PEERS`). **Not driven:** the *encrypted file* backup through
the native save dialog — the GTK dialog took the typed path and then
ignored Enter, Tab and Escape from `wtype` (HIT_ENTER_2 hit the same wall
and patched the path in by hand). Its Rust side is unit-tested; the dialog
needs a person with a mouse. Nor *I already have a key* (import), also
unit-tested only.

**M4b, the same two windows** (2026-10-11), both onboarded through *I
already have a key* with throwaway `nsec`s: a group, an invite, a join;
Alice reacts to Bob's message from the keyboard — Shift+Tab from the
composer to the newest message, Tab into its bar, Enter on 👍 — and Bob
sees *👍 1* live; Enter again takes it back, on both; Bob withdraws his own
message (*delete*, the bar's last button) and it leaves both windows; both
show each other *online*, in the member list's *online* section. Driving it
found four more focus defects, all fixed: closing a dialog left nothing
focused; Escape did not close a dialog while its `<select>` had focus
(WebKitGTK swallows it — the dialog now listens in the capture phase);
withdrawing a message removed the focused button with it; and every
message in the Tab order put the whole history between the composer and
the rest of the window (now a roving tabindex, ↑/↓ between messages). Note
for whoever drives it next: **Hyprland's focus follows the mouse** — park
the pointer off the windows, and refocus with
`hyprctl dispatch 'hl.dsp.focus({ window = "address:0x…" })'`.

Unread counts surviving a restart are covered by `hit-app`'s tests, not by
the window: with one group and one channel, the open channel never shows a
badge.

**M5, the same two windows** (2026-10-11): Alice (owner) opens Bob's
member dialog from the keyboard (Tab from the composer into the member
list), mutes him for 10 minutes — his composer says *muted for another 10
min* and his name is marked *muted* — lifts it, then bans him; the
confirmation lists what a ban does and cannot do before the button. Bob's
window shows *You were banned from this group*, his history still
readable. Group settings: a channel added, Bob in the ban list. Driving it
found: the composer was `disabled` while muted, which drops focus for good
(now `readonly`); the ban confirmation lost focus when it replaced its
button; and **Bob never learned he was banned** — his gossip was cut by the
ban itself and sync served him nothing. A removed person may now read the
action that removed them.

## 4. The app — the real "done when" (M4) — **not done yet**

HIT_ENTER_2 M4's recipe with a real person in another city: both install
the app (relays on, no environment variables), one makes a group and sends
the invite link by some other channel, the other joins, and they talk.
Then **the network off** on one side: quit, unplug or disable Wi-Fi, reopen
— history from disk — write a message, plug back in: it arrives on the
other side by itself. That half is where M4's only two real bugs lived
last time.

## 4b. DMs in the app (M7)

Two windows (§3), in a group together: Bob opens Alice in the member list
→ *message* → writes; Alice's DM button shows *1*; it survives a restart;
the DM button opens the latest conversation with the composer focused;
Alice replies and Bob's badge shows it. Driven on 2026-10-11. The
offline-through-an-anchor half and the database scan are automated
(`m7_done_when.rs`), not driven: the window has no anchor of its own.

## 4c. Files in the app (M8)

Driven on 2026-10-11: a picture posted from `hit-cli post-file` into a group
shown **inline** in Bob's window, three times over, fetched and checked by
hash. Two defects found and fixed on the way: the window's Content Security
Policy (`default-src 'self'`) blocked the `blob:` URL a picture is shown
from — `img-src 'self' blob:` now; and the bytes needed `new Uint8Array(…)`
before becoming a Blob. **Not driven:** attaching a file *in the window* —
the GTK open dialog took the typed path and ignored Enter and Escape, as
the save dialog did in M4. Attach, send, show and save are covered by
`hit-app`'s tests; the dialog needs a person with a mouse.

**M8b** (2026-10-11): a picture sent as a DM file from `hit-cli dm-file`
shown inline in Bob's window — decrypted on the Rust side, the window given
no key — with the conversation list saying *📎 gradient.png*. Sending one
from the window goes through the same GTK open dialog, not driven.

## 4d. Voice in the app (M9) — on null sinks

(`scripts/gui/sinks.sh up` and `launch.sh` do the setup below.)

**Never the real microphone or speakers** (CLAUDE.md). Each window's audio
goes to its own PipeWire null sinks: `HIT_AUDIO_IN` / `HIT_AUDIO_OUT` beat
the app's settings, and the ALSA `pulse` device follows `PULSE_SOURCE` /
`PULSE_SINK`.

```bash
for n in a_in a_out b_in b_out; do pactl load-module module-null-sink sink_name=hit_$n; done
export HIT_KEYCHAIN_SERVICE=hit-enter-m9-test HIT_RELAYS=none HIT_AUDIO_IN=pulse HIT_AUDIO_OUT=pulse
PULSE_SOURCE=hit_a_in.monitor PULSE_SINK=hit_a_out HIT_DATA_DIR=/tmp/hit-a HIT_PORT=7901 ./target/debug/hit-enter &
PULSE_SOURCE=hit_b_in.monitor PULSE_SINK=hit_b_out HIT_DATA_DIR=/tmp/hit-b HIT_PORT=7902 \
  HIT_PEERS=<A's device>@127.0.0.1:7901 ./target/debug/hit-enter &
pw-play --target hit_a_in tone.wav                       # Alice's "voice"
parecord -d hit_b_out.monitor --channels=1 --file-format=wav bob-hears.wav
# afterwards: unload the four modules; secret-tool clear service hit-enter-m9-test
```

Driven on 2026-10-11, the tone in 400 ms bursts (a steady tone is what the
noise suppressor exists to remove — see below). Alice made *Voice Test*,
added **🔊 lounge** in the settings (the new *voice* box), and invited Bob.
Bob saw *Alice* listed under the lounge **before joining**. Both in: *2/8*,
*1 connected* on both sides, Alice's dot lit as speaking in both windows,
and **Bob's speakers carried 440 Hz** (Goertzel on the recording). Alice
**muted**: Bob's speakers silent (peak 0.0000) and both dots dark.
**Push-to-talk**: silent with the key up; holding `` ` `` gave *talking…*
and the tone at Bob's end; silent again on release. Bob **left**: Alice's
lounge back to *1/8*, Bob's call bar gone. One defect on the way:
`KeyboardEvent.code` was not `Backquote` under `wtype`'s keymap, so the
talk key now matches the backquote character too.

**1:1 calls** (the same day, fresh identities, the same sinks): Alice
opened Bob's member dialog → *message* → **📞 call**: her header said
*ringing…* with a *hang up* button, and Bob's window showed the
**incoming call** banner — without taking his composer's focus, so a
half-typed message cannot answer it. **Declined**: Alice's window said
*call ended — declined*, Bob's focus back in his composer. **Answered**:
both call bars said *📞 …*, Bob's speakers carried Alice's 440 Hz (peak
0.080), and Alice's *hang up* in the header ended it on both sides — Bob's
window said *call ended — they hung up*. Two defects on the way, both
focus: the *call* button and the banner each disappeared with the focus
inside them, which in WebKitGTK leaves the window deaf to keys; focus now
goes back to the composer, or to whatever had it before the banner.

**Private channels** (the same day, one window): the owner made 🔊
*staff*, ticked *private* in group settings — *staff: private*, and a 🔒
beside it in the channel list — then toggled it three times, a second
apart: it ended *open to every member*, lock gone. The first attempt
stopped after one toggle: disabling the box while it saved took the focus
off it, leaving the window deaf to keys; every action in the dialog now
gives focus back. Two people in a private room, and a member kept out of
it, are `m9_private_voice.rs` — not driven in windows.

**No group in common** (fresh identities, no group at all): Alice's
profile dialog showed her **contact link** (copied with the button); Bob,
on the no-groups screen, chose **write to someone**, pasted it, and his
conversation with her opened (named by npub — profiles stay within
groups). His message reached her; both apps were restarted and her reply
reached him — each had kept the other's device. Bob called from the
conversation, Alice's app rang, she answered, and her speakers carried
Bob's 440 Hz. Two defects on the way: with no groups the app showed only
the welcome screen — no way into DMs at all — and, once a DM arrived, an
empty "joining…" group pane; with no groups, the DM view is now what shows.

Also, outside the app: `cargo run -p hit-voice --example devices` (with
`IN`/`OUT` set — **without them it only lists**) loops a tone through a
null sink: back at 0.2105 of 0.2121 with echo cancellation off, **0.0013
with it on** — the canceller removes what the speakers play. And
`--example call_devices` runs a call with real cpal devices on one side:
the other side hears the mic, and the speakers play the other side's tone
at full level.

**What this does not show.** Speech. With processing on, a steady tone
arrives at about a sixth of its level, because noise suppression takes it
for noise; that is the suppressor working, and it says nothing about
voices. Whether a real voice comes through clean, and whether a laptop on
speakers sends no echo, takes people — §6.

## 5. A NIP-29 client reading through an anchor (M6) — **not done yet**

The other half of M6's done-when, which needs a real NIP-29 client and a
real `wss://`: run an anchor as `docs/SELF_HOSTING.md` says (§4–§5), pinned
to a group you are in. In **Chachi** (chachi.chat) or **0xchat**, sign in
with the same key you use in HIT_ENTER, add the relay
`wss://<your anchor>`, and open the group (its id is the group key in hex).
Expect: the group's name and members, its messages, new ones arriving live;
posting is refused with a message about the channel tag. Someone signed in
with a key that is not a member should see nothing of the group.

What is automated instead (`crates/hit-anchor/tests/m6_done_when.rs`): a
WebSocket client speaking NIP-01 and NIP-42 as those clients do — NIP-11,
`auth-required` before AUTH, wrong relay and wrong challenge refused, the
member's REQs answered, translated `39000`/`39002` signed by the anchor and
none of the group key's own, a live message, a channel-less post refused,
and a stranger served nothing.

## 6. Voice for real — M9's done-when — **not done yet**

Four people in two cities in one voice channel for thirty minutes, **one of
them on laptop speakers**, and nobody hears an echo; a ninth told the
channel is full. Before that, the cheap version: two people, one on
speakers, ten minutes, each saying whether they hear themselves. Things to
note as you go, because M9 guessed at them:

- **Echo**: the person on speakers' voice should come back to nobody. If it
  does, try *audio → echo cancellation* off on a headset user's side only.
- **Drift and gaps**: each device's buffer is held at 40 ms by resampling
  within ±0.5 % (`hit-voice/src/drift.rs`). If the call bar says *audio
  glitches* and the number climbs, note the device (Bluetooth? a busy
  machine?) — that is drift or load the controller did not absorb. On null
  sinks it reads zero for a minute at a time (they share one clock, so this
  shows only that it does no harm; the ±0.3 % cases are simulated).
- **Bluetooth (HFP)** headsets: S3 found them robotic. Note which.
- **Who is listed**: someone who quits the app should vanish at once (the
  app says *out* on quit); one whose app crashes vanishes within 15 s.

## 7. Zaps (M10)

### 7a. Against the fake network — done (2026-10-11)

No sats, no internet: `hit-zap`'s fake network (`--features fake`) is a
Lightning address server for every account, NWC wallets and a relay, all
on loopback. The automated version is
`crates/hit-anchor/tests/m10_done_when.rs`; by hand, in two windows:

```bash
cargo build -p hit-anchor -p hit-app --features custom-protocol
cargo build -p hit-zap --features fake --example fake_network --example nwc_pay
./target/debug/examples/fake_network alice=10000 bob=0 hitenter=0 > target/gui/fake.log &
export HIT_ZAP_FEE_ADDRESS=hitenter@127.0.0.1:<port>   # debug builds only
# two windows (scripts/gui), a group, Bob joins; an anchor with its bridge:
AN="./target/debug/hit-anchor --data target/gui/anchor --keychain-service hit-enter-gui-test --relays none --port 47103"
$AN init; $AN peer <A's device> 127.0.0.1:47101; $AN pin '<invite>'
$AN run --ws 127.0.0.1:47190 --ws-url ws://127.0.0.1:47190 &
```

Bob: *you* → *zaps* → his fake address → save. Alice: *you* → paste her
NWC string from `fake.log` → connect. Bob posts; Alice focuses it, ⚡ zap.

Results on 2026-10-11, driven from the keyboard: the dialog read
*1,000 sats to Bob · 10 sats HIT_ENTER fee · 1,010 total* and named the
anchor's relay; *zap 1,010 sats* paid `alice → bob 1000 (zap)` then
`alice → hitenter 10, memo "HIT_ENTER fee"`; the anchor carried the
receipt in and **both windows showed ⚡ 1,000** within a second. By QR
code at 200 sats (paid with `nwc_pay`, as a phone would): the dialog
waited, said *Zapped 200 sats*, then showed the fee's own QR, 2 sats;
both windows showed ⚡ 1,200, and still did after a restart. At 50 sats the
quote showed no fee line. Driving it found two defects, fixed: the QR step
ended at once, because the anchor's relay serves nothing before NIP-42 so
the receipt watch closed — the app now also waits for the anchor's
carrier; and saving an address moved focus to the top of the profile, out
of sight of the result.

### 7b. For real — S4 and M10's done-when — **not done yet**

Needs: two wallets of your own — one that **sends over NWC** (Alby Hub,
Coinos…; give the connection a small budget) and one whose **Lightning
address supports NIP-57** (Coinos, Alby, Primal, Wallet of Satoshi…;
Phoenix's `₿` addresses are BIP-353, not LNURL, and produce no receipt) —
and an anchor reachable at a real `wss://` (§5, `SELF_HOSTING.md`).

1. **S4**: zap 21 sats from the sender to the receiver from the app; the
   receipt arrives on the anchor's `wss://`; ⚡ 21 shows. Note the
   provider, how long the receipt took, and its `description` hash.
2. **The done-when**: a 1,000-sat zap to a friend's file; their wallet gets
   1,000 and a fee address 10 with a memo naming nothing; the total shows
   in both apps and a third member's; a 50-sat zap charges nothing.
   (The fee needs `FEE_ADDRESS` set — or `HIT_ZAP_FEE_ADDRESS` in a debug
   build.)

**The owner's setup (2026-10-12, money in, nothing spent):** Coinos
sends (an NWC connection with a ~2,000-sat budget, made when needed,
pasted by the owner into *you → zaps*, never into a chat); **Wallet of
Satoshi** receives at the owner's address (kept out of the
repo) — its LNURL-pay answer has `allowsNostr: true`, minimum
1 sat. A different provider from the sender, so the payment really crosses
Lightning. Funded through Kraken (BTC withdrawn over Lightning to a Coinos
invoice — worked). Alby Hub dropped: too much to run, and users won't.

**S4 — done, 2026-10-17, five zaps of 21 sats** (Coinos over NWC, two
windows on the laptop, the anchor at `wss://anchor.hitenter.app` — a
named Cloudflare tunnel, free — after a quick tunnel):

- **Wallet of Satoshi, three zaps: paid, never counted.** Its invoices
  commit to the request, but it published **no receipt anywhere** — not to
  our anchor (quick tunnel or real domain; it never connected), not to the
  relays it publishes other people's receipts to (nos.lol, Primal,
  nostr.mom, damus, purplepag.es). Cause not known; guess: the zapped
  message and the sender exist nowhere public. Do not recommend WoS for
  receiving zaps.
- **Primal, first try: refused before paying.** Its invoices carry one fixed
  description hash whatever the request, and our strict NIP-57 check
  refused them — and the dialog hid the reason. Both fixed (`56c36e2`,
  the owner's decision: exact amount required, hash not).
- **Primal, second try: counted.** Paid 18:21:08 UTC; Primal's receipt
  reached the anchor at 18:21:18 (~10 s), was carried in, and synced to
  both windows and the anchor.
- **The fee, real (18:40 UTC):** a 200-sat zap to Primal from the
  release-path `FEE_ADDRESS = fee@hitenter.app` (a static LNURL-pay file
  on hitenter.app forwarding to the owner's Wallet of Satoshi): Primal got
  200, WoS got **2 sats with the memo "HIT_ENTER fee"**, the receipt was
  carried in under a second. M10's done-when at 1,000 sats waits for a
  Coinos top-up and a friend's app.
- **Still owed:** the app's own receipt watch gives up after ~4 s (the
  anchor serves nothing before NIP-42 and closes the subscription), so the
  dialog says "paid, the total appears when it arrives" even when it then
  does — the QR path waits for the carrier; the wallet path should too.

**`wss://` without a VPS — a Cloudflare quick tunnel** (`cloudflared`,
installed by the owner):

```bash
hit-anchor … run --ws 127.0.0.1:7777 --ws-url wss://<name>.trycloudflare.com
cloudflared tunnel --url http://127.0.0.1:7777   # prints the name
```
Start the tunnel first, read its address, then start the anchor with it.
The address changes on every restart, so do S4 in one sitting; the laptop
stays on throughout. Cloudflare sees receipts and the read-only bridge —
not the chat, which stays on iroh. For tests only.
