#!/usr/bin/env bash
# Download the real test media used by the benchmarks and the showcase
# scenario (~2.3GB, plus a generated 1-hour 4K file). Works on Linux and
# macOS. Everything is public domain or CC-licensed.
set -euo pipefail
DEST="${1:-$HOME/Videos/viode-test-media}"
mkdir -p "$DEST" && cd "$DEST"

echo "== His Girl Friday (1940, public domain, 1h32m) =="
curl -L --retry 3 -o his_girl_friday.mp4 \
  "https://archive.org/download/his_girl_friday/his_girl_friday.mp4"

echo "== Sita Sings the Blues (CC-BY-SA, 1h22m) =="
curl -L --retry 3 -o sita_1080p.mp4 \
  "https://archive.org/download/Sita_Sings_the_Blues/SSTB_2009_02_1920x1080.mp4"

echo "== Big Buck Bunny 4K60 (CC-BY, 10m34s) =="
curl -L --retry 3 -o bbb_4k.mp4.zip \
  "https://download.blender.org/demo/movies/BBB/bbb_sunflower_2160p_60fps_normal.mp4.zip"
unzip -o -q bbb_4k.mp4.zip && mv -f bbb_sunflower_2160p_60fps_normal.mp4 bbb_4k.mp4
rm -f bbb_4k.mp4.zip

echo "== 1 hour of genuine 4K60 (lossless loop, ~3.9GB) =="
ffmpeg -y -loglevel error -stream_loop 5 -i bbb_4k.mp4 \
  -c copy -map 0:v -map 0:a:0 bbb_4k_1h.mp4

ls -la "$DEST"
echo "done — point scripts/test-real-media.sh and the scenarios here"
