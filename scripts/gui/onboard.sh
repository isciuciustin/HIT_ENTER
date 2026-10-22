#!/usr/bin/env bash
# onboard.sh <pid> <name> — on a fresh window: "I already have a key", a
# random throwaway nsec, import, then the name.
set -euo pipefail
here=$(dirname "$0")
"$here/focus.sh" "$1"
"$here/drive.sh" "$1" -k Tab -k Return
sleep 0.6
"$here/drive.sh" "$1" "$(python3 "$here/nsec.py")" -k Tab -k Return
sleep 2.5
"$here/focus.sh" "$1"
"$here/drive.sh" "$1" "$2" -k Return
sleep 2
