#!/usr/bin/env bash
# stop.sh — closes every test window, the test anchor and the fake
# Lightning network, stops test tones, forgets the test identities. Matches
# exact process names only, and waits for them to exit: a window takes
# seconds to hang up, and one still running swallows the next launch
# (single-instance hands it the arguments and the new process exits).
pids=()
# Both builds: debug windows were missed once and kept running.
for p in $(pgrep -x hit-enter); do
  grep -qaE "target/(debug|release)/hit-enter" "/proc/$p/cmdline" && pids+=("$p")
done
for p in $(pgrep -x hit-cli); do
  grep -qa "target/bench/carol" "/proc/$p/cmdline" && pids+=("$p")
done
for p in $(pgrep -x hit-anchor); do
  grep -qa "target/gui" "/proc/$p/cmdline" && pids+=("$p")
done
for p in $(pgrep -x fake_network); do pids+=("$p"); done
[ ${#pids[@]} -gt 0 ] && kill "${pids[@]}"
for p in $(pgrep -x pw-play); do
  grep -qa "hit_" "/proc/$p/cmdline" && kill -9 "$p"
done
for _ in $(seq 1 40); do
  alive=0
  for p in "${pids[@]}"; do kill -0 "$p" 2>/dev/null && alive=1; done
  [ $alive = 0 ] && break
  sleep 0.25
done
[ "${alive:-0}" = 1 ] && echo "still running: ${pids[*]}" >&2
secret-tool clear service hit-enter-gui-test || true
