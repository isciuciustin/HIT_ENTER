#!/usr/bin/env bash
# sinks.sh up|down — the four null sinks the windows' audio goes to.
cd "$(dirname "$0")/../.."
mods=target/gui/sink-modules
case "${1:-}" in
  up)
    mkdir -p target/gui
    for n in a_in a_out b_in b_out; do
      pactl load-module module-null-sink sink_name=hit_$n
    done > "$mods" ;;
  down)
    [ -f "$mods" ] && for m in $(cat "$mods"); do pactl unload-module "$m" || true; done
    rm -f "$mods" ;;
  *) echo "usage: sinks.sh up|down" >&2; exit 2 ;;
esac
