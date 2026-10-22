#!/usr/bin/env bash
# seed.sh — the benchmark's world, made once: Carol (a CLI node, keychain
# service hit-enter-bench, apart from the windows' — stop.sh clears theirs;
# a group "history" with 1,000 messages of mixed length, and a
# group "photos" with 50 photographs' worth of pictures (3000 × 2000 JPEG,
# ~1–2 MB each). Writes the invite links to target/bench/links.
# Then `serve.sh` keeps Carol up to serve them.
set -euo pipefail
cd "$(dirname "$0")/../.."
B=target/bench
C="./target/release/hit-cli --data $B/carol --keychain-service hit-enter-bench --relays none --port 47120"
rm -rf "$B" && mkdir -p "$B/photos"
$C init >/dev/null
$C group create history >/dev/null
$C group create photos >/dev/null
# For the call phase (call.sh): a voice channel in "history".
$C channel history talk --voice >/dev/null

# Lines of every length a chat has: a word, a sentence, a paragraph.
words=(the a node relay group channel key wallet zap voice anchor sync gossip
  invite member device message picture call quiet loud fast slow today later)
line() {
  local n=$1 out=""
  for ((w = 0; w < n; w++)); do out+="${words[RANDOM % ${#words[@]}]} "; done
  echo "${out% }"
}
RANDOM=42
for i in $(seq 1 1000); do
  case $((i % 10)) in
    0) n=60 ;; 1|2|3) n=2 ;; *) n=12 ;;
  esac
  $C say history "$i: $(line $n)" >/dev/null
done

# Photographs: noise-rich, so JPEG cannot shrink them to nothing.
for i in $(seq -w 1 50); do
  magick -size 3000x2000 -seed "$i" plasma:fractal -blur 0x1 -quality 88 "$B/photos/photo-$i.jpg"
done
for f in "$B"/photos/*.jpg; do
  $C post-file photos "$f" --serve-for 1 >/dev/null
done

{
  echo "history $($C invite history --addr 127.0.0.1:47120)"
  echo "photos $($C invite photos --addr 127.0.0.1:47120)"
} > "$B/links"
du -sh "$B/photos" | cut -f1 | sed 's/^/photos on disk: /'
$C id | sed -n 's/^device *//p' > "$B/carol.device"
