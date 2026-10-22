#!/usr/bin/env bash
# M12a, idle only: the memory and CPU of running apps, measured the same way
# every time, side by side.
#
#   scripts/mem-bench.sh                     # hit-enter and Discord, if running
#   scripts/mem-bench.sh hit-enter litecord  # any process names (exact comm)
#   SECONDS_IDLE=60 scripts/mem-bench.sh     # sample window (default 30 s)
#
# Memory is PSS (shared pages split between the processes that map them),
# summed over every process with that name and all their descendants — so
# WebKit's and Electron's helpers count, and nothing counts twice within an
# app. CPU is utime+stime over the window, as % of one core. Leave the apps
# alone while it samples: this is the idle number, not the under-load one.
# Linux only (/proc). Nothing here opens audio or touches an app's data.
set -euo pipefail

window=${SECONDS_IDLE:-30}
names=("$@")
[[ ${#names[@]} -gt 0 ]] || names=(hit-enter Discord)
tck=$(getconf CLK_TCK)

# Every pid whose comm is $1, plus all their descendants.
tree() {
  local -A ppid_of=() comm_of=()
  local p stat rest
  for p in /proc/[0-9]*; do
    p=${p#/proc/}
    stat=$(cat "/proc/$p/stat" 2>/dev/null) || continue
    rest=${stat##*) }                      # past "pid (comm) "
    comm_of[$p]=${stat#*(}; comm_of[$p]=${comm_of[$p]%)*}
    ppid_of[$p]=$(cut -d' ' -f2 <<<"$rest")
  done
  local -A in=()
  for p in "${!comm_of[@]}"; do [[ ${comm_of[$p]} == "$1" ]] && in[$p]=1; done
  local grew=1
  while ((grew)); do
    grew=0
    for p in "${!ppid_of[@]}"; do
      [[ -z ${in[$p]:-} && -n ${in[${ppid_of[$p]}]:-} ]] && { in[$p]=1; grew=1; }
    done
  done
  printf '%s\n' "${!in[@]}"
}

pss_kb()   { awk '/^Pss:/{print $2; exit}' "/proc/$1/smaps_rollup" 2>/dev/null || echo 0; }
ticks()    { local s; s=$(cat "/proc/$1/stat" 2>/dev/null) || { echo 0; return; }
             s=${s##*) }; awk '{print $12+$13}' <<<"$s"; }
threads()  { awk '/^Threads:/{print $2}' "/proc/$1/status" 2>/dev/null || echo 0; }
calc()     { awk "BEGIN{print $1}"; }
comm()     { cat "/proc/$1/comm" 2>/dev/null || echo '?'; }

declare -A pids t0
found=()
for n in "${names[@]}"; do
  list=$(tree "$n" | sort -n | tr '\n' ' ')
  if [[ -z ${list// } ]]; then echo "skip: no process named '$n' is running" >&2; continue; fi
  pids[$n]=$list; found+=("$n")
  sum=0; for p in $list; do sum=$((sum + $(ticks "$p"))); done; t0[$n]=$sum
done
[[ ${#found[@]} -gt 0 ]] || { echo "nothing to measure" >&2; exit 1; }

echo "sampling ${window}s of idle CPU — don't touch the apps…" >&2
sleep "$window"

printf '\n%-12s %-26s %10s %8s\n' app process "PSS MB" threads
total_line=()
for n in "${found[@]}"; do
  mem=0; thr=0; cpu=0
  declare -A by_comm=() by_thr=()
  for p in ${pids[$n]}; do
    [[ -r /proc/$p/smaps_rollup ]] || continue
    k=$(pss_kb "$p"); t=$(threads "$p"); c=$(comm "$p")
    by_comm[$c]=$(( ${by_comm[$c]:-0} + k )); by_thr[$c]=$(( ${by_thr[$c]:-0} + t ))
    mem=$((mem + k)); thr=$((thr + t)); cpu=$((cpu + $(ticks "$p")))
  done
  for c in "${!by_comm[@]}"; do
    printf '%-12s %-26s %10.1f %8d\n' "$n" "$c" "$(calc "${by_comm[$c]}/1024")" "${by_thr[$c]}"
  done | sort -k3 -nr
  pct=$(calc "($cpu - ${t0[$n]}) * 100 / ($tck * $window)")
  total_line+=("$(printf '%-12s %10.1f MB PSS %6d threads %7.2f %% of a core (idle, %ss)' \
    "$n" "$(calc "$mem/1024")" "$thr" "$pct" "$window")")
  unset by_comm by_thr
done
echo
printf '%s\n' "${total_line[@]}"
echo "($(date -u +%FT%TZ), $(nproc) cores, $(uname -r))"
