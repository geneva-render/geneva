#!/usr/bin/env bash
# Fetches Tears of Steel (CC-BY 3.0, (C) Blender Foundation,
# mango.blender.org) and its English subtitles into src/. The film is
# not redistributed here; both jobs sets cut what they read from it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$here/src" && cd "$here/src"
if [ ! -f tears_of_steel_1080p.mov ]; then
  curl -fSL --retry 5 -o tos.zip https://download.blender.org/demo/movies/ToS/tears_of_steel_1080p.mov.zip
  unzip -o -q tos.zip && rm -f tos.zip
fi
[ -f TOS-en.srt ] || curl -fSL --retry 5 -o TOS-en.srt https://download.blender.org/demo/movies/ToS/subtitles/TOS-en.srt
ls -la "$here/src"
