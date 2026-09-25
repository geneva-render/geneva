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
python3 - "$dir" "$seconds" <<'PYEOF'
import json, sys

out, secs = sys.argv[1], int(sys.argv[2])
doc = json.load(open("words.json"))
segments = doc["segments"]
span = max(float(s["end"]) for s in segments)

def shift(seg, by):
    s = dict(seg)
    s["start"] = round(float(seg["start"]) + by, 3)
    s["end"] = round(float(seg["end"]) + by, 3)
    s["words"] = [
        dict(w, start=round(float(w["start"]) + by, 3), end=round(float(w["end"]) + by, 3))
        for w in seg.get("words", [])
    ]
    return s

grown, by, next_id = [], 0.0, 0
while by < secs:
    for seg in segments:
        if float(seg["end"]) + by > secs:
            break
        s = shift(seg, by)
        s["id"] = next_id
        next_id += 1
        grown.append(s)
    by += span
doc["segments"] = grown
json.dump(doc, open(f"{out}/words-long.json", "w"))
words = sum(len(s.get("words", [])) for s in grown)
print(f"subtitles: {len(grown)} cues, {words} words across {secs}s")
PYEOF

# Everything is timed through `render`, because `--renderer` is only on
# `render`: the verbs compile to a document and that is what is drawn.
# `--show-timeline` gives their document, so the verb jobs below are the
# same work the verb would have done.
cp card.html "$dir/card.html"

cat > "$dir/overlay.json" <<JSON
{ "geneva": "1.0",
  "output": { "width": 1920, "height": 1080, "fps": 30, "duration": "${seconds}s" },
  "assets": { "src": { "src": "long.mp4" }, "card": { "src": "card.html" } },
  "layers": [
    { "id": "v", "clips": [ { "source": { "kind": "video", "asset": "src" } } ] },
    { "id": "card", "clips": [ { "source": { "kind": "html", "asset": "card" },
        "duration": "${seconds}s" } ] } ] }
JSON
cat > "$dir/lower-third.json" <<JSON
{ "geneva": "1.0",
  "output": { "width": 1920, "height": 1080, "fps": 30, "duration": "${seconds}s" },
  "assets": { "src": { "src": "long.mp4" }, "card": { "src": "card.html" } },
  "layers": [
    { "id": "v", "clips": [ { "source": { "kind": "video", "asset": "src" } } ] },
    { "id": "card", "clips": [ { "source": { "kind": "html", "asset": "card" },
        "start": "3s", "duration": "5s" } ] } ] }
JSON

# The two verbs, as the documents they compile to.
"$geneva" subtitles --burn "$dir/words-long.json" --fit --show-timeline \
  -o "$dir/x.mp4" "$dir/long.mp4" > "$dir/subtitles.json" 2> "$dir/subtitles.err" \
  || { echo "subtitles --show-timeline failed:"; tail -3 "$dir/subtitles.err"; }
"$geneva" convert --for tiktok --show-timeline \
  -o "$dir/x.mp4" "$dir/long.mp4" > "$dir/tiktok.json" 2> "$dir/tiktok.err" \
  || { echo "convert --show-timeline failed:"; tail -3 "$dir/tiktok.err"; }

# name|document
jobs=$(cat <<'LIST'
subtitles burned in|DIR/subtitles.json
markup overlay, whole run|DIR/overlay.json
lower third, 5s of the run|DIR/lower-third.json
vertical reframe, --for tiktok|DIR/tiktok.json
the opening, markup heavy|opening.json
LIST
)

printf "%-32s %8s %10s %10s %9s\n" "job" "frames" "cpu" "gpu" "gpu/cpu"
printf "%-32s %8s %10s %10s %9s\n" "---" "------" "---" "---" "-------"

echo "$jobs" | while IFS='|' read -r name doc; do
  [ -z "$name" ] && continue
  doc=${doc//DIR/$dir}
  # A document built here reads its assets from here; one from the
  # repository reads them from beside itself.
  case "$doc" in "$dir"/*) assets=$dir ;; *) assets=$PWD ;; esac
  frames=""
  cpums=""
  gpums=""
  failed=""
  for r in cpu gpu; do
    best=""
    for pass in 1 2; do
      out=$dir/$(echo "$name" | tr -cd 'a-z')-$r.mp4
      a=$(date +%s%N)
      "$geneva" render "$doc" -o "$out" --assets "$assets" --renderer "$r" \
        > "$dir/last.log" 2>&1
      code=$?
      b=$(date +%s%N)
      if [ "$code" != 0 ]; then
        failed=1
        { echo "== $name ($r):"; tail -8 "$dir/last.log"; } >> "$dir/failures.log"
        break
      fi
      ms=$(( (b - a) / 1000000 ))
      if [ -z "$best" ] || [ "$ms" -lt "$best" ]; then best=$ms; fi
    done
    [ -n "$failed" ] && break
    [ -n "$frames" ] || frames=$(grep -oE '\(([0-9]+) frames' "$dir/last.log" | head -1 | tr -cd '0-9')
    grep -oE 'encoded with [^,]*' "$dir/last.log" | head -1 >> "$dir/encoders.txt"
    if [ "$r" = cpu ]; then cpums=$best; else gpums=$best; fi
  done
  if [ -n "$failed" ]; then
    printf "%-32s %8s %10s %10s %9s\n" "$name" "${frames:-?}" "failed" "" ""
  else
    ratio=$(awk -v c="$cpums" -v g="$gpums" 'BEGIN{ if (c>0) printf "%.2fx", g/c; else print "?" }')
    printf "%-32s %8s %9ss %9ss %9s\n" "$name" "${frames:-?}" \
      "$(awk -v m=$cpums 'BEGIN{printf "%.1f", m/1000}')" \
      "$(awk -v m=$gpums 'BEGIN{printf "%.1f", m/1000}')" "$ratio"
  fi
done

echo
echo "lower is better in the last column: below 1.00x the GPU won."
[ -s "$dir/encoders.txt" ] && echo "encoder: $(sort -u "$dir/encoders.txt" | head -2 | tr '\n' ';')"
[ -s "$dir/failures.log" ] && { echo; echo "failures:"; head -40 "$dir/failures.log"; }
echo "outputs in $dir"
