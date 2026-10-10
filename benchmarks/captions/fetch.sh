#!/usr/bin/env bash
# Fetches what the caption benchmark reads into src/ and fonts/:
# Tears of Steel (CC-BY 3.0, (C) Blender Foundation, mango.blender.org)
# through ../real-world/fetch.sh, cut to the two sources the jobs use,
# and Inter (SIL OFL) from the @fontsource package, as TrueType.
# Needs geneva on PATH, npm, and Python with fonttools and brotli.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
"$here/../real-world/fetch.sh"
film="$here/../real-world/src/tears_of_steel_1080p.mov"
mkdir -p "$here/src" "$here/fonts"
cd "$here/src"
# 2 minutes at 1920x800, and 60 s scaled to 3840x2160, both at 24 fps,
# H.264 at CRF 18: the sources of the proposal's baseline table.
# The film is 1920x800 already.
[ -f tos-800.mp4 ] || geneva trim "$film" -o tos-800.mp4 --from 6:00 --to 8:00 --exact --crf 18
# 60 s at 3840x2160 for the native 4K rows: the film is wider than
# 16:9, so convert fills the rest of the frame with its blur.
[ -f tos-2160.mp4 ] || {
  geneva trim tos-800.mp4 -o tos-60s.mp4 --to 60s --exact --crf 18
  geneva convert tos-60s.mp4 -o tos-2160.mp4 --width 3840 --height 2160 --crf 18
  rm -f tos-60s.mp4
}
cd "$here/fonts"
if [ ! -f Inter-700.ttf ]; then
  work=$(mktemp -d)
  (cd "$work" && npm pack -q @fontsource/inter@5 >/dev/null && tar -xzf fontsource-inter-*.tgz)
  for w in 400 600 700; do
    python3 -I -c 'import sys; from fontTools.ttLib import TTFont
f = TTFont(sys.argv[1]); f.flavor = None; f.save(sys.argv[2])' \
      "$work/package/files/inter-latin-$w-normal.woff2" "Inter-$w.ttf"
  done
  rm -rf "$work"
fi
ls -l "$here/src" "$here/fonts"
