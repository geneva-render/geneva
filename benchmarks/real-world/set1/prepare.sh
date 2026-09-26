#!/usr/bin/env bash
# Cuts the film windows the three jobs read, the way the other set does:
# x264 CRF 18, a fixed 48-frame GOP, BT.709 tagged, AAC 192k.
# Preparation, not measured. Run ../fetch.sh first.
set -euo pipefail
B=$(cd "$(dirname "$0")" && pwd)
MOV="$B/../src/tears_of_steel_1080p.mov"
EX="$B/../../../examples"
cut() { [ -f "$B/media/$1" ] || ffmpeg -hide_banner -v error -y -ss "$2" -i "$MOV" -t "$3" \
  -c:v libx264 -preset medium -crf 18 -g 48 -keyint_min 48 -sc_threshold 0 -pix_fmt yuv420p \
  -color_primaries bt709 -color_trc bt709 -colorspace bt709 -c:a aac -b:a 192k -ar 48000 -ac 2 \
  -movflags +faststart "$B/media/$1"; }
cut social.mp4 190 60
cut broadcast.mp4 0 180
cp "$EX/opening.html" "$EX/shibuya.mp4" "$B/media/"
mkdir -p "$B/remotion/public"
for f in social broadcast shibuya; do ln -f "$B/media/$f.mp4" "$B/remotion/public/$f.mp4"; done
(cd "$B/remotion" && npm ci --no-audit --no-fund)
(cd "$B/pw" && npm install --no-audit --no-fund)
ls -la "$B/media"
