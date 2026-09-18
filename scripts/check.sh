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
if [ -n "$input" ] && [ ! -f "$input" ]; then
  echo "check.sh: no such file: $input" >&2
  exit 2
fi
if [ -z "$geneva" ]; then
  here=$(cd "$(dirname "$0")" && pwd)
  if [ -x "$here/geneva" ]; then geneva=$here/geneva
  elif [ -x "$here/../target/release/geneva" ]; then geneva=$here/../target/release/geneva
  else geneva=$(command -v geneva || true); fi
fi
[ -x "$geneva" ] || { echo "check.sh: geneva binary not found; pass --geneva PATH" >&2; exit 1; }
# A binary unpacked from a browser download carries the macOS quarantine
# flag, and an unsigned one is then refused outright; clear it first.
if command -v xattr >/dev/null 2>&1; then xattr -d com.apple.quarantine "$geneva" 2>/dev/null || true; fi
ffmpeg=$(command -v ffmpeg || true)
# H.264 at settings comparable to geneva's default on this platform.
if [ "$(uname -s)" = Darwin ]; then ffh264=(-c:v h264_videotoolbox -q:v 55); else ffh264=(-c:v libx264 -preset medium -crf 23); fi

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
  { echo "== $name: $*"; "$@"; } >"$work/step.log" 2>&1
  status=$?
  t1=$(now)
  cat "$work/step.log" >>"$log"
  local secs; secs=$(elapsed "$t0" "$t1")
  local verdict="ok"
  if [ $status -ne 0 ]; then verdict="FAIL (exit $status)"
  elif [ "$out" != "-" ] && ! has_output "$out"; then verdict="FAIL (no output)"
  elif grep -q "copied without re-encoding" "$work/step.log"; then verdict="ok, copied"
  elif [ "${name#*copied}" != "$name" ]; then verdict="ok, RE-ENCODED"
  fi
  last=${#rows[@]}
  rows[$last]="$name|$verdict|${secs}s|__FF__|$(size_of "$out")|__FFOUT__"
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
  rows[$last]=${rows[$last]//__FFOUT__/$(size_of "${*: -1}")}
}
noff() { rows[$last]=${rows[$last]//__FF__/-}; rows[$last]=${rows[$last]//__FFOUT__/-}; }

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
printf '1\n00:00:00,500 --> 00:00:02,500\nFirst subtitle\n\n2\n00:00:03,000 --> 00:00:04,500\nSecond <i>subtitle</i>\n\n3\n00:00:04,500 --> 00:00:05,000\nA third cue long enough that it has to fold onto more than one line, and then some, to see what the fitting does with it\n' > "$work/en.srt"
cat > "$work/markup.html" <<'HTML'
<style>
  body { margin: 0; width: 960px; height: 540px; background: #0b1418; font-family: sans-serif; color: #eefbf4; overflow: hidden }
  .blob { position: absolute; width: 120%; height: 180%; animation: drift 5s cubic-bezier(.45,0,.25,1) infinite alternate }
  .blob.teal { left: -40%; top: -70%; background: radial-gradient(circle at 50% 50%, rgba(0,238,225,.35) 0%, rgba(0,238,225,.12) 40%, rgba(0,238,225,0) 70%) }
  .blob.gold { right: -50%; bottom: -80%; background: radial-gradient(circle at 50% 50%, rgba(255,210,51,.3) 0%, rgba(255,210,51,.1) 38%, rgba(255,210,51,0) 68%); animation-delay: -2.5s }
  @keyframes drift { from { transform: none } to { transform: translate(14%, 9%) scale(1.12) rotate(12deg) } }
  .scene { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center }
  .card { display: flex; flex-direction: column; width: 620px; padding: 36px 44px; border-radius: 28px; overflow: hidden; background: rgba(12,40,44,.82);
    box-shadow: 0 24px 64px rgba(0,0,0,.5); animation: card-in .9s cubic-bezier(.2,.8,.3,1) forwards, card-out .8s 4.1s ease-in forwards }
  @keyframes card-in { from { opacity: 0; transform: translateY(70px) scale(.92) } to { opacity: 1; transform: none } }
  @keyframes card-out { to { opacity: 0; transform: scale(.7); filter: blur(10px) } }
  .title { display: flex; font-size: 76px; font-weight: 700; letter-spacing: -.02em; line-height: 1.1 }
  .c { display: block; opacity: 0; animation: type .45s cubic-bezier(.2,.75,.3,1) forwards }
  @keyframes type { from { opacity: 0; filter: blur(8px); transform: translateY(18px) scale(1.3) } to { opacity: 1; filter: blur(0); transform: none } }
  .sub { margin-top: 10px; font-size: 24px; letter-spacing: .12em; color: #8fd4c8; opacity: 0; animation: rise .6s 1.1s ease-out forwards }
  @keyframes rise { from { opacity: 0; transform: translateY(12px) } to { opacity: 1; transform: none } }
  .bar { height: 6px; margin-top: 26px; background: linear-gradient(90deg, #00eee1, #ffd233);
    clip-path: polygon(0 0, 0 0, 0 100%, 0 100%); animation: wipe 1.6s 1.3s cubic-bezier(.62,.03,.31,1) forwards }
  @keyframes wipe { to { clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%) } }
</style>
<div class="blob teal"></div>
<div class="blob gold"></div>
<div class="scene"><div class="card">
  <div class="title"><span class="c" style="animation-delay:.25s">g</span><span class="c" style="animation-delay:.32s">e</span><span class="c" style="animation-delay:.39s">n</span><span class="c" style="animation-delay:.46s">e</span><span class="c" style="animation-delay:.53s">v</span><span class="c" style="animation-delay:.6s">a</span><span class="c" style="animation-delay:.67s">&nbsp;</span><span class="c" style="animation-delay:.74s">c</span><span class="c" style="animation-delay:.81s">h</span><span class="c" style="animation-delay:.88s">e</span><span class="c" style="animation-delay:.95s">c</span><span class="c" style="animation-delay:1.02s">k</span></div>
  <div class="sub">MARKUP ON EITHER RENDERER</div>
  <div class="bar"></div>
</div></div>
HTML
cat > "$work/markup.json" <<'JSON'
{ "geneva": "0.4", "output": { "width": 960, "height": 540, "fps": 30, "duration": "5s", "background": "black" },
  "assets": { "markup": { "src": "markup.html" } },
  "layers": [ { "clips": [ { "source": { "kind": "html", "asset": "markup" }, "duration": "5s" } ] } ] }
JSON
cat > "$work/styled.json" <<'JSON'
{ "geneva": "0.1", "output": { "width": 1280, "height": 720, "fps": 30, "duration": "3s", "background": "#1d2230" },
  "layers": [ { "clips": [ { "source": { "kind": "text", "text": "CSS shorthands", "font": "italic 600 72px/1.2 sans-serif", "color": "#ffdd00",
      "shadow": "0 4px 12px #000c", "outline": "3px #101820", "background": "#ffffff20", "padding": "16px", "radius": 12 } } ] } ] }
JSON

run "render timeline (720p, shapes+text)" "$work/rendered.mp4" "$geneva" render "$work/shapes.json" -o "$work/rendered.mp4" --renderer cpu; noff
# The same scene composited on the GPU; the note in the log names the
# device, or says why the CPU was used instead.
run "render timeline (720p, GPU)" "$work/rendered-gpu.mp4" "$geneva" render "$work/shapes.json" -o "$work/rendered-gpu.mp4" --renderer gpu; noff
if [ -z "$input" ]; then input=$work/rendered.mp4; echo "no input given; using the rendered clip"; fi
run "frame to PNG" "$work/logo.png" "$geneva" frame "$work/logo.json" -o "$work/logo.png"; noff

# --- everyday verbs ------------------------------------------------------------
run "probe" - "$geneva" probe "$input"; noff
run "trim, copied (2s..7s)" "$work/trim-copy.mp4" "$geneva" trim "$input" -o "$work/trim-copy.mp4" --from 2s --duration 5s
ff -ss 2 -i "$input" -t 5 -c copy "$work/ff-trim-copy.mp4"
run "trim, exact (smart cut)" "$work/trim-exact.mp4" "$geneva" trim "$input" -o "$work/trim-exact.mp4" --from 2s --duration 5s --exact
ff -ss 2 -i "$input" -t 5 "${ffh264[@]}" -c:a aac "$work/ff-trim-exact.mp4"
input_height=$("$geneva" --format json probe "$input" 2>/dev/null | sed -n 's/.*"height": *\([0-9]*\).*/\1/p' | head -1)
if [ "${input_height:-0}" -gt 720 ]; then
  run "resize to 720p" "$work/hd.mp4" "$geneva" resize "$input" -o "$work/hd.mp4" --height 720
  ff -i "$input" -vf scale=-2:720 "${ffh264[@]}" -c:a aac "$work/ff-hd.mp4"
fi
run "resize to 360p" "$work/small.mp4" "$geneva" resize "$input" -o "$work/small.mp4" --height 360
ff -i "$input" -vf scale=-2:360 "${ffh264[@]}" -c:a aac "$work/ff-small.mp4"
run "audio extract, copied (m4a)" "$work/sound.m4a" "$geneva" audio "$input" -o "$work/sound.m4a" --extract
ff -i "$input" -vn -c:a copy "$work/ff-sound.m4a"
run "audio extract to WAV" "$work/sound.wav" "$geneva" audio "$input" -o "$work/sound.wav" --extract
ff -i "$input" -vn "$work/ff-sound.wav"
# Chunked encoding: the same re-encode in one run and in stretches at
# once (auto decides from the encoder and this machine's cores; the
# report says how many ran). Compare the two times.
run "re-encode, chunks off" "$work/one-run.mp4" "$geneva" convert "$input" -o "$work/one-run.mp4" --crf 23 --chunks 1; noff
run "re-encode, chunks auto" "$work/chunked.mp4" "$geneva" convert "$input" -o "$work/chunked.mp4" --crf 23 --chunks auto; noff
run "concat two copies, copied" "$work/joined.mp4" "$geneva" concat "$work/trim-copy.mp4" "$work/trim-copy.mp4" -o "$work/joined.mp4"; noff
run "concat with crossfade (rendered)" "$work/faded.mp4" "$geneva" concat "$work/trim-exact.mp4" "$work/trim-exact.mp4" -o "$work/faded.mp4" --crossfade 0.5s; noff
run "overlay a PNG" "$work/branded.mp4" "$geneva" overlay "$work/trim-exact.mp4" "$work/logo.png" -o "$work/branded.mp4" --at bottom-right --scale 0.5 --opacity 0.9; noff
run "overlay a PNG for 2s of 5s (smart cut)" "$work/branded2.mp4" "$geneva" overlay "$work/trim-exact.mp4" "$work/logo.png" -o "$work/branded2.mp4" --at bottom-right --scale 0.5 --start 1s --duration 2s; noff
run "subtitles attach (mkv)" "$work/subbed.mkv" "$geneva" subtitles "$work/trim-copy.mp4" -o "$work/subbed.mkv" --add "$work/en.srt" --language en; noff
run "subtitles extract (vtt)" "$work/back.vtt" "$geneva" subtitles "$work/subbed.mkv" -o "$work/back.vtt" --extract; noff
run "subtitles burn-in, --fit (5s)" "$work/burned.mp4" "$geneva" subtitles "$work/trim-exact.mp4" -o "$work/burned.mp4" --burn "$work/en.srt" --fit
if [ -n "$ffmpeg" ] && "$ffmpeg" -hide_banner -filters 2>/dev/null | grep -q ' subtitles '; then ff -i "$work/trim-exact.mp4" -vf "subtitles=$work/en.srt" "${ffh264[@]}" -c:a copy "$work/ff-burned.mp4"; else noff; fi
run "text with CSS shorthands (3s)" "$work/styled.mp4" "$geneva" render "$work/styled.json" -o "$work/styled.mp4"; noff
# Animated markup (letters typed in with a blur, a card with a rounded
# clip and a polygon wipe, drifting gradients): the boxes are painted on
# the CPU either way, the groups composited on the renderer chosen.
run "render markup (960x540, 5s)" "$work/markup.mp4" "$geneva" render "$work/markup.json" -o "$work/markup.mp4" --renderer cpu; noff
run "render markup (960x540, 5s, GPU)" "$work/markup-gpu.mp4" "$geneva" render "$work/markup.json" -o "$work/markup-gpu.mp4" --renderer gpu; noff
run "targets table" - "$geneva" targets; noff
run "trim for tiktok (--for, 9:16 canvas)" "$work/tiktok.mp4" "$geneva" trim "$input" -o "$work/tiktok.mp4" --from 2s --duration 5s --for tiktok
ff -ss 2 -i "$input" -t 5 -vf "scale='min(1080,iw)':-2,pad=iw:iw*16/9:0:(oh-ih)/2" "${ffh264[@]}" -g 60 -movflags +faststart -c:a aac -b:a 128k "$work/ff-tiktok.mp4"
run "convert for tiktok, --fit cover (crop)" "$work/cover.mp4" "$geneva" convert "$work/trim-exact.mp4" -o "$work/cover.mp4" --for tiktok --fit cover
ff -i "$work/trim-exact.mp4" -vf "crop=ih*9/16:ih" "${ffh264[@]}" -g 60 -movflags +faststart -c:a aac -b:a 128k "$work/ff-cover.mp4"
run "convert for tiktok, --fill blur (rendered)" "$work/blurfill.mp4" "$geneva" convert "$work/trim-exact.mp4" -o "$work/blurfill.mp4" --for tiktok --fill blur; noff
run "trim for email, --budget 2MB" "$work/email.mp4" "$geneva" trim "$input" -o "$work/email.mp4" --from 2s --duration 5s --for email --budget 2MB; noff
run "frame from a video, chosen" "$work/thumb.jpg" "$geneva" frame "$input" -o "$work/thumb.jpg"
ff -i "$input" -vf "thumbnail=300" -frames:v 1 "$work/ff-thumb.jpg"
run "convert for web, copied (source fits)" "$work/web.mp4" "$geneva" convert "$work/trim-exact.mp4" -o "$work/web.mp4" --for web
ff -i "$work/trim-exact.mp4" -c copy -movflags +faststart "$work/ff-web.mp4"
if [ "$quick" = 0 ]; then
  run "5s to VP9/Opus (webm)" "$work/short.webm" "$geneva" trim "$input" -o "$work/short.webm" --from 2s --duration 5s
  ff -ss 2 -i "$input" -t 5 -c:v libvpx-vp9 -crf 31 -b:v 0 -row-mt 1 -c:a libopus "$work/ff-short.webm"
  run "5s to ProRes HQ (mov)" "$work/short.mov" "$geneva" trim "$input" -o "$work/short.mov" --from 2s --duration 5s --codec prores --profile hq
  ff -ss 2 -i "$input" -t 5 -c:v prores_ks -profile:v 3 -c:a pcm_s16le "$work/ff-short.mov"
  run "5s to DNxHR HQ (mxf)" "$work/short.mxf" "$geneva" trim "$input" -o "$work/short.mxf" --from 2s --duration 5s --profile dnxhr-hq
  ff -ss 2 -i "$input" -t 5 -c:v dnxhd -profile:v dnxhr_hq -c:a pcm_s24le -ar 48000 "$work/ff-short.mxf"
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
fmt='%-38s %-15s %8s %8s %11s %11s\n'
printf "$fmt" "step" "status" "geneva" "ffmpeg" "geneva out" "ffmpeg out"
printf "$fmt" "----" "------" "------" "------" "----------" "----------"
fails=0
for row in "${rows[@]}"; do
  IFS='|' read -r name verdict g f size fsize <<<"$row"
  printf "$fmt" "$name" "$verdict" "$g" "$f" "$size" "$fsize"
  case "$verdict" in FAIL*) fails=$((fails + 1)) ;; esac
done
echo
if [ $fails -gt 0 ]; then echo "$fails step(s) failed; details in $log"; [ "$keep" = 0 ] && cp "$log" "${TMPDIR:-/tmp}/geneva-check.log" && echo "log kept at ${TMPDIR:-/tmp}/geneva-check.log"; exit 1; fi
echo "all steps passed"
[ "$keep" = 1 ] && echo "outputs kept in $work"
exit 0
