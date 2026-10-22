#!/usr/bin/env bash
# S1, as two real OS processes: relay A (serve), app B (post + sync).
# A is stopped and restarted between rounds, so B really reconnects to a relay
# that was gone. Run `cargo build --release` first.
set -euo pipefail
cd "$(dirname "$0")"
BIN=./target/release/nostr-iroh-spike
W=$(mktemp -d)
trap 'kill "${A_PID:-}" 2>/dev/null || true; rm -rf "$W"' EXIT

start_a() {
  rm -f "$W/ticket"
  $BIN serve --db "$W/a.db" --ticket-file "$W/ticket" >/dev/null 2>"$W/a.log" &
  A_PID=$!
  for _ in $(seq 50); do [ -s "$W/ticket" ] && break; sleep 0.1; done
  [ -s "$W/ticket" ] || { echo "relay A never printed a ticket"; cat "$W/a.log"; exit 1; }
}

echo "== A and B each write 300 notes while apart"
$BIN post --db "$W/a.db" --content "from A" --count 300
$BIN post --db "$W/b.db" --content "from B" --count 300

echo "== A starts; B syncs"
start_a
$BIN sync --db "$W/b.db" --peer "$(cat "$W/ticket")"
echo "A holds: $(kill -0 $A_PID && echo running)"

echo "== A goes offline; both write more"
kill "$A_PID"; wait "$A_PID" 2>/dev/null || true
$BIN post --db "$W/a.db" --content "A while down" --count 5
$BIN post --db "$W/b.db" --content "B while A down" --count 3

echo "== A comes back (new process, new EndpointId); B syncs again"
start_a
$BIN sync --db "$W/b.db" --peer "$(cat "$W/ticket")"
kill "$A_PID"; wait "$A_PID" 2>/dev/null || true

A=$($BIN count --db "$W/a.db"); B=$($BIN count --db "$W/b.db")
echo "== A has $A, B has $B (expect 608 each)"
[ "$A" = 608 ] && [ "$B" = 608 ] && echo "PASS" || { echo "FAIL"; exit 1; }
