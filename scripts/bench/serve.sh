#!/usr/bin/env bash
# serve.sh — Carol, up: serves the seeded groups and their pictures, and
# syncs every second. Run in the background; stop.sh-style cleanup kills it.
cd "$(dirname "$0")/../.."
exec ./target/release/hit-cli --data target/bench/carol --keychain-service hit-enter-bench \
  --relays none --port 47120 run --every 1
