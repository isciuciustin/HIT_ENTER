#!/usr/bin/env bash
# device.sh <name> — the device id window <name> logged at start.
cd "$(dirname "$0")/../.."
grep -o "this device: [0-9a-f]*" "target/gui/$1.log" | tail -1 | cut -d' ' -f3
