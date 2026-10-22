#!/usr/bin/env bash
# M3's "done when", by hand, with five real hit-cli processes on ONE machine,
# relays off, fixed ports 7201–7205. See docs/TESTING.md §1.
#
#   cargo build -p hit-cli && scripts/m3-by-hand.sh [scratch-dir]
#
# Three peers in a group; the owner is killed (-9); the other two keep
# talking, and a fourth joins from an invite with the owner still gone; the
# owner comes back and sees everything. A non-member asking a member for the
# group's events gets nothing.
#
# `say` signs into the store from a separate process, so here messages move
# by sync (every 2 s), not gossip; live gossip is covered by
# crates/hit-node/tests/m3_done_when.rs.
#
# Identities and the group key go into the OS keychain under the service
# `hit-enter-m3-test`, never a real one, and are cleared on exit.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/target/debug/hit-cli
D=${1:-$(mktemp -d)}
SVC=hit-enter-m3-test
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  command -v secret-tool >/dev/null && secret-tool clear service $SVC
}
trap cleanup EXIT

rm -rf "$D"/{a,b,c,d,e} "$D"/*.log
cli() { local n=$1; shift; "$BIN" --data "$D/$n" --keychain-service $SVC --relays none "$@"; }
dev()  { cli "$1" id | awk '/device/{print $2}'; }
npub() { cli "$1" id | awk '/npub/{print $2}'; }
declare -A PORT=([a]=7201 [b]=7202 [c]=7203 [d]=7204 [e]=7205)
run() { cli "$1" --port "${PORT[$1]}" run --every 2 >"$D/$1$2.log" 2>&1 & PIDS+=($!); eval "P_$1=$!"; }
count() { cli "$1" log "$G" 2>&1 >/dev/null | awk '{print $1}'; }

for n in a b c d e; do cli $n init >/dev/null; done
# Where everyone is (relays are off). E knows only B.
for n in a b c d; do for m in a b c d; do
  [ $n != $m ] && cli $n peer "$(dev $m)" "127.0.0.1:${PORT[$m]}" >/dev/null
done; done
cli e peer "$(dev b)" "127.0.0.1:${PORT[b]}" >/dev/null

echo "== Three peers in a group"
G=$(cli a group create Friends | awk '$1=="group"{print $2}')
LINK=$(cli a invite "$G" --addr "127.0.0.1:${PORT[a]}")
run a 1; sleep 2
cli b --port "${PORT[b]}" join "$LINK"
cli c --port "${PORT[c]}" join "$LINK"
run b ""; run c ""; sleep 3
cli a say "$G" "hello from alice" >/dev/null
sleep 6
cli b group show "$G"
echo "B: $(count b) messages, C: $(count c) messages (expect 1, 1)"

echo "== kill -9 the owner; B and C keep talking"
kill -9 "$P_a"; wait "$P_a" 2>/dev/null
cli b say "$G" "bob, owner gone" >/dev/null
cli c say "$G" "carol, owner gone" >/dev/null
sleep 6
echo "B: $(count b), C: $(count c) (expect 3, 3)"

echo "== D joins from B's invite, owner still gone"
LINK2=$(cli b invite "$G" --hours 1 --addr "127.0.0.1:${PORT[b]}")
cli d --port "${PORT[d]}" join "$LINK2"
run d ""; sleep 2
cli d say "$G" "dave, joined while alice was away" >/dev/null
sleep 6
echo "C: $(count c) (expect 4); C sees D as a member: $(cli c group show "$G" | grep -c "$(npub d)")"

echo "== The owner comes back"
run a 2; sleep 10
echo "A: $(count a) messages (expect 4); A sees D: $(cli a group show "$G" | grep -c "$(npub d)")"
cli a log "$G"

echo "== The owner compacts: a new snapshot at seq 1; B gets it and nothing is lost"
cli a compact "$G"
sleep 6
cli b group show "$G" | head -1
echo "B: $(count b) messages, $(cli b group show "$G" | grep -c '^  npub') members (expect 4, 4)"

echo "== E, in no group, asks B for the group's events (expect 0 served)"
cli e --port "${PORT[e]}" req "$(dev b)" --group "$G" 2>/dev/null | tail -1
echo "== ...and for everything (expect 0 served)"
cli e --port "${PORT[e]}" req "$(dev b)" 2>/dev/null | tail -1
echo "== ...and syncs (expect fetched 0)"
cli e --port "${PORT[e]}" sync 2>/dev/null
