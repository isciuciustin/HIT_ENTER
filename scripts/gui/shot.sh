#!/usr/bin/env bash
# shot.sh <pid> <out.png> — screenshots that window only.
g=$(hyprctl clients -j | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c['pid']==$1: print(f\"{c['at'][0]},{c['at'][1]} {c['size'][0]}x{c['size'][1]}\")")
grim -g "$g" "$2"
