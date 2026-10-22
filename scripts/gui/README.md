# Driving app windows from the agent (Hyprland, Wayland)

How every "driven in two windows" result in `docs/TESTING.md` was got.
Throwaway identities under the keychain service `hit-enter-gui-test`,
data in `target/gui/<name>`, relays off, audio on null sinks — never the
owner's microphone or speakers.

```bash
scripts/gui/sinks.sh up                  # hit_a_in/out, hit_b_in/out null sinks
A=$(scripts/gui/launch.sh a 47101)       # prints the window's pid
scripts/gui/onboard.sh $A Alice          # import a fresh random nsec, set the name
B=$(HIT_PEERS=$(scripts/gui/device.sh a)@127.0.0.1:47101 scripts/gui/launch.sh b 47102)
scripts/gui/onboard.sh $B Bob
scripts/gui/drive.sh $A -k Tab -k Return # wtype, only if $A's window has focus
scripts/gui/shot.sh $A out.png           # screenshot one window (then Read it)
pw-play --target hit_a_in tone.wav       # Alice's "voice"
parecord -d hit_b_out.monitor --channels=1 --file-format=wav bob-hears.wav
scripts/gui/sinks.sh down; scripts/gui/stop.sh   # always, at the end
```

**The window (Slint, M12):** `scripts/gui/launch.sh a 47101`
starts `target/release/hit-enter` (build it first:
`cargo build --release -p hit-desktop`); `onboard.sh`, `drive.sh`,
`shot.sh` and `device.sh` work unchanged. In it: **Escape puts the
keyboard in the composer** — use it instead of counting Tabs, because the
lists take focus invisibly; `wtype`'s Shift does not reach it (Shift+Tab
moves forward), so drive with Tab only. An invite link lands on the
clipboard (`wl-paste -n` reads it) only while the window that copied it
runs. **`stop.sh` forgets the test identities** — to restart a window and
keep its key, `pkill -x hit-enter` instead. Tab stays inside a dialog
(after its last control comes the dialog itself, border lit, then its
first); a focused message takes `1`–`5` (reactions) and `Delete`;
typing a character anywhere in the main view lands in the composer.
Count Tabs from the composer: member rows come first. **Files without a file
dialog:** `NATIVE=debug` starts the debug build, which takes
`HIT_TEST_PICK=<path>[:<path>…]` for the attach buttons and
`HIT_TEST_SAVE_DIR=<dir>` for *save* (release builds ignore both) — the
portal's dialogs must never be driven or screenshotted. `wtype`'s Ctrl
does not reach Slint either (a focused message's Ctrl+S): Tab to *save*.
Below 880 px the member list is a drawer (the header's button; Escape
closes it), so the member rows are not in the Tab order then.

Rules (CLAUDE.md, "Driving the window"):

- **Check focus before every keystroke** — `drive.sh` refuses when the
  window is not focused, or `wtype` types into the owner's terminal.
  `focus.sh <pid>` brings it back (focus follows the mouse here).
- **Never screenshot a GTK file dialog** — it shows the owner's files.
  Those dialogs do not take `wtype`'s Enter or Escape anyway.
- **Never open audio without naming the device**: `launch.sh` sets
  `HIT_AUDIO_IN/OUT=pulse` and `PULSE_SOURCE/SINK` to that window's sinks.
  Bring the sinks up first, or the app's `pulse` device falls back to the
  real ones.
- Stop processes by exact name (`pgrep -x`), never `pkill -f`: a pattern
  matched the agent's own shell once.
- A tab order changes with every new control; screenshot before `Return`.
  The group's owner has a *settings* button the others lack, so the same
  keys land elsewhere in their window.
- **Restarting a window: stop it and wait** until it has exited
  (`stop.sh` does). A window still hanging up swallows the next launch —
  the single-instance plugin hands it the arguments and the new process
  quits, so `launch.sh` prints the *old* pid.
- **Zaps** (`docs/TESTING.md` §7a): `examples/fake_network` prints each
  account's address and NWC string; `examples/nwc_pay` pays an invoice
  copied from the window, as a phone would. `HIT_ZAP_FEE_ADDRESS` (debug
  builds) points the fee at a fake account. `stop.sh` stops the fake and
  the test anchor too.
