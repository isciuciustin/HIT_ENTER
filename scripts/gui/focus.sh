#!/usr/bin/env bash
# focus.sh <pid> — focuses that pid's HIT_ENTER window.
addr=$(hyprctl clients -j | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c['pid']==$1 and c['title']=='HIT_ENTER': print(c['address'])")
hyprctl dispatch "hl.dsp.focus({ window = \"address:$addr\" })" >/dev/null
sleep 0.4
