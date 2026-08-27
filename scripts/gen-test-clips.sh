#!/usr/bin/env bash
# Regenerate the gitignored test clips used by the Phase 0 spike.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p assets

ffmpeg -y -loglevel error \
  -f lavfi -i "testsrc2=duration=3:size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=440:duration=3" \
  -c:v libx264 -pix_fmt yuv420p -preset fast -c:a aac -shortest \
  assets/clip1.mp4

ffmpeg -y -loglevel error \
  -f lavfi -i "smptebars=duration=3:size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=880:duration=3" \
  -c:v libx264 -pix_fmt yuv420p -preset fast -c:a aac -shortest \
  assets/clip2.mp4

echo "wrote assets/clip1.mp4 assets/clip2.mp4"
