# Spike S3 — voice between two cities

**The question** (`docs/PLAN.md` M0, S3): can two people in different cities
hold a good voice call over iroh, from Rust — before M9 builds voice
channels on top of it?

**The tool:** n0's [`callme`](https://github.com/n0-computer/callme) — Opus
audio over `iroh-roq` (RTP over QUIC), `cpal` for devices, WebRTC audio
processing for echo cancellation. MIT OR Apache-2.0, so we may learn from it
and borrow from it in closed software, keeping its notice.

It is **not** vendored into this repo. The build lives outside it, in
`~/Work/tries/callme`, and nothing from it is committed here.

---

## Part 1 — local, one machine (done 2026-09-27)

Built `callme-cli` in release mode with echo cancellation on: **1m 15s, no
errors**, on Rust 1.98.1 (installed via `mise use -g rust@stable` — this
machine had no Rust at all). Every C dependency was already present
(ALSA, PipeWire 1.6.8, Opus 1.6.1, automake/libtool for
`webrtc-audio-processing`).

Ran two copies on one machine, both routed through a temporary PipeWire null
sink so that **no real microphone was recorded and nothing played through
the speakers** (`PULSE_SINK` / `PULSE_SOURCE`, device `pulse`). The sink was
removed afterwards.

| what | result |
|---|---|
| dial by node id alone | ✅ connected in **0.44 s** |
| audio negotiation | ✅ Opus stereo track in **both** directions |
| echo cancellation | ✅ enabled (`init audio processor (enabled=true)`) |
| capture / playback | ✅ 48 kHz, f32, through PipeWire's `pulse` device |
| real-time audio threads | ⚠️ **refused** — first because `rtkit` was missing, then because of a zero `RLIMIT_RTTIME` (below) |

### What part 1 proves and what it does not

- **Proves:** it builds here, audio I/O works through PipeWire from Rust,
  and a 2025 iroh (0.33) still finds a peer by node id through n0's
  discovery today.
- **Does not prove anything about distance.** Two processes on one machine
  connect directly, with no router, no NAT and no relay between them. The
  real question is part 2.
- **The `rtkit` warning matters for the product.** Without real-time
  priority, audio threads compete with everything else on the machine, and
  a busy CPU turns into crackles. For the app, M9 must check for it and say
  so, rather than stutter silently.

### Real-time priority, chased down (2026-09-27)

Installing `rtkit` was necessary and not sufficient. It took three
separate things, and only two of them are fixed:

1. **`rtkit` installed** (`sudo pacman -S rtkit`) — the D-Bus service now
   answers (`RTTimeUSecMax` 200000, `MaxRealtimePriority` 20). The error
   changed from `ServiceUnknown` to `setrlimit: Operation not permitted`.
2. **PipeWire had to be restarted** — it was still the process from boot,
   started before `rtkit` existed, and had logged `RTKit error:
   ServiceUnknown`. A logout had not restarted it.
   `systemctl --user restart pipewire pipewire-pulse wireplumber` fixed it:
   its `data-loop.0` thread now runs `SCHED_RR` at priority 20.
3. **Not fixed: processes started from a terminal cannot get real-time
   threads at all.** The `foot` terminal — and the tmux and everything
   launched inside it — inherits `RLIMIT_RTTIME` = **0 soft, 0 hard**, while
   Hyprland itself has it unlimited. `audio_thread_priority` must set that
   limit to rtkit's 200000 µs before asking, and an unprivileged process
   can lower a hard limit but never raise one — so it fails before rtkit is
   even asked. The source of the zero was not found: systemd's user default
   is `infinity`, and the terminal's own scope does not set it.

**What it means.** Real-time priority only protects audio **under load**;
the call works without it, as both local runs showed. Part 2's stress check
says whether it matters in practice. For M9: the app is launched from the
desktop rather than a terminal, which may inherit a different limit —
**check that when the Tauri app exists**, rather than assuming either way.
And the app must treat real-time priority as best-effort: fall back
cleanly, and if audio breaks up under load, say why.
- **`callme` is on iroh 0.33; HIT_ENTER will be on iroh 1.1.** The audio
  pipeline is what we are testing, and it does not care. Porting it to 1.1
  is M9 work, and is itself a small risk — `iroh-roq` has to exist for 1.1,
  or its job (RTP over QUIC datagrams) has to be redone on raw datagrams.

---

## Part 2 — the real test, with your friend (to do)

### Both of you build it

**Linux (you — already built):**

```bash
cd ~/Work/tries/callme
./target/release/callme-cli list-devices
```

**Your friend, Linux:** install Rust (`rustup` or `mise use -g rust@stable`),
ALSA and D-Bus headers, and autotools, then:

```bash
git clone https://github.com/n0-computer/callme.git && cd callme
cargo build --release -p callme-cli
```

**Your friend, macOS** (built 2026-10-04 on an Apple-silicon MacBook Air,
Rust 1.94.1 — 47 s once the two traps below were out of the way):

```bash
xcode-select --install                    # if the Command Line Tools are missing
brew install autoconf automake libtool    # webrtc-audio-processing needs them
git clone https://github.com/n0-computer/callme.git && cd callme
cargo build --release -p callme-cli
```

- Without Homebrew's `libtool` the build dies on `glibtoolize: NotFound` —
  on macOS the build script calls GNU libtoolize by its `g` name.
- **After any failed build of `webrtc-audio-processing-sys`, clean it before
  retrying**, or the retry dies on `Permission denied (os error 13)`: the
  first attempt copied the C source, read-only `.git` pack files included,
  into `target/`, and the copy cannot overwrite them.
  `cargo clean --release -p webrtc-audio-processing-sys` fixes it.
- The first `list-devices` may hang silently: macOS is waiting for
  microphone permission. Run it in Terminal yourself and click **Allow**, or
  turn the terminal on in System Settings → Privacy & Security → Microphone
  and restart it.

**Your friend, Windows:** rustup and VS Build Tools as in
`../HIT_ENTER_2/CLAUDE.md` "Windows build gotchas", then the same build.
If `webrtc-audio-processing` fails to build (its C++ needs autotools, which
Windows does not have), build without it — **and then they must use
headphones**, because there is no echo cancellation:

```bash
cargo build --release -p callme-cli --no-default-features
```

### Make the call, and save the logs

One of you accepts, and sends the printed node id to the other (any chat app):

```bash
RUST_LOG=info,iroh=info ./target/release/callme-cli accept 2>&1 | tee accept.log
```

The other connects:

```bash
RUST_LOG=info,iroh=info ./target/release/callme-cli connect <NODE_ID> 2>&1 | tee connect.log
```

Keep both `.log` files — they say whether the connection went **direct** or
through a **relay**, which is the difference between "your routers let iroh
through" and "iroh fell back", and it changes how the latency should be read.

### What to check, in this order

1. **Headphones, 5 minutes of normal talk.** Does it *feel* like a phone
   call, or do you keep talking over each other? Anything above ~250 ms
   one-way starts to feel like that.
2. **The "now" test.** One says "now", the other says "now" back the
   instant they hear it. Try it five times. A snappy reply means low
   latency; a noticeable pause means a relay or a buffer.
3. **Quality.** Clear, or robotic, crackling, cutting out? Note when.
4. **Laptop speakers, one of you, 2 minutes.** The one on speakers is
   fine; **the other listens for their own voice coming back.** That echo is
   exactly what echo cancellation exists to remove. If the friend built with
   `--no-default-features`, skip this and write down that it was skipped.
5. **Stress.** While talking, one of you starts a big download or a build.
   Does the audio break up? (This is the `rtkit` question, in practice.)

### Then

Paste both log files and your notes into the session, and the verdict below
gets written from them.

---

## Part 2, first attempt — macOS ↔ Linux, same LAN (2026-10-04)

The Mac dialled (`connect`), the Linux machine (Omarchy) accepted. Only the
Mac's `connect.log` has been read so far; **the Linux side's `accept.log`
is still needed.** Heard on the call: **robotic sound, with gaps.**

**This was not the two-city test.** Both machines were on the same home
network (`192.168.1.135` ↔ `192.168.1.132`), so it says nothing about
distance — but it does say that the bad sound is *not* the network.

| what | result |
|---|---|
| connection | ✅ **direct** UDP in ~0.35 s (relay `euw1-1` only for the first packets), stayed direct |
| Mac mic | ⚠️ Bluetooth headphones, opened at **24 kHz mono** — macOS's hands-free (HFP) profile |
| Mac outgoing audio | ❌ **698 capture xruns** (`record xrun: failed to push out 960 of 960`), each 20 ms of speech lost — **541 in one minute (~18 %)**, then none after ~2 min |
| Mac incoming audio | ❌ **307 playback xruns, ~1 per second, steady**, `buffered 0` every time — 180–1500 of each period's samples missing |
| real-time threads (macOS) | ⚠️ refused: `thread_policy_set: time_constraint`, both capture and playback |

Per minute, from the log:

| minute | capture xruns | playback xruns |
|---|---|---|
| 13:05 | 122 | 13 |
| 13:06 | 541 | 57 |
| 13:07 | 35 | 59 |
| 13:08–13:10 | 0 | ~59 each |

**Reading it.**

1. **The Bluetooth headset mic is the first suspect for "robotic".** Using
   a Bluetooth headset's microphone forces it into HFP, which is narrowband
   and sounds robotic on its own, before any software touches it.
2. **The capture xruns are the Mac's own voice being dropped** before it
   was encoded — the other side heard up to a fifth of it missing. They
   stopped by themselves after two minutes, which smells like a start-up
   or device-switch effect rather than steady overload. Not explained yet.
3. **The playback xruns are a rate mismatch, not packet loss.** Once a
   second, regular as a clock, with an empty jitter buffer: the incoming
   stream delivers slightly less audio than the Mac's output consumes. On
   a direct LAN link this is clock drift between the two machines' devices
   (or the sender falling behind) — and callme does not appear to resample
   to absorb it. **This one matters for M9**: real calls between real
   devices will always drift, so the voice pipeline needs drift
   compensation, not just a jitter buffer. The Linux log can confirm
   whether the sender was falling behind instead.

**The Linux side (`accept.log`, read 2026-10-04).** Clean:

- over ~9 minutes: **2 playback xruns, 0 capture xruns** — Linux captured
  and sent without a gap, and the Mac's voice reached it steadily;
- direct UDP path at ~15 ms, stayed direct; the call ended normally when the
  Mac hung up (`closed by peer: 0`).

So Linux was **not** a sender falling behind, and the Mac's
once-a-second playback underrun is on the Mac's output side — most likely
the Bluetooth headphones' clock, which callme does not compensate for. The
open question is whether it also happens on built-in audio.

**Correcting the "18 %".** 541 capture xruns × 20 ms ≈ 10.8 s of 60 s is
18 % *of that one minute*. Over the whole call it was 698 × 20 ms ≈ 14 s of
~7 min, **about 3 %**, nearly all in a start-up burst; zero after minute 3.

**Next attempt.**

- Mac on its **built-in microphone**, headphones for output only (they stay
  in high-quality A2DP): `callme-cli -i "MacBook Air Microphone" -o
  "<headphones>" connect <NODE_ID>`.
- Bring **both** logs.
- Also once with the Mac's **built-in speakers**, no Bluetooth at all:
  if the once-a-second underrun disappears it was the headphones' clock;
  if it stays, callme has no drift compensation and M9 needs it.
- If still robotic, once more with `--disable-processing` (headphones
  only) to rule echo cancellation in or out.
- Then the real two-city run, from different networks.

---

## Part 2, second attempt — Linux ↔ Linux, laptop ↔ gaming PC (2026-10-10)

Both machines are the owner's, both Omarchy, each with a gaming headset (laptop:
a USB headset; PC: Redragon Zeus Lite on the motherboard's two 3.5 mm jacks) and its own Claude session. This
removes the Mac's two suspects at once — **no Bluetooth/HFP and no macOS
audio stack** — so if it is still robotic, callme's pipeline is the cause.

Whether the two machines share a LAN decides what the result means: **same
network = "does callme sound right"; different networks (e.g. one on a phone
hotspot) = the real distance test.** Write down which it was.

`call.sh` in this directory does the work; callme is still built outside the
repo (`~/Work/tries/callme`), logs go to `~/Work/tries/callme-logs/`.

**Both machines, once:**

```bash
git pull                              # in HIT_ENTER
spikes/voice/call.sh setup            # Rust via mise, clone + build callme (~1.5 min)
spikes/voice/call.sh devices          # the headset should be the starred (*) sink and source
```

If the headset is not the default: `wpctl set-default <id>` for both the sink
and the source (ids from `wpctl status`), or `pavucontrol`.

**The call** — laptop accepts, PC connects (the node id changes on every run):

```bash
# laptop
spikes/voice/call.sh accept            # prints "our node id:" — send it to the PC
# PC
spikes/voice/call.sh connect <NODE_ID>
```

Each run writes `accept-<host>-<time>.log` / `connect-<host>-<time>.log`.
Then the checks above, **headphones first** ("what to check", 1–3 and 5).
Skip 4 (speakers) unless one of you takes the headset off; if both stay on
headsets, the echo-cancellation question stays open.

Ladder if it is not good, one change at a time:

1. `call.sh accept --disable-processing` (and the same on `connect`) —
   rules echo cancellation in or out.
2. Pin a specific device: `CALLME_IN="USB Audio Device" CALLME_OUT="USB Audio
   Device" call.sh …` (names from `devices`).
3. Different networks, to separate "callme sounds bad" from "the path is bad".

**What to bring back:** both logs, whether the connection went direct or
relayed (`grep -iE 'direct|relay' *.log`), the xrun counts
(`grep -c xrun`), and the answers to the five checks.

**Result (2026-10-10): clear, no robotic sound, low latency** (owner's
ear; headsets both ends, same LAN, ~2 minutes).

Laptop `accept.log`, read the same day:

| what | result |
|---|---|
| connection | ✅ **direct** in ~0.1 s (relay `euw1-1` for the first packets only), LAN UDP then IPv6 |
| audio negotiation | ✅ Opus stereo both ways, echo cancellation on |
| capture xruns | ✅ **0** |
| playback xruns | ✅ **7 in ~123 s** (~1 per 17 s), versus the Mac's ~1 per second |
| real-time threads | ⚠️ refused (`rtkit` not installed on the laptop) — no visible effect |
| end | ✅ clean, `closed by peer: 0` |

The PC's `connect.log` was not read. Not tested: speakers/echo
cancellation, a busy machine, different networks.

### Speaker test — laptop on speakers, PC on its headset (2026-10-10)

Checks the echo cancellation: the laptop plays the PC's voice out loud and
listens with its **own** microphone, so there is a real echo path. **The PC
stays on its headset and listens for its own voice coming back.**

Laptop (`accept`), internal speakers and internal mic via PipeWire names —
`devices` / `pactl list short sinks` shows yours:

```bash
PULSE_SINK=alsa_output.pci-0000_05_00.6.analog-stereo \
PULSE_SOURCE=alsa_input.pci-0000_05_00.6.analog-stereo \
spikes/voice/call.sh accept
```

PC (`connect`) — nothing changes, headset as before:

```bash
spikes/voice/call.sh connect <NODE_ID>
```

Do it in this order, ~1 minute each, and note what the **PC** hears:

1. Normal talk both ways, speakers at a normal volume — own voice coming back?
2. Laptop volume loud — does it come back, and does it fade after a second?
3. Optional: repeat with `--disable-processing` on the laptop
   (`call.sh accept --disable-processing`) — the echo should now be obvious,
   which proves the cancellation was doing the work in 1 and 2.

Log: `logs/2026-10-10-laptop-accept-speakers-internal-mic.log`.

**Result (2026-10-10): works, but two real problems — and the echo test was
not clean.**

~7 min call, direct (IPv6), laptop on internal speakers + internal mic, PC on
its headset (on the motherboard jacks, not USB). Laptop log, from the
moment the call connected:

| what | headset call (USB) | speakers + internal mic |
|---|---|---|
| capture xruns | 0 in ~2 min | **138 in ~7 min — a steady ~20 / min**, in pairs ~80 ms apart |
| playback xruns | 7 in ~2 min (~3.5 / min) | **~20 / min**, steady, `buffered 0` |
| heard | clear | **PC's voice clear on the laptop speakers; the laptop's mic sounded robotic, with gaps** |

- **The robotic voice came from the laptop mic path**, not from echo: the
  owner sat between both machines, so the laptop's mic also picked up their
  own voice, and that arrived robotic. The PC's headset mic, played on the
  laptop speakers, was clear.
- **Gaps match the xruns**: ~20 playback underruns a minute is ~6× the
  headset call on the same machine, and the capture side had none before.
- **Echo cancellation: not proven either way.** One person at both ends in
  one room means "own voice coming back" cannot be told from "own voice
  picked up by the other mic". No one reported a classic echo, but that is
  not a pass.
- **Not separated yet:** whether the robotic sound is the laptop's cheap
  internal mic (`alsa_input.pci-0000_05_00.6.analog-stereo`, s32le 2ch), the
  echo canceller chewing a speaker-to-mic path (WebRTC AEC is known to do
  this), or CPU. The `--disable-processing` round (step 3) was **not run** —
  it is the next discriminator.

### Speaker test, round 2 — `--disable-processing` (2026-10-10)

Same setup as round 1, laptop on internal speakers + internal mic, echo
cancellation **off** (`enabled=false`). ~6 min, direct. Log:
`logs/2026-10-10-laptop-accept-speakers-no-processing.log`.

| | round 1, AEC on | round 2, AEC off |
|---|---|---|
| capture xruns | 138 in ~7 min (~19 / min) | **72 in ~6 min (~12 / min)** |
| playback xruns | ~147 in ~7 min (~20 / min) | **89 in ~6 min (~14 / min)** |
| heard (owner) | laptop voice robotic, gaps | **laptop voice still robotic, "a little better"; keyboard and mouse clicks very clear; fewer gaps on the laptop mic, but gaps still on the PC mic played on the laptop speakers** |

- **Echo cancellation costs something but is not the main cause.** Turning
  it off cut the xruns by roughly a third and made the sound a bit better,
  yet the voice stayed robotic and the underruns stayed at ~12–14 a minute.
- **The common factor is the laptop's internal analog audio path**
  (`alsa_*.pci-0000_05_00.6.analog-stereo`, s32le 2ch): the USB-headset call
  on the same machine, AEC on, had ~3.5 playback xruns a minute and no capture
  xruns. Playback xruns persisting with no processing at all say the
  underruns are not the canceller's.
- **"Robotic" with clear keyboard clicks** says the mic path delivers full
  bandwidth for transients but the voice is being mangled — consistent with
  dropped 20 ms Opus frames (the pairs of capture xruns) rather than a
  narrowband mic. That still points at scheduling/underruns, not the mic's
  frequency response.
- **Single run each, and the owner's impression of gaps is subjective;**
  minute-to-minute counts vary (6–19 capture xruns a minute). Treat the
  one-third drop as a direction, not a number.
- **Echo cancellation itself is still unproven**: same room, one person.

**Next, one change at a time:** install `rtkit` (real-time audio threads,
the only thing that protects audio under load — `sudo pacman -S rtkit`,
then restart PipeWire); rerun with the laptop on speakers; if the xruns
remain, try `CALLME_IN`/`CALLME_OUT` set to the named ALSA devices instead of
`pulse`, or a larger buffer — a callme change worth noting for M9.

### Speaker test, round 3 — `rtkit` installed (2026-10-10)

Same as round 1 (laptop on internal speakers + internal mic, echo
cancellation **on**), but with `rtkit` installed and PipeWire restarted, and
callme started from `systemd-run --user` instead of the terminal so it
inherits an unlimited `RLIMIT_RTTIME`. ~2.3 min of call (04:27:45 to the
log's last line at 04:30:01 UTC), direct. **It ended abnormally — see below.** Log:
`logs/2026-10-10-laptop-accept-speakers-rtkit.log`.

| | round 1 | round 2 (AEC off) | round 3 (rtkit, AEC on) |
|---|---|---|---|
| capture xruns / min | ~19 | ~12 | **~16** (36 in 2.3 min) |
| playback xruns / min | ~20 | ~14 | **~16** (36 in 2.3 min) |

- **callme still did not get real-time threads**, so this does not test
  real-time priority. The error changed from `ServiceUnknown` (no rtkit) to
  `setrlimit … Operation not permitted`, and `rtkit-daemon`'s journal shows
  **no request from callme at all**. In `audio_thread_priority` 0.33
  (`rt_linux.rs`) that `setrlimit` message is the cleanup path: after a failed
  promotion it tries to restore the old limit and cannot raise a hard limit
  it just lowered — so the real promotion error is hidden. **The cause is
  not found yet**; PipeWire itself is real-time (`SCHED_RR` 20) and rtkit
  grants it. For M9: log the *first* error, not the cleanup one.
- **The xruns are between round 1 and round 2 (~16 a minute) with
  cancellation on.** One short run, so it neither confirms nor rules out
  that rtkit or the PipeWire restart changed anything. Not a conclusion.
- **The call ended in a time-out, not a hang-up.** The PC's log
  (`connect-…-072745.log`, not committed) saw the direct path go
  `PongTimeout` at 04:30:10 and the connection `closed with error: timed
  out` at 04:30:31 — every other call in this spike ended `closed by peer:
  0`. **The laptop's log simply stops** at 04:30:01.7, mid-stream: no
  `shutting down`, no error, no panic, no signal line, and no underruns
  logged for the last 30 s although they had been arriving every 2–5 s. A
  network drop would have kept the log going (buffers running dry log
  underruns), so this points at the laptop's process being killed, the
  machine freezing or suspending, or the log being copied before it ended —
  **not found yet**. Candidate to rule out first: this was the only run
  started with `systemd-run --user`, and stopping that unit ends the
  process without callme's shutdown line. Open: did the owner stop it
  themselves at about 04:30 UTC?
- **Not a pipeline finding either way:** the PC side had 0 capture xruns
  and 4 or fewer playback xruns on every call, and nothing in its log before
  04:30:09 was unusual.
- **Listening notes for this round: not yet recorded.**

---

## Verdict

**S3 parked, 2026-10-10 — the pipeline is good enough to build M9 on.
The cross-network call was not done and is deferred, not passed.**

- **Direct or relayed / latency:** direct on the same LAN (IPv6 and IPv4),
  and it felt low latency. **Not measured across networks or NATs**: the
  owner's village connection is too slow for a meaningful call and the
  iPhone's Personal Hotspot would not turn on. Redo it from a decent
  connection — a different network is all it takes.
- **Quality:** clear Linux ↔ Linux with headsets (laptop USB, PC on analog
  jacks): 0 capture xruns, ~3.5 playback xruns a minute. The Mac's robotic
  sound and once-a-second underrun came from its Bluetooth (HFP) headset.
- **Echo cancellation on speakers: not proven.** Three rounds on the laptop's
  internal speakers + internal mic were robotic with gaps (12–20 xruns a
  minute); switching processing off helped by about a third but did not fix
  it, and the same machine was clean on its USB headset. One person in one
  room cannot tell echo from pick-up. **The test M9 owes**: two people, two
  rooms, one on speakers.
- **Busy machine: not tested.** callme never obtained real-time threads
  (see round 3), so "does real-time priority matter" is open.
- **Round 3's time-out is most likely ours**: that was the only run under
  `systemd-run --user`, and it was stopped with `systemctl --user stop`
  at about the time the log ends (04:30 UTC) — which kills callme without
  its shutdown line. Not confirmed; no evidence of a pipeline fault.
- **What changes in `docs/DESIGN.md` §10:** the S3 status line. Nothing
  structural.

### What M9 inherits from this spike

1. **Drift compensation** — callme does not absorb clock drift; real
   devices always drift (Mac run, once a second). *(M9: done —
   `hit-voice/src/drift.rs`.)*
2. **A real jitter/underrun story** — ~12–20 playback xruns a minute on a
   cheap internal analog path is audible; the app should surface it rather
   than stutter silently. *(M9: counted, shown in the call bar.)*
3. **Real-time threads are best-effort**, and the failing error is hidden:
   `audio_thread_priority` 0.33 reports the cleanup `setrlimit`, not the
   first failure. Log the first one.
4. **Device choice is a feature**: the `pulse` default, a named ALSA device,
   and a Bluetooth HFP mic gave three different qualities. Let people pick,
   and warn on HFP (24 kHz mono).
5. **Echo cancellation needs its own test** with two people before it is
   claimed to work.
6. **callme is on iroh 0.33; HIT_ENTER is on 1.1** — porting `iroh-roq` or
   redoing RTP on raw datagrams is M9's first risk.
