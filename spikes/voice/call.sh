#!/usr/bin/env bash
# Spike S3 helper — set up and run a callme voice call on Linux (Omarchy).
#
#   ./call.sh setup            install Rust (mise), clone + build callme outside this repo
#   ./call.sh devices          show PipeWire defaults and the devices callme can see
#   ./call.sh accept           wait for a call, print our node id  (run on ONE machine)
#   ./call.sh connect <ID>     dial the node id the other side printed
#
# Extra flags go after the node id / command, e.g.:
#   ./call.sh accept --disable-processing     (no echo cancellation; headphones only)
#
# Overrides: CALLME_DIR, LOG_DIR, CALLME_IN, CALLME_OUT (callme device names;
# default "pulse", i.e. whatever PipeWire's default source/sink is).
#
# callme is NOT vendored here — it is built in $CALLME_DIR, outside the repo,
# and logs go to $LOG_DIR. Nothing from either belongs in a commit.

set -euo pipefail

CALLME_DIR="${CALLME_DIR:-$HOME/Work/tries/callme}"
LOG_DIR="${LOG_DIR:-$HOME/Work/tries/callme-logs}"
BIN="$CALLME_DIR/target/release/callme-cli"
IN_DEV="${CALLME_IN:-pulse}"
OUT_DEV="${CALLME_OUT:-pulse}"

with_cargo() {
  if ! command -v cargo >/dev/null 2>&1; then
    command -v mise >/dev/null 2>&1 && eval "$(mise env -s bash)"
  fi
}

need_bin() {
  [[ -x "$BIN" ]] || { echo "callme-cli not built — run: $0 setup" >&2; exit 1; }
}

cmd_setup() {
  command -v mise >/dev/null 2>&1 || { echo "mise not found" >&2; exit 1; }
  with_cargo
  if ! command -v cargo >/dev/null 2>&1; then
    mise use -g rust@stable
    eval "$(mise env -s bash)"
  fi
  echo "cargo: $(cargo --version)"

  local missing=()
  for p in opus alsa-lib dbus autoconf automake libtool clang webrtc-audio-processing-1; do
    pacman -Q "$p" >/dev/null 2>&1 || missing+=("$p")
  done
  if ((${#missing[@]})); then
    echo "missing packages: ${missing[*]}" >&2
    echo "install them with: sudo pacman -S --needed ${missing[*]}" >&2
    exit 1
  fi

  mkdir -p "$(dirname "$CALLME_DIR")" "$LOG_DIR"
  [[ -d "$CALLME_DIR/.git" ]] || git clone https://github.com/n0-computer/callme.git "$CALLME_DIR"
  (cd "$CALLME_DIR" && cargo build --release -p callme-cli)
  echo "built: $BIN"

  pacman -Q rtkit >/dev/null 2>&1 ||
    echo "note: rtkit is not installed — calls work, but audio threads are not real-time (sudo pacman -S rtkit)"
}

cmd_devices() {
  need_bin
  command -v wpctl >/dev/null 2>&1 && wpctl status 2>/dev/null | sed -n '/^Audio/,/^Video/p'
  if command -v pactl >/dev/null 2>&1; then
    echo
    echo "Active jack/port per analog device (headset on the motherboard jacks should be"
    echo "a Line Out / Headphones sink and a Rear/Front Microphone source):"
    { pactl list sinks; pactl list sources; } 2>/dev/null | awk '
      /^\tName:/ {n=$2}
      /^\tActive Port:/ && n !~ /\.monitor$/ {print "  " n " -> " $3}'
  fi
  echo
  "$BIN" list-devices 2>&1 | grep -v 'ALSA lib'
}

run_call() {
  need_bin
  mkdir -p "$LOG_DIR"
  local role="$1"; shift
  local log="$LOG_DIR/$role-$(hostname)-$(date +%Y%m%d-%H%M%S).log"
  echo "log: $log"
  echo "devices: in=$IN_DEV out=$OUT_DEV  (PipeWire default; set CALLME_IN / CALLME_OUT to change)"
  RUST_LOG="${RUST_LOG:-info,iroh=info}" "$BIN" -i "$IN_DEV" -o "$OUT_DEV" "$@" 2>&1 |
    grep --line-buffered -v 'ALSA lib' | tee "$log"
}

case "${1:-}" in
  setup)   cmd_setup ;;
  devices) cmd_devices ;;
  accept)  shift; run_call accept "$@" accept ;;
  connect)
    shift
    [[ -n "${1:-}" ]] || { echo "usage: $0 connect <NODE_ID>" >&2; exit 1; }
    id="$1"; shift
    run_call connect "$@" connect "$id"
    ;;
  *) sed -n '2,15p' "$0"; exit 1 ;;
esac
