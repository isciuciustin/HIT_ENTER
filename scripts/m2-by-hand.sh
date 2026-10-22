#!/usr/bin/env bash
# M2's "done when", by hand, with three real hit-cli processes on ONE machine,
# relays off, fixed ports 7101–7104. See docs/TESTING.md §1. Since M3 the
# two relays share a group (M3 replaced M2's roster and test notes).
#
#   cargo build -p hit-cli && scripts/m2-by-hand.sh [scratch-dir]
#
# Identities and group keys go into the OS keychain under the service
# `hit-enter-m2-test`, never a real one, and are cleared on exit (Linux:
# secret-tool).
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/target/debug/hit-cli
D=${1:-$(mktemp -d)}
SVC=hit-enter-m2-test
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  command -v secret-tool >/dev/null && secret-tool clear service $SVC
}
trap cleanup EXIT

rm -rf "$D"/{a,b,c} "$D"/*.log "$D"/a.json
cli() { local n=$1; shift; "$BIN" --data "$D/$n" --keychain-service $SVC --relays none "$@"; }
dev()  { cli "$1" id | awk '/device/{print $2}'; }

for n in a b c; do cli $n init >/dev/null; done
cli a peer "$(dev b)" 127.0.0.1:7102 >/dev/null
cli b peer "$(dev a)" 127.0.0.1:7101 >/dev/null
cli c peer "$(dev b)" 127.0.0.1:7102 >/dev/null

G=$(cli a group create M2 | awk '$1=="group"{print $2}')
LINK=$(cli a invite "$G" --addr 127.0.0.1:7101)

echo "== A runs; B joins; both run; A says 5"
cli a --port 7101 run --every 2 >"$D/a1.log" 2>&1 & A=$!; PIDS+=($A)
sleep 2
cli b --port 7102 join "$LINK"
cli b --port 7102 run --every 2 >"$D/b.log" 2>&1 & B=$!; PIDS+=($B)
sleep 2
for i in 1 2 3 4 5; do cli a say "$G" "alice $i" >/dev/null; done
sleep 6
echo "B has $(cli b log "$G" 2>&1 | tail -1)"

echo "== kill -9 A; B says 20"
kill -9 $A; wait $A 2>/dev/null
for i in $(seq 1 20); do cli b say "$G" "bob while alice is dead $i" >/dev/null; done
echo "A has $(cli a log "$G" 2>&1 | tail -1) while dead"

echo "== A restarts"
cli a --port 7101 run --every 2 >"$D/a2.log" 2>&1 & A=$!; PIDS+=($A)
sleep 8
echo "A has $(cli a log "$G" 2>&1 | tail -1); B has $(cli b log "$G" 2>&1 | tail -1)"
echo "distinct messages at A: $(cli a log "$G" 2>/dev/null | cut -d' ' -f4- | sort -u | wc -l)  (expect 25)"
echo "the sync that carried the twenty:"
grep -h "fetched 20\|pushed 20" "$D/a2.log" "$D/b.log" | sed 's/^.*hit_node[^ ]* //'

echo "== C presents A's binding, copied (expect: refused, different device)"
cli a --port 7103 binding >"$D/a.json" 2>/dev/null
cli c --port 7104 sync --present-binding "$D/a.json" 2>/dev/null
echo "== C with its own binding, in no group with B (expect: fetched 0, pushed 0)"
cli c --port 7104 sync 2>/dev/null
