#!/usr/bin/env bash
# M5's "done when", by hand, with four real hit-cli processes on ONE machine,
# relays off, fixed ports 7401–7404. See docs/TESTING.md §1.
#
#   cargo build -p hit-cli && scripts/m5-by-hand.sh [scratch-dir]
#
# A owns a group; B is a moderator; C and M are members. B bans M. M's
# patched client — `hit-cli lie`, which signs and hands a message straight to
# a device without judging it — posts to A, B and C, dated now and
# backdated. No honest peer stores or shows any of it, and M's old invite no
# longer lets her in.
#
# Identities and the group key go into the OS keychain under the service
# `hit-enter-m5-test`, never a real one, and are cleared on exit.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/target/debug/hit-cli
D=${1:-$(mktemp -d)}
SVC=hit-enter-m5-test
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  command -v secret-tool >/dev/null && secret-tool clear service $SVC
}
trap cleanup EXIT

rm -rf "$D"/{a,b,c,m} "$D"/*.log
cli() { local n=$1; shift; "$BIN" --data "$D/$n" --keychain-service $SVC --relays none "$@"; }
dev()  { cli "$1" id | awk '/device/{print $2}'; }
npub() { cli "$1" id | awk '/npub/{print $2}'; }
declare -A PORT=([a]=7401 [b]=7402 [c]=7403 [m]=7404)
run() { cli "$1" --port "${PORT[$1]}" run --every 2 >"$D/$1.log" 2>&1 & PIDS+=($!); eval "P_$1=$!"; }
lies() { cli "$1" log "$G" 2>/dev/null | grep -c "lie" ; }

for n in a b c m; do cli $n init >/dev/null; done
for n in a b c m; do for k in a b c m; do
  [ $n != $k ] && cli $n peer "$(dev $k)" "127.0.0.1:${PORT[$k]}" >/dev/null
done; done

echo "== A's group; B, C, M join; B is made a moderator"
G=$(cli a group create Moderated | awk '$1=="group"{print $2}')
LINK=$(cli a invite "$G" --addr "127.0.0.1:${PORT[a]}")
run a; sleep 2
for n in b c m; do cli $n --port "${PORT[$n]}" join "$LINK" >/dev/null; done
cli a promote "$G" "$(npub b)" mod >/dev/null
run b; run c; run m; sleep 6
cli m say "$G" "mallory, still welcome" >/dev/null
sleep 5
echo "C has: $(cli c log "$G" 2>/dev/null | tail -1)"

echo "== B bans M"
cli b ban "$G" "$(npub m)"
sleep 6
for n in a b c; do
  printf '%s: ' "$n"; cli $n group show "$G" | grep -c "$(npub m)" | sed 's/^0$/M is not a member/;s/^[1-9].*/M is STILL listed/'
done

echo "== M's patched client lies to A, B and C (expect: refused, three times twice)"
kill "$P_m"; wait "$P_m" 2>/dev/null
for n in a b c; do
  cli m lie "$(dev $n)" "$G" "a lie after the ban"
  cli m lie "$(dev $n)" "$G" "a backdated lie" --ago 60
done
echo "== M tries her old invite (expect: join FAILED)"
cli m --port "${PORT[m]}" join "$LINK"
sleep 4
echo "== What the honest peers hold (expect 0 lies each)"
for n in a b c; do echo "$n: $(lies $n) lies"; done
echo "M's message from before the ban stays (it is history, not a lie):"
cli c log "$G" 2>/dev/null | grep "still welcome"
