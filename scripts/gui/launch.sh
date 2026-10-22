#!/usr/bin/env bash
# launch.sh <name> <port> [args…] — one app window with its own data dir, relays
# off, audio pinned to the null sinks hit_<name>_in / hit_<name>_out.
# Prints the window's pid. HIT_PEERS passes through (device@ip:port,…).
# The release build; NATIVE=debug the debug build, which takes
# HIT_TEST_PICK / HIT_TEST_SAVE_DIR in place of the file dialogs. (The
# Tauri/web window left the tree at 0.1.0-beta.3; NATIVE=1 still works.)
set -euo pipefail
cd "$(dirname "$0")/../.."
dir=target/gui/$1
if [ "${NATIVE:-}" = debug ]; then
  bin=./target/debug/hit-enter
else
  bin=./target/release/hit-enter
fi
comm=hit-enter
mkdir -p "$dir"
HIT_DATA_DIR=$dir HIT_KEYCHAIN_SERVICE=hit-enter-gui-test HIT_RELAYS=none HIT_PORT=$2 \
  HIT_PEERS=${HIT_PEERS:-} HIT_AUDIO_IN=pulse HIT_AUDIO_OUT=pulse \
  PULSE_SOURCE=hit_$1_in.monitor PULSE_SINK=hit_$1_out \
  setsid "$bin" "${@:3}" >>"$dir.log" 2>&1 &
for _ in $(seq 1 40); do
  sleep 0.25
  pid=$(pgrep -n -x "$comm" || true)
  if hyprctl clients -j | python3 -c "
import json,sys
sys.exit(0 if any(c['pid']==int('${pid:-0}') for c in json.load(sys.stdin)) else 1)"; then
    echo "$pid"; exit 0
  fi
done
echo "no window" >&2; exit 1
