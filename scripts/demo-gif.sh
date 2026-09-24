#!/usr/bin/env bash
# Renders the README's demo pictures: the lower third and the captions.
#
#   scripts/demo-gif.sh            all of them
#   scripts/demo-gif.sh captions   one of them
#
# The two animations are animated WebP: the whole clip, 640 wide at
# 20 fps, full colour. Every frame is a keyframe (kmax=1). With the
# encoder's default of patching each frame from the one before, the
# bright hull of the capsule kept stale blocks for several frames at any
# quality that stayed near 5 MB; whole frames at quality 75 cost the
# same and have none. The GIFs these replaced needed a 192-colour
# palette tuned so the card's accent bar stayed red.
#
# Check the result before committing it: the accent bar should be near
# #c4362f, and the word being said should be brighter than the rest of
# the caption.
set -euo pipefail

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cargo build --release -p geneva-cli
geneva=./target/release/geneva

# $1 output, $2 input: the whole clip, 640 wide at 20 fps.
webp() {
  ffmpeg -v error -y -i "$2" -vf "fps=20,scale=640:-1:flags=lanczos" "$work/f%04d.png"
  python3 - "$1" "$work" <<'PY'
import glob, sys
from PIL import Image
out, work = sys.argv[1], sys.argv[2]
frames = [Image.open(f).convert("RGB") for f in sorted(glob.glob(f"{work}/f*.png"))]
frames[0].save(out, save_all=True, append_images=frames[1:], duration=50, loop=0,
               quality=75, method=6, kmin=0, kmax=1)
PY
  rm -f "$work"/f*.png
  printf 'wrote %s (%s)\n' "$1" "$(du -h "$1" | cut -f1)"
}

lower_third() {
  $geneva render examples/lower-third.json -o "$work/demo.mp4"
  webp docs/demo.webp "$work/demo.mp4"
}

captions() {
  $geneva subtitles examples/iss.mp4 --burn examples/words.json \
    --highlight '#ffd233' \
    --style '{ "font": "700 44px Liberation Sans", "outline": "3px #000000cc", "max_width": "80%" }' \
    -o "$work/captioned.mp4"
  webp docs/captions.webp "$work/captioned.mp4"
}

# The still beside the document form, which has a second caption layer
# from a SubRip file so the figure shows both kinds of source.
two_layers() {
  $geneva frame examples/captions.json --at 2s -o "$work/two.png"
  ffmpeg -v error -y -i "$work/two.png" -vf scale=760:-1:flags=lanczos docs/captions.png
  printf 'wrote %s (%s)\n' docs/captions.png "$(du -h docs/captions.png | cut -f1)"
}

# The tall frame, small enough to sit beside the text that explains it.
reframe() {
  $geneva frame examples/social-reframe.json --at 2s -o "$work/reframe.png"
  ffmpeg -v error -y -i "$work/reframe.png" -vf scale=270:-1:flags=lanczos docs/social-reframe.png
  printf 'wrote %s (%s)\n' docs/social-reframe.png "$(du -h docs/social-reframe.png | cut -f1)"
}

case "${1:-all}" in
  all) lower_third; captions; two_layers; reframe ;;
  demo | lower-third) lower_third ;;
  captions) captions; two_layers ;;
  reframe | social) reframe ;;
  *) echo "usage: $0 [all|lower-third|captions|reframe]" >&2; exit 2 ;;
esac
