#!/usr/bin/env bash
# Builds the A/V sync corpus in tests/media/sync: small files whose
# timestamps carry the traps real files carry (B-frame edit lists,
# negative composition offsets, variable frame rate, streams that do not
# start at zero, an audio track starting after the video, NTSC rates,
# encoder priming), all from one scene with markers at known times: the
# picture is black except one white frame at 1.0 s and one at 2.0 s, and
# the sound is silence except a 1 kHz tone from 1.0 to 1.5 s and from
# 2.0 to 2.5 s. A file that decodes right shows the flash and starts the
# tone at the same instant. Needs ffmpeg. Run from the repository root;
# the files are committed, so this runs only when the corpus changes.
set -eu
out=${1:-tests/media/sync}
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# One frame of white at 1.0 s and at 2.0 s (frame-rate independent: the
# frame whose start lies in [T, T + 1/fps) is white).
flash() { # fps
  echo "drawbox=enable='gte(t,1)*lt(t,1+1/($1))+gte(t,2)*lt(t,2+1/($1))':color=white:t=fill"
}
tone="volume='if(between(t,1,1.5)+between(t,2,2.5),1,0)':eval=frame"
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
video() { # fps duration
  echo "color=black:s=160x90:r=$1:d=$2"
}
# One millisecond per frame, so the tone's edges (shaped per frame) land
# within a millisecond of where they are asked.
audio() { # rate duration
  echo "sine=frequency=1000:sample_rate=$1:duration=$2:samples_per_frame=$(($1 / 1000))"
}
x264=(-c:v libx264 -preset veryfast -crf 12 -pix_fmt yuv420p)

# Plain constant frame rate, no B-frames: the control.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24)" -af "$tone" \
   "${x264[@]}" -bf 0 -g 12 -c:a aac -b:a 96k "$out/cfr.mp4"

# B-frames: the muxer shifts decode times with an edit list (the
# default) or with negative composition offsets.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k "$out/bframes-editlist.mp4"
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k -movflags negative_cts_offsets "$out/bframes-negative-cts.mp4"

# Variable frame rate: frames 5 to 8 and 30 to 33 are missing, so the
# frames before them last longer.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" \
   -vf "$(flash 24),select='not(between(n,5,8)+between(n,30,33))'" -fps_mode vfr -af "$tone" \
   "${x264[@]}" -bf 0 -g 12 -c:a aac -b:a 96k "$out/vfr.mp4"

# Both streams start at 10 s.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k -output_ts_offset 10 "$out/start-at-10s.mp4"

# The audio track starts half a second after the video. The offset is
# applied to the input's timestamps before the tone is shaped, so the
# tone still falls at 1.0 s of the file, in step with the flash; only
# the track's own start moves, which a reader must not shift back to
# zero.
for ext in mp4 mkv; do
  ff -f lavfi -i "$(video 24 3)" -itsoffset 0.5 -f lavfi -i "$(audio 48000 2.5)" \
     -map 0:v -map 1:a -vf "$(flash 24)" -af "$tone" \
     "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k "$out/audio-late.$ext"
done

# A phone recording: NTSC rate in a 600-tick timebase with every
# timestamp jittered by up to 4 ms either way, so the average rate the
# file reports (frames over duration) is not the rate any frame has.
# Not in the sync table: the direct path samples such a file on its grid
# and a frame that lands late moves to the next slot, which the marks
# would report as a slip. Here for the copy planner, which must still
# read the file as 30000/1001.
ff -f lavfi -i "$(video 30000/1001 3)" -f lavfi -i "$(audio 48000 3)" \
   -vf "$(flash 30000/1001),setpts='(N/(30000/1001)+0.004+(random(1)-0.5)*0.008)/TB'" -fps_mode passthrough \
   -af "$tone" "${x264[@]}" -bf 0 -g 15 -c:a aac -b:a 96k -video_track_timescale 600 "$out/jitter-2997.mov"

# NTSC rates.
ff -f lavfi -i "$(video 30000/1001 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 30000/1001)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 15 -c:a aac -b:a 96k "$out/ntsc-2997.mp4"
ff -f lavfi -i "$(video 24000/1001 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24000/1001)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k "$out/film-2398.mp4"

# 44.1 kHz audio against 24 fps video.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 44100 3)" -vf "$(flash 24)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k "$out/aac-44k.mp4"

# Opus in WebM: 6.5 ms of pre-skip the decoder must drop, and VP9.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24)" -af "$tone" \
   -c:v libvpx-vp9 -crf 20 -b:v 0 -g 12 -pix_fmt yuv420p -c:a libopus -b:a 64k "$out/opus.webm"

# MPEG-TS: timestamps start at 1.4 s, as the muxer does by default.
ff -f lavfi -i "$(video 24 3)" -f lavfi -i "$(audio 48000 3)" -vf "$(flash 24)" -af "$tone" \
   "${x264[@]}" -bf 3 -g 12 -c:a aac -b:a 96k "$out/mpegts.ts"

ls -l "$out"
