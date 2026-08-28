#!/usr/bin/env bash
# Realistic dogfood protocol on REAL media (~3h of film + 4K footage).
# Media: ~/Videos/viode-test-media/ (his_girl_friday.mp4, sita_1080p.mp4,
# bbb_4k.mp4). Every step is timed; read the numbers like an editor would:
# imports and edits must feel instant, scans must be linear, renders
# proportional.
set -euo pipefail
cd "$(dirname "$0")/.."
VIODE="${VIODE:-$(pwd)/target/release/viode}"
MEDIA="${MEDIA:-$HOME/Videos/viode-test-media}"
[ -x "$VIODE" ] || { echo "build first: cargo build --release"; exit 1; }
[ -f "$MEDIA/his_girl_friday.mp4" ] || { echo "media missing in $MEDIA"; exit 1; }

work=$(mktemp -d /tmp/viode-realtest.XXXXXX)
trap 'echo; echo "workdir kept for inspection: $work"' EXIT
cd "$work"

t() { local label="$1"; shift; local s=$EPOCHREALTIME; "$@" >/dev/null; local e=$EPOCHREALTIME
     printf "%-46s %8.2fs\n" "$label" "$(echo "$e - $s" | bc)"; }

echo "== The 3-hour project (His Girl Friday + Sita, real dialogue + music) =="
"$VIODE" new feature >/dev/null && cd feature
t "add 1h32m film (first probe)"          "$VIODE" add "$MEDIA/his_girl_friday.mp4"
t "add 1h22m film"                        "$VIODE" add "$MEDIA/sita_1080p.mp4"
"$VIODE" ls | tail -1
t "50 splits across a ~3h timeline"       bash -c "for i in \$(seq 0 49); do '$VIODE' split \$i 30 >/dev/null; done"
t "ls (52 clips)"                         "$VIODE" ls
t "undo-ish: rebuild via git would go here" true

echo
echo "== The podcast workflow (dialogue film stands in for an episode) =="
t "silence scan, full 1h32m of dialogue"  "$VIODE" silences 0 --min 0.8
t "audio levels map, first clip"          "$VIODE" levels 0 --window 2
t "waveform PNG, first clip"              "$VIODE" waveform 0

echo
echo "== Proxies (the architecture bet: ~60x realtime expected) =="
t "proxy build, both films (~3h total)"   "$VIODE" proxy

echo
echo "== Multicam: fake a second camera from the same event =="
# Angle 2 = same film, cropped + started 2.0s later (recorder offset).
t "create angle2 (10min re-encode)"       ffmpeg -y -loglevel error -ss 2 -t 600 \
    -i "$MEDIA/his_girl_friday.mp4" -vf "crop=iw*0.8:ih*0.8" \
    -c:v libx264 -preset veryfast -c:a aac ../angle2.mp4
t "angle add (audio auto-sync)"           "$VIODE" angle ../angle2.mp4
grep -A4 'angle' project.viode | head -6
t "take 60s from the angle"               "$VIODE" take 2 00:30 01:30

echo
echo "== The pro pass on a 60s excerpt =="
cd .. && "$VIODE" new excerpt >/dev/null && cd excerpt
"$VIODE" add "$MEDIA/his_girl_friday.mp4" --in 10:00 --out 11:00 >/dev/null
"$VIODE" add "$MEDIA/sita_1080p.mp4" --in 5:00 --out 5:30 >/dev/null
"$VIODE" fade 1 0.75 --kind bar-wipe-lr >/dev/null
"$VIODE" color 0 --saturation 0.4 --contrast 1.1 >/dev/null
"$VIODE" speed 1 1.5 >/dev/null
"$VIODE" track add pip --kind video >/dev/null
"$VIODE" add "$MEDIA/sita_1080p.mp4" --track 1 --at 5 --in 60:00 --out 60:10 >/dev/null
"$VIODE" place 0 --track 1 --x 0.72 --y 0.06 --scale 0.22 >/dev/null
"$VIODE" title "REAL MEDIA TEST" --at 1 --dur 4 --y 0.8 --color "#FFCC00" >/dev/null
t "render 80s composite (fades/PiP/grade)" "$VIODE" render
t "scope (waveform) of graded clip"        "$VIODE" scope 0
t "podcast preset export"                  "$VIODE" render --preset podcast
t "ProRes interchange export"              "$VIODE" render --codec prores

if [ -f "$MEDIA/bbb_4k.mp4" ]; then
  echo
  echo "== 4K stress (2160p60) =="
  cd .. && "$VIODE" new uhd >/dev/null && cd uhd
  t "add 4K60 file (probe)"                "$VIODE" add "$MEDIA/bbb_4k.mp4"
  t "proxy the 4K file"                    "$VIODE" proxy
  t "split x10"                            bash -c "for i in \$(seq 0 9); do '$VIODE' split \$i 20 >/dev/null; done"
  "$VIODE" trim 0 --out 00:30 >/dev/null 2>&1 || true
  t "render 30s of 4K"                     bash -c "cd .. && '$VIODE' --project uhd/project.viode render -o uhd/renders/uhd30.mp4" || true
fi

echo
echo "done. Judge like an editor: edits instant? scans linear? renders proportional?"
