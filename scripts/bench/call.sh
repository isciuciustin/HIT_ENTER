#!/usr/bin/env bash
# call.sh <native|web> — the call phase of M12's benchmark: Alice and Bob,
# two windows of the same app, in the voice channel "talk" of the seeded
# group, five minutes, a tone into Alice's (null-sink) microphone the whole
# time. Alice is measured: before the call, during it, and after leaving.
# Audio only ever goes to the null sinks (scripts/gui/sinks.sh) — never
# the real devices.
set -euo pipefail
app=${1:?usage: call.sh <native|web>}
cd "$(dirname "$0")/../.."
case $app in
  # Join dialog submit (see run.sh), then the Tab count from the composer
  # (after Escape) to the voice channel "talk", for someone who is not
  # the group's owner.
  native) mode=1; bin=./target/release/hit-enter; join=(-k Return); tabs=${TABS:-7} ;;
  web) echo "the Tauri/web window left the tree at 0.1.0-beta.3: check out 5a32529 to measure it" >&2; exit 2 ;;
  *) echo "native or web" >&2; exit 2 ;;
esac
out=target/bench/$app-call.txt

scripts/gui/stop.sh >/dev/null 2>&1 || true
rm -rf target/gui/a target/gui/b
scripts/bench/seed.sh >/dev/null
scripts/gui/sinks.sh up >/dev/null
setsid scripts/bench/serve.sh > target/bench/carol.log 2>&1 &
sleep 3
carol=$(cat target/bench/carol.device)@127.0.0.1:47120
link=$(awk '$1=="history"{print $2}' target/bench/links)

window() { # $1 name, $2 port, $3 person → pid, onboarded, in the group
  local pid
  pid=$(HIT_PEERS=$carol NATIVE=$mode scripts/gui/launch.sh "$1" "$2")
  sleep 6
  scripts/gui/onboard.sh "$pid" "$3" >/dev/null
  sleep 8
  HIT_DATA_DIR=target/gui/$1 HIT_KEYCHAIN_SERVICE=hit-enter-gui-test HIT_RELAYS=none \
    HIT_PORT=$2 "$bin" "$link" || true
  sleep 3
  scripts/gui/focus.sh "$pid" >/dev/null; sleep 0.5
  scripts/gui/drive.sh "$pid" "${join[@]}"
  sleep 25
  echo "$pid"
}
voice() { # $1 pid: Tab to "talk", Enter
  scripts/gui/focus.sh "$1" >/dev/null; sleep 0.5
  scripts/gui/drive.sh "$1" -k Escape
  for _ in $(seq 1 "$tabs"); do scripts/gui/drive.sh "$1" -k Tab; sleep 0.15; done
  scripts/gui/drive.sh "$1" -k Return
}
sample() { python3 scripts/bench/sample.py "$A" "$1" "$app: $2" | tee -a "$out"; }
streams() { pactl list short sink-inputs | wc -l; }

: > "$out"
A=$(window a 47101 Alice)
B=$(window b 47102 Bob)
sample 30 "two in the group, no call"
voice "$A"; sleep 3
voice "$B"; sleep 8
scripts/gui/shot.sh "$A" "target/bench/$app-call.png"
echo "  ($(streams) audio streams open, on the null sinks)" | tee -a "$out"
pw-play --target hit_a_in target/bench/tone-330s.wav &
tone=$!
sample 300 "five-minute call, speaking"
kill "$tone" 2>/dev/null || true
scripts/gui/stop.sh >/dev/null 2>&1 || true
scripts/gui/sinks.sh down >/dev/null 2>&1 || true
echo "($(date -u +%FT%TZ), $(nproc) cores, $(uname -r))" | tee -a "$out"
