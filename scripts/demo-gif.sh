#!/usr/bin/env bash
# Renders the README's demo GIF from examples/lower-third.json.
#
# The settings are not arbitrary. A GIF has 256 colours for the whole
# clip, and palettegen spends them on whatever covers the most pixels —
# here a blue-white video. The card's 5px accent bar is a rounding error
# by that measure, so with too few colours, or at a width that shrinks it
# below about two pixels, it quantizes to grey and the demo shows a
# feature the document does not have.
#
#   width 640     keeps the 5px bar at 2.5px after the downscale
#   192 colours   leaves room for a colour nothing else in the frame needs
#   stats_mode=diff  weights the palette towards what changes, not the sky
#   sierra2_4a    error diffusion; ordered dither smears thin features
#
# Check the result before committing it: the bar should be near #4ade80.
set -euo pipefail

out=${1:-docs/demo.gif}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cargo build --release -p geneva-cli
./target/release/geneva render examples/lower-third.json -o "$work/demo.mp4"

ffmpeg -v error -y -ss 1.75 -t 2.15 -i "$work/demo.mp4" \
  -vf "fps=8,scale=640:-1:flags=lanczos,split[s0][s1];\
[s0]palettegen=max_colors=192:stats_mode=diff[p];\
[s1][p]paletteuse=dither=sierra2_4a" "$out"

printf 'wrote %s (%s)\n' "$out" "$(du -h "$out" | cut -f1)"
