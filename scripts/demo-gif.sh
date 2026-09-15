#!/usr/bin/env bash
# Renders the README's demo GIFs: the lower third and the captions.
#
#   scripts/demo-gif.sh            both
#   scripts/demo-gif.sh captions   one of them
#
# The encoding settings are not arbitrary. A GIF has 256 colours for the
# whole clip, and palettegen spends them on whatever covers the most
# pixels — here a blue-white video. The card's 5px accent bar, and the
# one highlighted word in a caption, are rounding errors by that measure,
# so with too few colours, or at a width that shrinks them below about
# two pixels, they quantize to grey and the demo shows a feature the
# document does not have.
#
#   width 640     keeps the 5px bar at 2.5px after the downscale
#   192 colours   leaves room for a colour nothing else in the frame needs
#   stats_mode=diff  weights the palette towards what changes, not the sky
#   sierra2_4a    error diffusion; ordered dither smears thin features
#
# Check the result before committing it: the accent bar should be near
# #c4362f, and the word being said should be brighter than the rest of
# the caption rather than grey with it.
set -euo pipefail

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cargo build --release -p geneva-cli
geneva=./target/release/geneva

# $1 output, $2 start, $3 length, $4 input
gif() {
  ffmpeg -v error -y -ss "$2" -t "$3" -i "$4" \
    -vf "fps=8,scale=640:-1:flags=lanczos,split[s0][s1];\
[s0]palettegen=max_colors=192:stats_mode=diff[p];\
[s1][p]paletteuse=dither=sierra2_4a" "$1"
  printf 'wrote %s (%s)\n' "$1" "$(du -h "$1" | cut -f1)"
}

lower_third() {
  $geneva render examples/lower-third.json -o "$work/demo.mp4"
  # The window covers the card sliding in and two caption cues.
  gif docs/demo.gif 1.9 2.6 "$work/demo.mp4"
}

captions() {
  $geneva subtitles examples/iss.mp4 --burn examples/words.json \
    --highlight '#ffd233' \
    --style '{ "font": "700 44px Liberation Sans", "outline": "3px #000000cc", "max_width": "80%" }' \
    -o "$work/captioned.mp4"
  # The window covers one cue ending and the next one starting, so the
  # highlight moves across both.
  gif docs/captions.gif 1.4 2.6 "$work/captioned.mp4"
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
