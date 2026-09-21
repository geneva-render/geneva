#!/usr/bin/env bash
# Times the jobs people actually run, on each renderer, and prints one
# table. Meant for a machine with a GPU, to answer whether compositing
# on the device is worth it on work of a realistic length and shape.
#
# Usage: gpu-bench.sh [--geneva PATH] [--seconds N] [--dir DIR]
#   --geneva PATH  the binary to time; target/release/geneva by default
#   --seconds N    how long the built source should be; 60 by default
#   --dir DIR      where to work; a temporary directory by default
#
# The short examples in the repository are concatenated, without
# re-encoding, into one source of the asked-for length. Every job then
# runs twice on each renderer and the better wall clock of the two is
# reported, so a cold page cache or a first-call driver setup does not
# decide the answer.
#
# The encoder is whatever the machine offers and is the same for both
# renderers, so the difference between the columns is the compositing
# and not the encode. The report says which encoder ran.
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
geneva=""
seconds=60
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    --geneva) geneva=$2; shift 2 ;;
    --seconds) seconds=$2; shift 2 ;;
    --dir) dir=$2; shift 2 ;;
    -h | --help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "gpu-bench.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$geneva" ] || geneva=$root/target/release/geneva
[ -x "$geneva" ] || { echo "gpu-bench.sh: no geneva at $geneva" >&2; exit 1; }
geneva=$(cd "$(dirname "$geneva")" && pwd)/$(basename "$geneva")
[ -n "$dir" ] || dir=$(mktemp -d "${TMPDIR:-/tmp}/geneva-bench.XXXXXX")
mkdir -p "$dir"
cd "$root/examples" || exit 1

echo "$("$geneva" --version), jobs of ${seconds}s"
echo

# One source of the asked-for length, by copying the streams of the
# short example end to end. No re-encode, so this costs nothing and the
# picture is the same one throughout.
copies=$(( (seconds + 9) / 10 ))
args=""
for _ in $(seq 1 $copies); do args="$args shibuya.mp4"; done
# shellcheck disable=SC2086
"$geneva" concat $args -o "$dir/long.mp4" > "$dir/build.log" 2>&1 || {
  echo "could not build the source:"; tail -5 "$dir/build.log"; exit 1; }
echo "source: $(du -h "$dir/long.mp4" | cut -f1), $copies copies of shibuya.mp4"

# Subtitles across the whole thing rather than the first ten seconds,
# so the text is work on most frames and not a tenth of them.
python3 - "$dir" "$seconds" <<'PY'
import json, sys
out, secs = sys.argv[1], int(sys.argv[2])
words = json.load(open("words.json"))
items = words["words"] if isinstance(words, dict) and "words" in words else words
span = max(float(w.get("end", w.get("start", 0))) for w in items) or 10.0
grown = []
t = 0.0
while t < secs:
    for w in items:
        s, e = float(w.get("start", 0)) + t, float(w.get("end", 0)) + t
        if e > secs:
            break
        n = dict(w); n["start"] = round(s, 3); n["end"] = round(e, 3)
        grown.append(n)
    t += span
json.dump({"words": grown} if isinstance(words, dict) and "words" in words else grown,
          open(f"{out}/words-long.json", "w"))
print(f"subtitles: {len(grown)} words across {secs}s")
PY

# A persistent overlay, and a brief one. Both are real shapes of job:
# a badge that stays up, and a lower third that appears once.
cat > "$dir/overlay.json" <<JSON
{ "geneva": "0.4",
  "output": { "width": 1920, "height": 1080, "fps": 30 },
  "assets": { "src": { "src": "$dir/long.mp4" }, "card": { "src": "card.html" } },
  "layers": [
    { "id": "v", "clips": [ { "source": { "kind": "video", "asset": "src" } } ] },
    { "id": "card", "clips": [ { "source": { "kind": "html", "asset": "card" } } ] } ] }
JSON
cat > "$dir/lower-third.json" <<JSON
{ "geneva": "0.4",
  "output": { "width": 1920, "height": 1080, "fps": 30 },
  "assets": { "src": { "src": "$dir/long.mp4" }, "card": { "src": "card.html" } },
  "layers": [
    { "id": "v", "clips": [ { "source": { "kind": "video", "asset": "src" } } ] },
    { "id": "card", "clips": [ { "source": { "kind": "html", "asset": "card" },
        "start": "3s", "duration": "5s" } ] } ] }
JSON

# name|what to run, with RENDERER and OUT standing in
jobs=$(cat <<'LIST'
subtitles burned in, 1080p|subtitles DIRLONG -o OUT --burn DIRWORDS --fit --renderer RENDERER
markup overlay, whole run|render DIRDIR/overlay.json -o OUT --renderer RENDERER
lower third, 5s of the run|render DIRDIR/lower-third.json -o OUT --renderer RENDERER
vertical reframe, --for tiktok|convert DIRLONG -o OUT --for tiktok --renderer RENDERER
the opening, markup heavy|render opening.json -o OUT --renderer RENDERER
LIST
)

printf "%-32s %9s %10s %10s %9s\n" "job" "frames" "cpu" "gpu" "gpu/cpu"
printf "%-32s %9s %10s %10s %9s\n" "---" "------" "---" "---" "-------"

encoders=""
echo "$jobs" | while IFS='|' read -r name cmd; do
  [ -z "$name" ] && continue
  line=""
  frames=""
  for r in cpu gpu; do
    best=""
    for pass in 1 2; do
      out=$dir/$(echo "$name" | tr -cd 'a-z')-$r.mp4
      run=${cmd//RENDERER/$r}
      run=${run//OUT/$out}
      run=${run//DIRLONG/$dir\/long.mp4}
      run=${run//DIRWORDS/$dir\/words-long.json}
      run=${run//DIRDIR/$dir}
      a=$(date +%s%N)
      # shellcheck disable=SC2086
      "$geneva" $run > "$dir/last.log" 2>&1
      code=$?
      b=$(date +%s%N)
      [ "$code" = 0 ] || break
      ms=$(( (b - a) / 1000000 ))
      if [ -z "$best" ] || [ "$ms" -lt "$best" ]; then best=$ms; fi
    done
    if [ "$code" != 0 ]; then
      line="$line FAILED"
      echo "$name ($r) failed:" >> "$dir/failures.log"
      tail -4 "$dir/last.log" >> "$dir/failures.log"
      break
    fi
    [ -n "$frames" ] || frames=$(grep -o '([0-9]* frames' "$dir/last.log" | head -1 | tr -cd '0-9')
    grep -o 'encoded with [^,]*' "$dir/last.log" | head -1 >> "$dir/encoders.txt"
    line="$line $best"
  done
  set -- $line
  if [ "${1:-}" = "FAILED" ] || [ -z "${2:-}" ]; then
    printf "%-32s %9s %10s %10s %9s\n" "$name" "${frames:-?}" "see failures.log" "" ""
  else
    cpu=$1; gpu=$2
    ratio=$(awk -v c="$cpu" -v g="$gpu" 'BEGIN{ if (c>0) printf "%.2fx", g/c; else print "?" }')
    printf "%-32s %9s %9ss %9ss %9s\n" "$name" "${frames:-?}" \
      "$(awk -v m=$cpu 'BEGIN{printf "%.1f", m/1000}')" \
      "$(awk -v m=$gpu 'BEGIN{printf "%.1f", m/1000}')" "$ratio"
  fi
done

echo
echo "lower is better in the last column: below 1.00x the GPU won."
[ -s "$dir/encoders.txt" ] && echo "encoder: $(sort -u "$dir/encoders.txt" | head -2 | tr '\n' ';')"
[ -s "$dir/failures.log" ] && { echo; echo "failures:"; cat "$dir/failures.log"; }
echo "outputs in $dir"
