#!/usr/bin/env bash
# drive.sh <pid> <wtype args…> — types only if <pid>'s window has focus.
pid=$1; shift
act=$(hyprctl activewindow -j | python3 -c "import json,sys; print(json.load(sys.stdin).get('pid'))")
if [ "$act" != "$pid" ]; then echo "FOCUS LOST (active pid $act), not typing" >&2; exit 3; fi
wtype "$@"
