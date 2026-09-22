#!/usr/bin/env bash
# Renders the markup golden frames with a geneva binary and compares them
# with the committed references, byte for byte. Every case under
# tests/golden named markup-* is rendered, so a new one is picked up
# without editing this script.
#
# Usage: check-markup.sh [--geneva PATH]
#   --geneva PATH   the binary to test; by default target/release/geneva,
#                   then whatever scripts/fetch-build.sh unpacked
#
# It needs this checkout's tests/golden and nothing else: no toolchain, no
# media libraries. That makes it the way to check markup rendering on a
# machine with a binary built somewhere else, which is where font matching
# and coverage arithmetic have differed between machines before.
#
# The frames are written by the same code that wrote the references, so a
# frame that matches matches to the byte. One that does not is kept, and
# its path is printed.
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
geneva=""
while [ $# -gt 0 ]; do
  case "$1" in
    --geneva) geneva=$2; shift 2 ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "check-markup.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
if [ -z "$geneva" ]; then
  if [ -x "$root/target/release/geneva" ]; then
    geneva=$root/target/release/geneva
  else
    geneva=$(find "$root/geneva-build" -maxdepth 2 -name geneva -type f -perm -u+x 2>/dev/null | head -1)
  fi
fi
[ -n "$geneva" ] && [ -x "$geneva" ] || {
  echo "check-markup.sh: no geneva binary; build one or pass --geneva PATH" >&2
  exit 1
}
# The frames are rendered from tests/golden, so a relative path to the
# binary would stop meaning what it meant on the command line.
geneva=$(cd "$(dirname "$geneva")" && pwd)/$(basename "$geneva")
if command -v xattr >/dev/null 2>&1; then xattr -d com.apple.quarantine "$geneva" 2>/dev/null || true; fi

echo "$("$geneva" --version) from $geneva"
work=$(mktemp -d "${TMPDIR:-/tmp}/geneva-markup.XXXXXX")
cd "$root/tests/golden" || exit 1
bad=0
for case in markup-*; do
  [ -f "$case/golden.json" ] || continue
  # golden.json is one line: { "frames": ["1s", "2.4s"] }
  frames=$(tr -d '\n' < "$case/golden.json" | sed -n 's/.*\[\(.*\)\].*/\1/p' | tr -d '" ' | tr ',' ' ')
  for t in $frames; do
    out=$work/$case-$t.png
    if ! "$geneva" frame "$case/scene.json" --assets . --at "$t" -o "$out" >/dev/null 2>&1; then
      echo "FAIL $case $t: the frame did not render"
      bad=$((bad + 1))
      continue
    fi
    if cmp -s "$out" "$case/expected/$t.png"; then
      echo "ok   $case $t"
    else
      echo "DIFF $case $t: $out"
      bad=$((bad + 1))
    fi
  done
done
if [ "$bad" = 0 ]; then
  rm -rf "$work"
  echo "every markup frame matches its reference"
else
  echo "$bad frame(s) differ; the pictures are in $work"
  exit 1
fi
