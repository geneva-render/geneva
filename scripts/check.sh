#!/bin/bash
# Exercises geneva end to end on one machine and prints a timing table.
#
# Usage: check.sh [input.mp4] [--geneva PATH] [--keep] [--quick]
#
# Without an input, a 10-second 720p test clip is rendered first. Every step
# is timed; when ffmpeg is installed, the steps it can do are timed with it
# too, at comparable settings. Outputs go to a temporary directory unless
# --keep is given.
set -u

geneva=""
input=""
keep=0
quick=0
while [ $# -gt 0 ]; do
  case "$1" in
    --geneva) geneva=$2; shift 2 ;;
    --keep) keep=1; shift ;;
    --quick) quick=1; shift ;;
    -h | --help) sed -n '2,10p' "$0"; exit 0 ;;
    *) input=$1; shift ;;
  esac
done
if [ -z "$geneva" ]; then
  here=$(cd "$(dirname "$0")" && pwd)
  if [ -x "$here/geneva" ]; then geneva=$here/geneva
  elif [ -x "$here/../target/release/geneva" ]; then geneva=$here/../target/release/geneva
  else geneva=$(command -v geneva || true); fi
fi
[ -x "$geneva" ] || { echo "check.sh: geneva binary not found; pass --geneva PATH" >&2; exit 1; }
ffmpeg=$(command -v ffmpeg || true)

work=$(mktemp -d "${TMPDIR:-/tmp}/geneva-check.XXXXXX")
if [ "$keep" = 0 ]; then trap 'rm -rf "$work"' EXIT; fi
log=$work/log.txt
: > "$log"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
elapsed() { perl -e "printf '%.1f', $2 - $1"; }
size_of() { if [ -f "$1" ]; then du -k "$1" | awk '{printf "%.1f MB", $1/1024}'; else echo "-"; fi; }

rows=()
last=0
# run NAME OUT CMD... : times a geneva step whose output is OUT ("-" if none).
run() {
  local name=$1 out=$2; shift 2
  local t0 t1 status
  t0=$(now)
  { echo "== $name: $*"; "$@"; } >>"$log" 2>&1
  status=$?
  t1=$(now)
  local secs; secs=$(elapsed "$t0" "$t1")
  local verdict="ok"
  if [ $status -ne 0 ]; then verdict="FAIL (exit $status)"
  elif [ "$out" != "-" ] && ! has_output "$out"; then verdict="FAIL (no output)"
  fi
  last=${#rows[@]}
  rows[$last]="$name|$verdict|${secs}s|__FF__|$(size_of "$out")"
}
# has_output PATH : true if PATH exists and is non-empty, or names a written image sequence.
has_output() {
  case "$1" in
    *%*) local glob; glob=$(basename "$1" | sed 's/%[0-9]*d/*/'); ls "$(dirname "$1")"/$glob >/dev/null 2>&1 ;;
    *) [ -s "$1" ] ;;
  esac
}
# ff CMD... : times the ffmpeg equivalent of the last step and fills its column.
ff() {
  if [ -z "$ffmpeg" ]; then noff; return; fi
  local t0 t1
  t0=$(now)
  { echo "== ffmpeg: $*"; "$ffmpeg" -hide_banner -loglevel error -y "$@"; } >>"$log" 2>&1
  t1=$(now)
  rows[$last]=${rows[$last]//__FF__/$(elapsed "$t0" "$t1")s}
}
noff() { rows[$last]=${rows[$last]//__FF__/-}; }

echo "geneva $("$geneva" --version | awk '{print $2}') on $(uname -s) $(uname -m)$( [ -n "$ffmpeg" ] && echo ", comparing with $("$ffmpeg" -version | head -1 | awk '{print $3}')")"
echo "working in $work"

# --- test material -----------------------------------------------------------
cat > "$work/shapes.json" <<'JSON'
{ "geneva": "0.1",
  "output": { "width": 1280, "height": 720, "fps": 30, "duration": "10s", "background": "#1d2230" },
  "layers": [
    { "clips": [ { "source": { "kind": "shape", "shape": "ellipse", "width": 240, "height": 240, "fill": "#ff8800" },
        "transform": { "position": { "keyframes": [ { "t": 0, "v": { "x": "15%", "y": "50%" }, "ease": "ease-in-out" }, { "t": "10s", "v": { "x": "85%", "y": "50%" } } ] },
                       "rotation": { "keyframes": [ { "t": 0, "v": 0 }, { "t": "10s", "v": 720 } ] } } } ] },
    { "clips": [ { "source": { "kind": "shape", "shape": "rect", "width": 400, "height": 90, "fill": "#101820c0", "radius": 16 },
        "transform": { "position": { "x": "50%", "y": "85%" } } } ] },
    { "clips": [ { "source": { "kind": "text", "text": "geneva check", "size": 48, "color": "white" },
        "transform": { "position": { "x": "50%", "y": "85%" } } } ] }
  ] }
JSON
cat > "$work/logo.json" <<'JSON'
{ "geneva": "0.1", "output": { "width": 320, "height": 120, "fps": 1, "duration": "1s", "background": "#00000000" },
  "layers": [ { "clips": [ { "source": { "kind": "shape", "shape": "rect", "width": 320, "height": 120, "fill": "#ffffffc0", "radius": 24 } } ] },
              { "clips": [ { "source": { "kind": "text", "text": "LOGO", "size": 64, "color": "#1d2230" } } ] } ] }
JSON
printf '1\n00:00:00,500 --> 00:00:02,500\nFirst subtitle\n\n2\n00:00:03,000 --> 00:00:05,000\nSecond <i>subtitle</i>\n' > "$work/en.srt"

run "render timeline (720p, shapes+text)" "$work/rendered.mp4" "$geneva" render "$work/shapes.json" -o "$work/rendered.mp4"; noff
if [ -z "$input" ]; then input=$work/rendered.mp4; echo "no input given; using the rendered clip"; fi
run "frame to PNG" "$work/logo.png" "$geneva" frame "$work/logo.json" -o "$work/logo.png"; noff

# --- everyday verbs ------------------------------------------------------------
run "probe" - "$geneva" probe "$input"; noff
run "trim, copied (2s..7s)" "$work/trim-copy.mp4" "$geneva" trim "$input" -o "$work/trim-copy.mp4" --from 2s --duration 5s
ff -ss 2 -i "$input" -t 5 -c copy "$work/ff-trim-copy.mp4"
run "trim, exact (re-encode H.264)" "$work/trim-exact.mp4" "$geneva" trim "$input" -o "$work/trim-exact.mp4" --from 2s --duration 5s --exact
if [ "$(uname -s)" = Darwin ]; then ff -ss 2 -i "$input" -t 5 -c:v h264_videotoolbox -q:v 55 -c:a aac "$work/ff-trim-exact.mp4"
else ff -ss 2 -i "$input" -t 5 -c:v libx264 -preset medium -crf 23 -c:a aac "$work/ff-trim-exact.mp4"; fi
run "resize to 360p" "$work/small.mp4" "$geneva" resize "$input" -o "$work/small.mp4" --height 360
if [ "$(uname -s)" = Darwin ]; then ff -i "$input" -vf scale=-2:360 -c:v h264_videotoolbox -q:v 55 -c:a aac "$work/ff-small.mp4"
else ff -i "$input" -vf scale=-2:360 -c:v libx264 -preset medium -crf 23 -c:a aac "$work/ff-small.mp4"; fi
run "audio extract, copied (m4a)" "$work/sound.m4a" "$geneva" audio "$input" -o "$work/sound.m4a" --extract
ff -i "$input" -vn -c:a copy "$work/ff-sound.m4a"
run "audio extract to WAV" "$work/sound.wav" "$geneva" audio "$input" -o "$work/sound.wav" --extract
ff -i "$input" -vn "$work/ff-sound.wav"
run "concat two copies, copied" "$work/joined.mp4" "$geneva" concat "$work/trim-copy.mp4" "$work/trim-copy.mp4" -o "$work/joined.mp4"; noff
run "concat with crossfade (rendered)" "$work/faded.mp4" "$geneva" concat "$work/trim-exact.mp4" "$work/trim-exact.mp4" -o "$work/faded.mp4" --crossfade 0.5s; noff
run "overlay a PNG" "$work/branded.mp4" "$geneva" overlay "$work/trim-exact.mp4" "$work/logo.png" -o "$work/branded.mp4" --at bottom-right --scale 0.5 --opacity 0.9; noff
run "subtitles attach (mkv)" "$work/subbed.mkv" "$geneva" subtitles "$work/trim-copy.mp4" -o "$work/subbed.mkv" --add "$work/en.srt" --language en; noff
run "subtitles extract (vtt)" "$work/back.vtt" "$geneva" subtitles "$work/subbed.mkv" -o "$work/back.vtt" --extract; noff
if [ "$quick" = 0 ]; then
  run "5s to VP9/Opus (webm)" "$work/short.webm" "$geneva" trim "$input" -o "$work/short.webm" --from 2s --duration 5s
  ff -ss 2 -i "$input" -t 5 -c:v libvpx-vp9 -crf 31 -b:v 0 -row-mt 1 -c:a libopus "$work/ff-short.webm"
  run "5s to ProRes HQ (mov)" "$work/short.mov" "$geneva" trim "$input" -o "$work/short.mov" --from 2s --duration 5s --codec prores --profile hq
  ff -ss 2 -i "$input" -t 5 -c:v prores_ks -profile:v 3 -c:a pcm_s16le "$work/ff-short.mov"
  run "5s to DNxHR HQ (mxf)" "$work/short.mxf" "$geneva" trim "$input" -o "$work/short.mxf" --from 2s --duration 5s --profile dnxhr-hq
  ff -ss 2 -i "$input" -t 5 -c:v dnxhd -profile:v dnxhr_hq -c:a pcm_s24le "$work/ff-short.mxf"
  mkdir -p "$work/frames"
  run "1s to PNG sequence" "$work/frames/%04d.png" "$geneva" trim "$input" -o "$work/frames/%04d.png" --duration 1s
  ff -i "$input" -t 1 "$work/frames/ff-%04d.png"
fi
if [ "$(uname -s)" = Darwin ]; then
  cat > "$work/hevc.json" <<'JSON'
{ "geneva": "0.1", "output": { "width": 1280, "height": 720, "fps": 30, "duration": "3s", "encode": { "video": { "codec": "h265", "hardware": "require" } } },
  "layers": [ { "clips": [ { "source": { "kind": "solid", "color": "#336699" } } ] } ] }
JSON
  run "H.265 via VideoToolbox" "$work/hevc.mp4" "$geneva" render "$work/hevc.json" -o "$work/hevc.mp4"; noff
fi

# --- summary -----------------------------------------------------------------
echo
printf '%-38s %-16s %9s %9s %10s\n' "step" "status" "geneva" "ffmpeg" "output"
printf '%-38s %-16s %9s %9s %10s\n' "----" "------" "------" "------" "------"
fails=0
for row in "${rows[@]}"; do
  IFS='|' read -r name verdict g f size <<<"$row"
  printf '%-38s %-16s %9s %9s %10s\n' "$name" "$verdict" "$g" "$f" "$size"
  case "$verdict" in FAIL*) fails=$((fails + 1)) ;; esac
done
echo
if [ $fails -gt 0 ]; then echo "$fails step(s) failed; details in $log"; [ "$keep" = 0 ] && cp "$log" "${TMPDIR:-/tmp}/geneva-check.log" && echo "log kept at ${TMPDIR:-/tmp}/geneva-check.log"; exit 1; fi
echo "all steps passed"
[ "$keep" = 1 ] && echo "outputs kept in $work"
exit 0
