#!/usr/bin/env bash
# M6's "done when", by hand, on ONE machine, relays off, ports 7601–7603 and
# 7690 for the anchor's ws://. See docs/TESTING.md §1.
#
#   cargo build -p hit-cli -p hit-anchor && scripts/m6-by-hand.sh [scratch-dir]
#
# A (hit-cli) owns a group; N (hit-anchor) pins it. A's next invite names N
# first. A is killed; at "4am" only N runs; D joins from A's link and says
# something; A comes back and sees D. Then N's NIP-11 over HTTP — what a
# NIP-29 client fetches first. Reading the group with Chachi itself is
# docs/TESTING.md §5.
#
# Keys go into the OS keychain under `hit-enter-m6-test`, cleared on exit.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI=$ROOT/target/debug/hit-cli
ANCHOR=$ROOT/target/debug/hit-anchor
D=${1:-$(mktemp -d)}
SVC=hit-enter-m6-test
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  command -v secret-tool >/dev/null && secret-tool clear service $SVC
}
trap cleanup EXIT

rm -rf "$D"/{a,n,d} "$D"/*.log
cli() { local n=$1; shift; "$CLI" --data "$D/$n" --keychain-service $SVC --relays none "$@"; }
anc() { "$ANCHOR" --data "$D/n" --keychain-service $SVC --relays none "$@"; }
dev()  { cli "$1" id | awk '/device/{print $2}'; }
declare -A PORT=([a]=7601 [n]=7602 [d]=7603)

cli a init >/dev/null; cli d init >/dev/null
anc init --name "night-owls anchor" 2>/dev/null
NDEV=$(anc id | awk '/device/{print $2}')
cli a peer "$NDEV" "127.0.0.1:${PORT[n]}" >/dev/null
anc peer "$(dev a)" "127.0.0.1:${PORT[a]}" >/dev/null
# D knows only the anchor's address — what discovery would give it.
cli d peer "$NDEV" "127.0.0.1:${PORT[n]}" >/dev/null

echo "== A's group; the anchor pins it"
G=$(cli a group create "Night owls" | awk '$1=="group"{print $2}')
LINK=$(cli a invite "$G" --addr "127.0.0.1:${PORT[a]}")
cli a --port "${PORT[a]}" run --every 2 >"$D/a.log" 2>&1 & PIDS+=($!); P_A=$!
sleep 2
anc --port "${PORT[n]}" pin "$LINK"
# The binary itself, not the `anc` function: backgrounding a function
# records the subshell's pid, and cleanup would kill the wrong process.
"$ANCHOR" --data "$D/n" --keychain-service $SVC --relays none --port "${PORT[n]}" \
  run --every 2 --ws 127.0.0.1:7690 >"$D/n.log" 2>&1 & PIDS+=($!)
cli a say "$G" "said before bedtime" >/dev/null
sleep 8

echo "== A's next invite names the anchor first (expect: N's device first)"
LATE=$(cli a invite "$G" --hours 24 --addr "127.0.0.1:${PORT[a]}")
echo "$LATE" | grep -o 'p=[0-9a-f]\{10\}' | head -2
echo "   N is ${NDEV:0:10}"
# The invite must reach the anchor before A goes: `invite` signs into A's
# store and A's `run` carries it on its next sync. A link made the moment
# before its maker goes offline does not work at 4am (docs/TESTING.md §1).
sleep 6

echo "== 4am: A is killed; only the anchor runs; D joins"
kill -9 "$P_A"; wait "$P_A" 2>/dev/null
cli d --port "${PORT[d]}" join "$LATE"
cli d log "$G" 2>&1 | tail -2
cli d say "$G" "dave, at 4am" >/dev/null
cli d --port "${PORT[d]}" sync 2>/dev/null | head -2

echo "== Morning: A comes back (expect: D's message, and D a member)"
cli a --port "${PORT[a]}" run --every 2 >"$D/a2.log" 2>&1 & PIDS+=($!)
sleep 8
cli a log "$G" 2>/dev/null | grep "4am"
echo "members at A: $(cli a group show "$G" | grep -c '^  npub')  (expect 3: A, the anchor, D)"

echo "== The anchor's NIP-11 (what a NIP-29 client asks first)"
curl -s -H 'Accept: application/nostr+json' http://127.0.0.1:7690/ ; echo
