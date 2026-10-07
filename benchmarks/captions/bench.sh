#!/usr/bin/env bash
# Runs the caption benchmark: each job rendered by geneva, timed, with
# CPU time and peak memory (measure.py). Writes one line per run to
# results/<label>.txt and prints it.
#
#   bench.sh [label]          the jobs at 1920x800 (2 min) and 3840x2160 (60 s),
#                             and the same captions through subtitles --burn
#   bench.sh [label] many     a 2-hour 1920x1080 timeline of 2,666 html captions
#                             over a solid, against a 2-minute one of 45; at
#                             6 fps and x264 ultrafast, since what it measures
#                             is memory and compile time, not the encoder
#
# Run fetch.sh first. GENEVA (default: geneva on PATH) picks the binary;
# THREADS, when set, is passed as --threads.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
geneva=${GENEVA:-geneva}
label=${1:-$(date +%Y-%m-%d)}
what=${2:-jobs}
work="$here/work"
mkdir -p "$work" "$here/results"
out="$here/results/$label.txt"
threads=()
[ -n "${THREADS:-}" ] && threads=(--threads "$THREADS")

run() { # name timeline [preset]
  local name=$1 doc=$2 preset=${3:-medium} wall cpu rss
  read -r wall cpu rss < <(python3 "$here/measure.py" \
    "$geneva" render "$doc" -o "$work/$name.mp4" --crf 17 --preset "$preset" "${threads[@]}" 2>"$work/$name.log")
  printf '%-28s wall %7.1f s  cpu %7.1f s  peak %6.2f GB\n' "$name" "$wall" "$cpu" \
    "$(echo "scale=2; $rss / 1048576" | bc)" | tee -a "$out"
}

compile() { # name timeline
  local t0 t1
  t0=$(date +%s.%N)
  "$geneva" validate "$2" >/dev/null 2>&1 || true
  t1=$(date +%s.%N)
  printf '%-28s validate %5.2f s\n' "$1" "$(echo "$t1 - $t0" | bc)" | tee -a "$out"
}

if [ "$what" = many ]; then
  python3 "$here/make-jobs.py" "$here" --jobs still --size 1920x1080 --fps 6 --seconds 7200 --suffix=-2h >/dev/null
  python3 "$here/make-jobs.py" "$here" --jobs still --size 1920x1080 --fps 6 --seconds 121.5 --suffix=-2m >/dev/null
  compile still-2h "$here/still-2h.json"
  run still-2m "$here/still-2m.json" ultrafast
  run still-2h "$here/still-2h.json" ultrafast
  exit 0
fi

python3 "$here/make-jobs.py" "$here" --video "$here/src/tos-800.mp4" --size 1920x800 --seconds 120 --suffix=-800 --native native-800 >/dev/null
python3 "$here/make-jobs.py" "$here" --video "$here/src/tos-2160.mp4" --size 3840x2160 --seconds 60 \
  --jobs still,steps,blur-in,glow --suffix=-2160 --native native-2160 >/dev/null
# The native caption paths, for comparison: an .srt on a plate, and word
# times with the spoken word lit.
burn() { # name source captions [flags]
  local name=$1 src=$2 caps=$3 wall cpu rss
  shift 3
  read -r wall cpu rss < <(python3 "$here/measure.py" \
    "$geneva" subtitles "$src" -o "$work/$name.mp4" --burn "$caps" --crf 17 --preset medium "${threads[@]}" "$@" 2>"$work/$name.log")
  printf '%-28s wall %7.1f s  cpu %7.1f s  peak %6.2f GB\n' "$name" "$wall" "$cpu" \
    "$(echo "scale=2; $rss / 1048576" | bc)" | tee -a "$out"
}
for size in 800 2160; do
  burn "srt-plate-$size" "$here/src/tos-$size.mp4" "$here/native-$size.srt" --style '{"background": "#000000b3", "padding": "8px"}'
  burn "words-highlight-$size" "$here/src/tos-$size.mp4" "$here/native-$size.words.json" --highlight '#FFD400'
done
for job in still steps blur-in stroke frosted glow; do
  run "$job-800" "$here/$job-800.json"
done
for job in still steps blur-in glow; do
  run "$job-2160" "$here/$job-2160.json"
done
