#!/usr/bin/env bash
# Cuts the film windows the four jobs read, the way the 2026-09-21
# harness cut its source: x264 CRF 18, a fixed 48-frame GOP, BT.709
# tagged, AAC 192k. Preparation, not measured.
set -euo pipefail
B=$(cd "$(dirname "$0")" && pwd)
MOV="$B/../src/tears_of_steel_1080p.mov"
cut() { [ -f "$B/media/$1" ] || ffmpeg -hide_banner -v error -y -ss "$2" -i "$MOV" -t "$3" \
  -c:v libx264 -preset medium -crf 18 -g 48 -keyint_min 48 -sc_threshold 0 -pix_fmt yuv420p \
  -color_primaries bt709 -color_trc bt709 -colorspace bt709 -c:a aac -b:a 192k -ar 48000 -ac 2 \
  -movflags +faststart "$B/media/$1"; }
cut intro-film.mp4 97 5
cut captions-film.mp4 190 240
cut dynamic-film.mp4 300 90
mkdir -p "$B/remotion/public"
for f in intro-film captions-film dynamic-film; do ln -f "$B/media/$f.mp4" "$B/remotion/public/$f.mp4"; done
(cd "$B/remotion" && npm ci --no-audit --no-fund)
(cd "$B/pw" && npm install --no-audit --no-fund)
ls -la "$B/media"
