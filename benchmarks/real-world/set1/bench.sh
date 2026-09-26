#!/usr/bin/env bash
# Three real-world jobs, three ways: geneva, Playwright + ffmpeg, Remotion.
# Every arm encodes x264 CRF 23, preset medium, 4:2:0.
#   bench.sh <runs> [scenario ...]    scenarios: social broadcast opening
set -uo pipefail
B=$(cd "$(dirname "$0")" && pwd)
GV=${GENEVA:-geneva}
# Remotion uses its own Chrome unless CHROME names one (the published
# numbers used Playwright's headless shell 1194 for both browser arms).
HS=${CHROME:-}
RUNS=${1:-3}; shift || true
SCEN=${*:-social broadcast opening}
X264=(-c:v libx264 -crf 23 -preset medium -pix_fmt yuv420p)
busy() { awk '/^cpu /{print $2+$3+$4+$7+$8}' /proc/stat; }
row() { local arm=$1 job=$2 run=$3; shift 3
  local c0 c1 s e; c0=$(busy); s=$(date +%s.%N)
  "$@" > "$B/log-$arm-$job.txt" 2>&1; local rc=$?
  e=$(date +%s.%N); c1=$(busy)
  echo "$arm $job $run $(echo "$e - $s" | bc) $(( (c1 - c0) / $(getconf CLK_TCK) )) rc=$rc" | tee -a "$B/results.txt"; }

geneva() { cd "$B" && "$GV" render "$1.json" -o "$B/out/gv-$1.mp4" "${@:2}"; }

pw() { local job=$1; cd "$B" && rm -rf "f-$job" && case $job in
  social)
    node pw/shoot.cjs pw/social.html 1080 1920 24 1440 "f-$job" &&
    ffmpeg -loglevel error -y -i media/social.mp4 -framerate 24 -i "f-$job/%05d.png" -filter_complex \
      "[0:v]split[a][b];[a]scale=-2:1920,crop=1080:1920,gblur=sigma=45[bg];[b]scale=1080:-2[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2[base];[base][1:v]overlay=format=auto[v]" \
      -map "[v]" -map 0:a -c:a copy "${X264[@]}" "$B/out/pw-$job.mp4" ;;
  broadcast)
    node pw/shoot.cjs pw/broadcast.html 1920 800 24 4320 "f-$job" &&
    ffmpeg -loglevel error -y -i media/broadcast.mp4 -framerate 24 -i "f-$job/%05d.png" -filter_complex \
      "[0:v][1:v]overlay=format=auto[v]" -map "[v]" -map 0:a -c:a copy "${X264[@]}" "$B/out/pw-$job.mp4" ;;
  opening)
    node pw/shoot.cjs pw/opening.html 960 540 30 291 "f-$job" &&
    ffmpeg -loglevel error -y -f lavfi -i color=black:s=960x540:r=30:d=13 -i media/shibuya.mp4 -framerate 30 -i "f-$job/%05d.png" -filter_complex \
      "[1:v]fps=30,setpts=PTS-STARTPTS+8.4/TB,fade=t=out:st=12.2:d=0.8[v];[0:v][v]overlay=eof_action=pass[b];[b][2:v]overlay=eof_action=pass[o]" \
      -map "[o]" -t 13 "${X264[@]}" "$B/out/pw-$job.mp4" ;;
  esac; }

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
