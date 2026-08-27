#!/usr/bin/env bash
# Long-form performance check: generate a long 720p file and time the
# operations that must stay fast on podcast-length footage.
# Usage: ./scripts/bench-longform.sh [minutes]   (default 10)
set -euo pipefail
cd "$(dirname "$0")/.."

MINUTES="${1:-10}"
VIODE="${VIODE:-$(pwd)/target/release/viode}"
[ -x "$VIODE" ] || { echo "build first: cargo build --release"; exit 1; }

work=$(mktemp -d /tmp/viode-bench.XXXXXX)
trap 'rm -rf "$work"' EXIT
echo "== generating ${MINUTES}min 720p source (one-time cost) =="
time ffmpeg -y -loglevel error \
  -f lavfi -i "testsrc2=duration=$((MINUTES * 60)):size=1280x720:rate=30" \
  -f lavfi -i "anoisesrc=color=pink:d=$((MINUTES * 60))" \
  -c:v libx264 -preset ultrafast -pix_fmt yuv420p -c:a aac -shortest \
  "$work/long.mp4"

cd "$work"
"$VIODE" new bench >/dev/null && cd bench

echo "== add (first probe) ==";        time "$VIODE" add ../long.mp4 >/dev/null
echo "== 50 splits (model ops) ==";    time for i in $(seq 0 49); do "$VIODE" split "$i" 0.2 >/dev/null; done
echo "== proxy build (540p) ==";       time "$VIODE" proxy >/dev/null
echo "== silence scan (full file) =="; time "$VIODE" silences 50 >/dev/null
echo "== ls (positions over 51 clips) =="; time "$VIODE" ls >/dev/null
"$VIODE" ls | tail -1
echo "done — model ops must be instant, scans linear in footage, renders proportional to output."
