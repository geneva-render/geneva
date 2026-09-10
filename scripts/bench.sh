#!/usr/bin/env sh
# Times a plain re-encode of one file with geneva and with ffmpeg at the
# same codec settings, as a baseline for performance work.
#
# Usage: scripts/bench.sh input.mp4 [crf] [preset]
set -eu
input=${1:?usage: bench.sh input.mp4 [crf] [preset]}
crf=${2:-23}
preset=${3:-medium}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

geneva=${GENEVA:-target/release/geneva}
probe=$("$geneva" --format json probe "$input")
width=$(printf '%s' "$probe" | sed -n 's/.*"width": *\([0-9]*\).*/\1/p' | head -1)
height=$(printf '%s' "$probe" | sed -n 's/.*"height": *\([0-9]*\).*/\1/p' | head -1)
fps=$(printf '%s' "$probe" | sed -n 's/.*"fps": *\([0-9.]*\).*/\1/p' | head -1)

cat > "$work/timeline.json" <<JSON
{ "geneva": "0.1",
  "output": { "width": $width, "height": $height, "fps": $fps,
              "encode": { "video": { "codec": "h264", "crf": $crf, "preset": "$preset" } } },
  "assets": { "in": { "src": "input" } },
  "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "in" }, "fit": "none" } ] } ] }
JSON
cp "$input" "$work/input"

echo "geneva render:"
time "$geneva" render "$work/timeline.json" --assets "$work" -o "$work/geneva.mp4"
echo
echo "ffmpeg:"
time ffmpeg -hide_banner -loglevel error -y -i "$input" -c:v libx264 -crf "$crf" -preset "$preset" -pix_fmt yuv420p -c:a aac -b:a 160k "$work/ffmpeg.mp4"
echo
ls -l "$work/geneva.mp4" "$work/ffmpeg.mp4"
