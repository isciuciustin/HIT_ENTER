#!/usr/bin/env bash
# run.sh <native|web> — the load half of M12's benchmark, one app at a time,
# from a freshly seeded world (seed.sh) every run:
#
#   sign-up screen · signed in, no groups · a channel of 1,000 messages ·
#   600 more arriving live · after the burst · a channel of 50 photographs
#
# Each phase is sampled once a second by sample.py (PSS over the app's
# whole process tree, peak, threads, CPU). One window is measured; the
# phases' lines go to target/bench/<app>.txt. The window joins by a
# `hit-enter:` link handed to it as a clicked link would be, and Return.
# Driving rules are scripts/gui's: focus-checked keys, null-sink audio,
# no screenshots of file dialogs (none are opened).
set -euo pipefail
app=${1:?usage: run.sh <native|web>}
cd "$(dirname "$0")/../.."
case $app in
  # How the join dialog, open with the link, is submitted: the native
  # field is one line (Enter joins); the web one a textarea (Enter is a
  # new line), so Tab past "cancel" to "join".
  native) mode=1;           bin=./target/release/hit-enter; join=(-k Return) ;;
  web) echo "the Tauri/web window left the tree at 0.1.0-beta.3: check out 5a32529 to measure it" >&2; exit 2 ;;
  *) echo "native or web" >&2; exit 2 ;;
esac
out=target/bench/$app.txt

scripts/gui/stop.sh >/dev/null 2>&1 || true
rm -rf target/gui/a
scripts/bench/seed.sh >/dev/null
scripts/gui/sinks.sh up >/dev/null
setsid scripts/bench/serve.sh > target/bench/carol.log 2>&1 &
sleep 3
carol=$(cat target/bench/carol.device)@127.0.0.1:47120

sample() { python3 scripts/bench/sample.py "$A" "$1" "$app: $2" | tee -a "$out"; }
# A link handed over as the desktop would: a second copy with the URL.
link() {
  HIT_DATA_DIR=target/gui/a HIT_KEYCHAIN_SERVICE=hit-enter-gui-test HIT_RELAYS=none \
    HIT_PORT=47101 "$bin" "$(awk -v g="$1" '$1==g{print $2}' target/bench/links)" || true
  sleep 3
  scripts/gui/focus.sh "$A"; sleep 0.5
  scripts/gui/drive.sh "$A" "${join[@]}"
}

: > "$out"
A=$(HIT_PEERS=$carol NATIVE=$mode scripts/gui/launch.sh a 47101)
sleep 6
sample 20 "sign-up screen"
scripts/gui/onboard.sh "$A" Alice
sleep 8
sample 30 "signed in, no groups"
link history
sleep 30
count() { sqlite3 target/gui/a/node.db "select count(*) from events where kind=9" 2>/dev/null || echo "?"; }
echo "  ($(count) messages held)"
sample 30 "1,000 messages open"
C="./target/release/hit-cli --data target/bench/carol --keychain-service hit-enter-bench --relays none"
( for i in $(seq 1 600); do
    $C say history "live $i: the quick relay and the slow anchor, message number $i" >/dev/null 2>&1
  done ) &
poster=$!
sample 75 "600 arriving live"
wait "$poster"   # not a bare wait: Carol is a background job too
echo "  ($(count) messages held)"
sample 30 "after the burst, idle"
link photos
sample 90 "50 photos channel open"
scripts/gui/shot.sh "$A" "target/bench/$app-photos.png"
sample 30 "photos open, idle"

scripts/gui/stop.sh >/dev/null 2>&1 || true
scripts/gui/sinks.sh down >/dev/null 2>&1 || true
echo "($(date -u +%FT%TZ), $(nproc) cores, $(uname -r))" | tee -a "$out"
