# Changelog

All notable changes to Geneva are recorded here. The project follows
semantic versioning once it reaches 1.0; until then minor versions may
change the timeline format, and the `geneva` field in every document names
the format version it was written for.

## 0.1.4

### Fixes

- On macOS, a release archive fetched with a browser carries the
  quarantine flag on everything it unpacks, and Gatekeeper refuses the
  unsigned binary in place even after `install.sh` has installed a clean
  copy. `install.sh` now clears the flag on the unpacked folder as well,
  and `check.sh` clears it on the binary it runs.

## 0.1.3

### Performance

- A resize, or a change of codec that needs another sample layout (ProRes
  and DNxHR take 10-bit 4:2:2), no longer passes through the compositor
  when the picture is otherwise untouched: frames are scaled and repacked
  straight from the decoder to the encoder in their coded encoding. On a
  four-core machine a 720p to 360p resize went from 3.4 s to 0.8 s for 10 s
  of video (ffmpeg: 0.8 s) and ProRes HQ from 7.1 s to 4.1 s (ffmpeg: about
  the same).
- Decoders use every core as well; the decoder context had the same
  threading gap as the encoders.

## 0.1.2

### Fixes

- Writing the audio of a long output no longer slows to a crawl after the
  last frame: samples were shifted through the pending buffer once per
  encoder frame, which is quadratic in the length of the audio. A seven
  minute join now finishes seconds after its last frame instead of minutes.
- ProRes, DNxHR, PNG and Motion JPEG encode on all cores; the encoder
  context did not allow frame or slice threading, so FFmpeg's own encoders
  ran on one thread.
- A video whose picture could be copied but whose audio codec the
  container refuses (AAC into MXF, for one) is rendered instead of failing
  at muxing.
- `render` prints a "mixing audio" line so a long job shows what it is
  doing after the last frame.

### Installing

- Release archives include `install.sh`, which puts `geneva` on the PATH
  (and clears the macOS quarantine flag), and `check.sh`, which runs the
  everyday commands on one machine and prints a timing table with ffmpeg
  alongside when it is installed.
- The release workflow signs and notarizes the macOS binaries when the
  Apple signing secrets are configured.

## 0.1.1

### Codecs and containers

- ProRes (422 Proxy, LT, 422, HQ, 4444, 4444 XQ) and DNxHR (LB, SQ, HQ,
  HQX, 444) video, chosen with `encode.video.profile` or `--profile`; the
  encoder now packs 10-bit 4:2:2 and 4:4:4 as well as 8-bit layouts.
- PNG and Motion JPEG video, and image sequences: an output path with a
  numbered pattern such as `frames/%04d.png` writes one file per frame.
- MP3, Vorbis, ALAC, AC-3 and 24-bit PCM audio; MP3 as an audio-only
  container.
- MXF, defaulting to DNxHR HQ with 24-bit PCM.

### Subtitles

- Subtitle assets (`.srt`, `.vtt`) and `subtitles` tracks in the timeline,
  written as text streams (3GPP timed text in MP4 and MOV, SubRip in MKV,
  WebVTT in WebM) with language and title.
- `geneva subtitles --add` attaches subtitle files to a video, `--extract`
  writes a subtitle stream out as SRT or WebVTT; `probe` lists subtitle
  streams.

### Fixes

- Opus and Vorbis sources decoded short: the sample-rate converter's
  output was sized from the first, shorter frame and every later frame was
  capped to it.
- A demuxer's transient "try again" was taken as the end of the stream.

### Diagnostics

- E421: a codec profile that does not belong to the chosen codec.

## 0.1.0

First release.

### Timeline format 0.1

- JSON documents with a versioned `geneva` field, validated against a
  published JSON Schema (`geneva schema`, `schema/`).
- Layers of clips with solid, shape, image, video, text and nested
  composition sources; audio tracks with gain and fades.
- Exact rational time; times as seconds, milliseconds, frames or timecode.
- Keyframed values with CSS easing names, cubic Béziers and springs.
- Transforms (anchor, position, scale, rotation), fit modes, opacity, blend
  modes computed in linear light, crossfade transitions.
- Color tags per asset and per output, with documented inference for
  untagged material.
- Diagnostics with stable codes, JSON pointers and suggested fixes.

### Commands

- `validate`, `frame`, `render`, `probe`, `schema`.
- `trim`, `concat`, `convert`, `resize`, `overlay`, `audio`, each building
  a timeline and rendering it through the same engine; `--show-timeline`
  prints that timeline.
- `--format json` on every command for programs and agents.

### Engine

- CPU reference renderer in premultiplied linear light with supersampled
  edges and bit-exact pixel-aligned images; row-parallel on a thread pool.
- Text layout and shaping through cosmic-text and swash, with font assets
  and system fallback.
- Media through statically linked, trimmed media libraries: H.264 (OpenH264
  in software; NVENC and VideoToolbox in hardware), H.265 (hardware), VP9,
  AV1; AAC, Opus, FLAC and PCM audio; MP4, MOV, MKV, WebM and audio-only
  M4A, Ogg, FLAC and WAV.
- Stream copy for trims and joins that leave the picture untouched, with
  keyframe-snapped cuts reported and `--exact` to re-encode instead; when
  a re-encode is needed anyway, decoded frames go straight to the encoder.
- Golden-frame test harness with perceptual comparison.

### Platforms

- Prebuilt binaries for Linux x86_64 and arm64 and macOS arm64 and x86_64.
  Windows builds from source without media support.
