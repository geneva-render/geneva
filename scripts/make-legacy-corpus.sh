#!/usr/bin/env bash
# Builds tests/media/legacy: short files in the formats people still
# upload from camcorders and archives (DV, MPEG-2 in program and
# transport streams, MS-MPEG4 v3, Motion JPEG, QuickTime RLE), each with
# the sync corpus's markers moved earlier to keep DV small: black but for
# one white frame at 0.5 s, silent but for a 1 kHz tone from 0.5 to
# 0.6 s. Plus one stream geneva has no decoder for (Cinepak), for the
# probe's report of it. Needs ffmpeg. Run from the repository root; the
# files are committed, so this runs only when the corpus changes.
set -eu
out=${1:-tests/media/legacy}
mkdir -p "$out"
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
flash() { echo "drawbox=enable='gte(t,0.5)*lt(t,0.5+1/($1))':color=white:t=fill"; }
tone="volume='if(between(t,0.5,0.6),1,0)':eval=frame"
video() { echo "color=black:s=$3:r=$1:d=$2"; }
audio() { echo "sine=frequency=1000:sample_rate=$1:duration=$2:samples_per_frame=$(($1 / 1000))"; }

# DV: NTSC (4:1:1, 8:9 pixels) and PAL (4:2:0, 16:15), bottom field
# first, with PCM in the DV stream. DV is 720x480 or 720x576 at 25
# Mb/s whatever the picture, hence the short length.
ff -f lavfi -i "$(video 30000/1001 0.7 720x480)" -f lavfi -i "$(audio 48000 0.7)" \
   -vf "$(flash 30000/1001),setfield=bff" -af "$tone" -target ntsc-dv "$out/dv-ntsc.dv"
ff -f lavfi -i "$(video 25 0.7 720x576)" -f lavfi -i "$(audio 48000 0.7)" \
   -vf "$(flash 25),setfield=bff" -af "$tone" -target pal-dv "$out/dv-pal.dv"
# MPEG-2 with B-frames and MP2 audio, in a program stream (whose I and P
# frames carry only a decode time) and a transport stream.
ff -f lavfi -i "$(video 25 1 352x288)" -f lavfi -i "$(audio 48000 1)" -vf "$(flash 25)" -af "$tone" \
   -c:v mpeg2video -q:v 4 -g 12 -bf 2 -c:a mp2 -b:a 128k -f vob "$out/mpeg2.mpg"
ff -f lavfi -i "$(video 30000/1001 1 352x240)" -f lavfi -i "$(audio 48000 1)" -vf "$(flash 30000/1001)" -af "$tone" \
   -c:v mpeg2video -q:v 4 -g 15 -bf 2 -c:a mp2 -b:a 128k "$out/mpeg2.ts"
# MS-MPEG4 v3 and Motion JPEG in AVI, QuickTime RLE in MOV.
ff -f lavfi -i "$(video 25 1 160x120)" -f lavfi -i "$(audio 44100 1)" -vf "$(flash 25)" -af "$tone" \
   -c:v msmpeg4 -q:v 4 -c:a pcm_s16le "$out/msmpeg4v3.avi"
ff -f lavfi -i "$(video 25 1 160x120)" -f lavfi -i "$(audio 48000 1)" -vf "$(flash 25)" -af "$tone" \
   -c:v mjpeg -q:v 4 -pix_fmt yuvj422p -c:a pcm_s16le "$out/mjpeg.avi"
ff -f lavfi -i "$(video 25 1 160x120)" -f lavfi -i "$(audio 48000 1)" -vf "$(flash 25)" -af "$tone" \
   -c:v qtrle -c:a pcm_s16be "$out/qtrle.mov"
# A stream with no decoder in geneva.
ff -f lavfi -i "color=c=0x2060a0:s=32x32:r=10:d=0.1" -c:v cinepak "$out/cinepak.avi"

ls -l "$out"
