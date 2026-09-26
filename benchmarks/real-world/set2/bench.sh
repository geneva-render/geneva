#!/usr/bin/env bash
# Four more real-world jobs, three ways: geneva, Playwright + ffmpeg,
# Remotion (bundled once, JPEG frames, all four cores). Every arm encodes
# x264 CRF 23, preset medium, 4:2:0.   bench.sh <runs> [scenario ...]
set -uo pipefail
B=$(cd "$(dirname "$0")" && pwd)
GV=${GENEVA:-geneva}
# Remotion uses its own Chrome unless CHROME names one (the published
# numbers used Playwright's headless shell 1194 for both browser arms).
HS=${CHROME:-}
RUNS=${1:-3}; shift || true
SCEN=${*:-intro kinetic captions dynamic}
X264=(-c:v libx264 -crf 23 -preset medium -pix_fmt yuv420p)
busy() { awk '/^cpu /{print $2+$3+$4+$7+$8}' /proc/stat; }
row() { local arm=$1 job=$2 run=$3; shift 3
  local c0 c1 s e; c0=$(busy); s=$(date +%s.%N)
  "$@" > "$B/log-$arm-$job.txt" 2>&1; local rc=$?
  e=$(date +%s.%N); c1=$(busy)
  echo "$arm $job $run $(echo "$e - $s" | bc) $(( (c1 - c0) / $(getconf CLK_TCK) )) rc=$rc" | tee -a "$B/results.txt"; }

geneva() { cd "$B" && "$GV" render "$1.json" -o "$B/out/gv-$1.mp4" "${@:2}"; }

pw() { local job=$1 f="$B/f-$1"; cd "$B" && rm -rf "$f" && case $job in
  intro)
    node pw/shoot.cjs pw/intro.html 1920 1080 30 420 "$f" &&
    ffmpeg -loglevel error -y -f lavfi -i color=black:s=1920x1080:r=30:d=14 -i media/intro-film.mp4 -framerate 30 -i "$f/%05d.png" -filter_complex \
      "[1:v]scale=-2:1080,crop=1920:1080,fps=30,setpts=PTS-STARTPTS+9/TB[v];[0:v][v]overlay=eof_action=pass[b];[b][2:v]overlay=eof_action=pass[o];[1:a]adelay=9000|9000[a]" \
      -map "[o]" -map "[a]" -t 14 "${X264[@]}" -c:a aac -b:a 192k "$B/out/pw-$job.mp4" ;;
  kinetic)
    node pw/shoot.cjs pw/kinetic.html 1920 1080 30 480 "$f" &&
    ffmpeg -loglevel error -y -framerate 30 -i "$f/%05d.png" "${X264[@]}" "$B/out/pw-$job.mp4" ;;
  captions|dynamic)
    local frames=5760; [ $job = dynamic ] && frames=2160
    node pw/shoot.cjs "pw/$job.html" 1920 800 24 $frames "$f" &&
    ffmpeg -loglevel error -y -i "media/$job-film.mp4" -framerate 24 -i "$f/%05d.png" -filter_complex \
      "[0:v][1:v]overlay=format=auto[v]" -map "[v]" -map 0:a -c:a copy "${X264[@]}" "$B/out/pw-$job.mp4" ;;
  esac; local rc=$?; rm -rf "$f"; return $rc; }

rm_() { local job=$1; shift; cd "$B/remotion" && npx --no-install remotion render "$B/rm-bundle" "$job" "$B/out/rm-$job.mp4" \
  --crf 23 --x264-preset medium ${HS:+--browser-executable "$HS"} --log error "$@"; }

mkdir -p "$B/out"
[ -d "$B/rm-bundle" ] || (cd "$B/remotion" && npx --no-install remotion bundle src/index.ts --out-dir "$B/rm-bundle" --log error)
for run in $(seq 1 "$RUNS"); do
  for job in $SCEN; do
    row geneva $job $run geneva $job --crf 23 --preset medium
    row geneva-default $job $run geneva $job
    row playwright $job $run pw $job
    row remotion $job $run rm_ $job --concurrency 4
  done
done
echo DONE >> "$B/results.txt"
