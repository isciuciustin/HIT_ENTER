#!/usr/bin/env bash
# M8's "done when", by hand, with three hit-cli processes on ONE machine,
# relays off, ports 7801–7803. See docs/TESTING.md §1.
#
#   cargo build -p hit-cli && scripts/m8-by-hand.sh [scratch-dir]
#
# A posts a file and serves it; B fetches it and runs (its cache serves);
# A is gone; C fetches the same file — from B — and it matches byte for byte.
#
# Keys go into the OS keychain under `hit-enter-m8-test`, cleared on exit.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/target/debug/hit-cli
D=${1:-$(mktemp -d)}
SVC=hit-enter-m8-test
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  command -v secret-tool >/dev/null && secret-tool clear service $SVC
}
trap cleanup EXIT

rm -rf "$D"/{a,b,c} "$D"/*.log "$D"/file.bin "$D"/got-*.bin
cli() { local n=$1; shift; "$BIN" --data "$D/$n" --keychain-service $SVC --relays none "$@"; }
dev()  { cli "$1" id | awk '/device/{print $2}'; }
declare -A PORT=([a]=7801 [b]=7802 [c]=7803)

for n in a b c; do cli $n init >/dev/null; done
for n in a b c; do for m in a b c; do
  [ $n != $m ] && cli $n peer "$(dev $m)" "127.0.0.1:${PORT[$m]}" >/dev/null
done; done
head -c 400000 /dev/urandom > "$D/file.bin"

echo "== A's group; B and C join"
G=$(cli a group create Files | awk '$1=="group"{print $2}')
LINK=$(cli a invite "$G" --addr "127.0.0.1:${PORT[a]}")
cli a --port "${PORT[a]}" run --every 2 >"$D/a.log" 2>&1 & P_A=$!; PIDS+=($P_A)
sleep 2
cli b --port "${PORT[b]}" join "$LINK" >/dev/null
cli c --port "${PORT[c]}" join "$LINK" >/dev/null
kill "$P_A"; wait "$P_A" 2>/dev/null

echo "== A posts a file and serves it for 20 s; B fetches it"
"$BIN" --data "$D/a" --keychain-service $SVC --relays none --port "${PORT[a]}" \
  post-file "$G" "$D/file.bin" --text "the file" --serve-for 20 >"$D/a-post.log" 2>&1 &
P_A=$!; PIDS+=($P_A)
sleep 3
HASH=$(grep -o '[0-9a-f]\{64\}' "$D/a-post.log" | head -1)
echo "   hash ${HASH:0:16}…"
cli b --port "${PORT[b]}" get-file "$HASH" "$D/got-b.bin"
echo "== B runs (its cache serves); A goes offline"
"$BIN" --data "$D/b" --keychain-service $SVC --relays none --port "${PORT[b]}" \
  run --every 2 >"$D/b.log" 2>&1 & PIDS+=($!)
wait "$P_A" 2>/dev/null
echo "== C fetches it with A gone (expect: from B, byte for byte)"
cli c --port "${PORT[c]}" get-file "$HASH" "$D/got-c.bin"
cmp "$D/file.bin" "$D/got-c.bin" && echo "   C's copy matches the original"
cmp "$D/file.bin" "$D/got-b.bin" && echo "   B's copy matches the original"
