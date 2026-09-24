#!/usr/bin/env bash
# Runs every everyday command once against the geneva on PATH, on a clip it
# renders itself, and fails on the first thing that does not work.
#
# Usage: scripts/smoke.sh
#
# The release workflow runs it on Windows, where nothing else exercises the
# binary before it is published; it runs anywhere bash does. It checks that
# each command works and writes what it should, not how good the result is:
# that is what the test suite and the goldens are for.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
mkdir a b frames

step() { echo; echo "==> $*"; }
exists() { [ -s "$1" ] || { echo "smoke.sh: $1 was not written" >&2; exit 1; }; }

geneva --version
step validate
geneva validate "$root/examples/shapes.json"
step "frame, shapes and text"
geneva frame "$root/examples/shapes.json" -o shapes.png --at 1s
exists shapes.png
step "frame, markup (system fonts)"
geneva frame "$root/examples/opening.json" -o opening.png --at 2s
exists opening.png
step "render on the CPU"
geneva render "$root/examples/shapes.json" -o a/clip.mp4 --renderer cpu
exists a/clip.mp4
step "render on the GPU, or the CPU with a note where there is none"
geneva render "$root/examples/shapes.json" -o gpu.mp4 --renderer gpu
exists gpu.mp4
step probe
geneva probe a/clip.mp4
step "convert to VP9 and AV1"
geneva convert a/clip.mp4 -o clip.webm --codec vp9
exists clip.webm
geneva convert a/clip.mp4 -o clip-av1.mp4 --codec av1
exists clip-av1.mp4
step "H.265, which needs a hardware encoder"
if geneva convert a/clip.mp4 -o clip-hevc.mp4 --codec h265 2>hevc.err; then
  exists clip-hevc.mp4
else
  cat hevc.err
  grep -q "needs a hardware encoder" hevc.err
  echo "(no hardware encoder here, and it says so)"
fi
step "trim, as a stream copy and exact"
geneva trim a/clip.mp4 -o trim.mp4 --from 1s --to 2s
exists trim.mp4
geneva trim a/clip.mp4 -o trim-exact.mp4 --from 0.5s --duration 1s --exact
exists trim-exact.mp4
step "concat, inputs in two folders"
cp a/clip.mp4 b/other.mp4
geneva concat a/clip.mp4 b/other.mp4 -o joined.mp4
exists joined.mp4
geneva concat a/clip.mp4 b/other.mp4 -o faded.mp4 --crossfade 0.5s
exists faded.mp4
step overlay
geneva overlay a/clip.mp4 shapes.png -o overlaid.mp4 --at bottom-right --scale 0.25
exists overlaid.mp4
step "audio, extracted"
geneva audio a/clip.mp4 -o clip.wav --extract
exists clip.wav
step "image sequence, and no file left named after the pattern"
geneva convert a/clip.mp4 -o frames/%04d.png
exists frames/0001.png
if ls frames | grep -q '%'; then
  echo "smoke.sh: a file named after the pattern was left in frames/" >&2
  exit 1
fi
echo
echo "smoke test passed"
