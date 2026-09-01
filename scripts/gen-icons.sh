#!/usr/bin/env bash
# Rasterize packaging/icons/viode.svg into the hicolor PNG set that the
# Linux packages install. Output lands in packaging/icons/hicolor/ (kept
# out of git — regenerate any time; the SVG is the source of truth).
# The macOS .icns is generated on the Mac side (iconutil is macOS-only).
set -euo pipefail
cd "$(dirname "$0")/.."

SVG=packaging/icons/viode.svg
OUT=packaging/icons/hicolor

command -v rsvg-convert > /dev/null || {
    echo "rsvg-convert not found (package: librsvg)" >&2
    exit 1
}

for size in 16 24 32 48 64 128 256 512; do
    dir="$OUT/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "$SVG" -o "$dir/viode.png"
done
mkdir -p "$OUT/scalable/apps"
cp "$SVG" "$OUT/scalable/apps/viode.svg"
echo "wrote $OUT"
