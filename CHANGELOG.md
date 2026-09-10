# Changelog

All notable changes to Geneva are recorded here. The project follows
semantic versioning once it reaches 1.0; until then minor versions may
change the timeline format, and the `geneva` field in every document names
the format version it was written for.

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
  keyframe-snapped cuts reported and `--exact` to re-encode instead.
- Golden-frame test harness with perceptual comparison.

### Platforms

- Prebuilt binaries for Linux x86_64 and arm64 and macOS arm64 and x86_64.
  Windows builds from source without media support.
