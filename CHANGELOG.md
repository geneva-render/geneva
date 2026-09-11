# Changelog

All notable changes to Geneva are recorded here. The project follows
semantic versioning once it reaches 1.0; until then minor versions may
change the timeline format, and the `geneva` field in every document names
the format version it was written for.

## Unreleased

### Added

- Timeline format 0.2. A 0.2 document is a 0.1 document with more
  optional clip fields, so `"geneva": "0.1"` documents are read as they
  are; the verbs write 0.2. The schema file is now
  `schema/geneva-timeline-0.2.schema.json`.
- `crop` on clips: a rectangle of the source (pixels or percentages of
  the source's own size) that becomes the clip's box before `fit` and the
  transform. A video shown as it is with a crop takes the direct path,
  the region scaled straight from the decoder, with bars when it does
  not fill the frame.
- `--crop RECT` on `convert`, `resize` and `trim`: `X,Y,WxH`, or `WxH`
  for the middle of the picture. The output takes the crop's size unless
  a size is given.

### Performance

- The compositor was profiled on the check scene at 720p and 1080p and
  its four largest costs fixed. At 1080p on four cores: clearing the
  frame 2.9 → 0.65 ms (parallel), the check scene 4.3 → 1.6 ms, a
  frame-sized nested composition 38 → 2.7 ms (its buffer is drawn from
  directly and kept for the next frame instead of being allocated,
  cleared and copied every frame), a scaled or fractionally placed
  image 43 → 6.4 ms (spans are resampled with a constant step and no
  per-texel bounds checks), and the 8-bit 4:2:0 conversion 6.7 → 4.4
  ms (a dedicated loop over 2×2 blocks). A magnified or unit-scale image
  is now sampled once at the pixel center well inside the picture, so
  text and pictures at fractional positions come out sharper; minified
  images, rotated ones and edges keep the 2×2 supersample. The text
  golden references were regenerated for the sharper edges.
- Decoded 8-bit 4:2:0 frames go straight to linear light for the
  compositor, with bilinear chroma upsampling at the standard siting,
  instead of through a 16-bit 4:4:4 pass first. What remains of a
  composited 1080p frame's cost is the memory traffic of the 16-byte
  working pixel (each full-frame pass moves 33 MB) and the encoder.

### Fixes

- The build without the media feature (checked by CI with clippy)
  compiled again: the examples and the tests that need media are gated
  on the feature.
- Keeping the source's audio sample rate (0.1.11) broke outputs whose
  codec or container cannot take it: Opus (48 kHz and its own rates
  only) and MXF (48 kHz only) failed on a 44.1 kHz source. The rate
  now falls back to 48 kHz where the codec or container needs it.
- A smart cut is taken only when at least a fifth of the frames can be
  copied; below that (subtitles over most of a clip) the plain encode
  was faster.
- On VideoToolbox, `--for` wrote files about twice the size of a plain
  encode: a data rate limit on top of quality mode changes how the
  hardware encoder works. The ceiling is no longer passed to it in
  quality mode (the report says so), and `--budget` now switches it to
  bitrate mode at the budget's rate, which holds the size. New encode
  field `bitrate_kbps`, which x264 ignores in favour of constant
  quality under the ceiling.

## 0.1.11

### Added

- Smart cut. With `--exact`, or with an overlay or burned-in subtitle
  shown for part of the time, an H.264 source is no longer re-encoded
  whole: its packets are copied wherever nothing changes and only the
  frames from a cut to the next keyframe, or under the overlay, are
  encoded into the same stream (system x264, CRF 18, parameter sets
  side by side with the source's). The report calls the mode `smart`
  and counts the frames copied and encoded; copied frames are
  bit-identical to the source. The clips' own audio is copied as coded
  when the container takes it (exact at the start of the file, within
  half a packet at a join), so nothing is decoded outside the encoded
  runs. The planner reads only the packets around each clip's range,
  not the whole file. A one-second overlay on a ten-second 1080p clip:
  3.0 s instead of 4.6 s; an exact trim inside a one-second GOP
  re-encodes at most a second.

### Changes

- A picture fitted onto a larger frame of one color, as a landscape
  video on the portrait canvas `--for instagram` or `--for tiktok`
  builds, takes the direct path: the frame is cleared to the background
  once and each decoded picture is scaled to its place, on several
  threads, instead of every frame passing through the compositor. A 5 s
  720p clip onto a 1080×1920 canvas takes 1.2 s where it took 5.7 s
  (ffmpeg: 1.4 s).
- `--fit cover` on `convert` and `resize` also decides how the picture
  meets the canvas `--for` builds: a landscape video on a 9:16 target is
  cropped to its center at the source's full height instead of getting
  bars. The crop takes the direct path as well (0.4 s for a 5 s 720p
  clip; ffmpeg 0.4 s), and the N410 note says which of the two was done.
- The verbs keep the source's audio sample rate and channel count when
  every input agrees, instead of writing 48 kHz stereo: a mono 44.1 kHz
  track extracted to WAV is the same size as ffmpeg's now.
- Releases carry the tag's section of this changelog as their notes; a
  `release-notes` workflow sets them on an existing release.
- `check.sh` resizes to 720p as well when the input is taller, and
  also exercises burned-in subtitles with `--fit`, CSS
  shorthands in a text source, `geneva targets`, `--for tiktok` and
  `--for email --budget`, and compares the burn-in and the portrait
  canvas with ffmpeg.

### Fixes

- `output.audio.channels: 1` was documented but every file was written
  as stereo; mono is now written as mono, downmixed from the stereo mix.

## 0.1.10

### Added

- `--for TARGET` on every verb and on `render`: encode for a destination
  (phone, tablet, desktop, tv, web, youtube, instagram, tiktok, x,
  linkedin, email). From one table and the source's facts it picks a
  size ceiling (never upscaling; a 9:16 canvas for Reels and TikTok),
  H.264 with the level the size needs, a quality tier (`--quality best`,
  `good`, `eco`), a bitrate cap, keyframes every 2 s, fast start and the
  audio bitrate, writes them into the explicit encode block, and reports
  every choice (N410). Platform limits become warnings (W411 to W414);
  `--budget SIZE` caps the bitrate to a file size. `geneva targets`
  prints the table with the source and date of each platform's numbers.
- Encode block fields `keyframe_interval`, `max_bitrate_kbps`, `level`
  and `fast_start`, wired through every encoder.
- CSS shorthand strings where an object also is: `shadow` as
  `"0 2px 8px #0008"`, `outline` as `"2px black"`, `padding` as `"8px"`,
  and the `font` shorthand `"italic 600 40px/1.2 Inter"`, whose parts fill
  in size, weight, italic and line height. Same structures underneath;
  the object form stays canonical. Still format 0.1.

### Changes

- MP4, MOV and M4A files carry their index at the front by default
  (`encode.fast_start`), for copied and encoded outputs alike.

## 0.1.9

### Added

- `geneva subtitles --burn FILE` draws the cues of an SRT or WebVTT file
  into the picture: `--position` (bottom, top, center), `--margin`, and
  `--style` as a JSON object of text fields over a default look sized to
  the frame. Overlapping cues go on further layers. Every cue is laid out
  before rendering; one that leaves the frame is warning W403, one
  outside the title-safe area (`--safe`, 5% by default) is note N404.
  `--fit` shrinks such cues in steps until they fit, down to half size,
  and reports each one (N405).
- A word longer than a text line now breaks inside the word instead of
  running past the edge, in every text source.

### Fixes

- Overlays on an untouched video (burned-in subtitles, a logo, a lower
  third) no longer send every frame through the compositor. The decoded
  frames go straight to the encoder; only the overlays are drawn, onto
  a transparent frame the size of their box, and laid over the decoded
  planes where they have coverage. Pixels outside stay byte-identical.
  Text that does not change with time is laid out once per clip. Burning
  three short cues into 70 s of SD video on four cores: 17.3 s to 8.3 s,
  against ffmpeg's 7.7 s with the same x264 settings.
- Image sequences (PNG) from an 8-bit YCbCr source take the direct path:
  the scaler applies the source's matrix and a table re-encodes its
  transfer curve, instead of the compositor's floating-point conversion.
  One second of 1080p to PNG went from 1.6 s to ffmpeg's speed.

## 0.1.8

### Changes

- Software H.264 uses the system's x264 when its library is installed
  (`libx264.so.NNN` from the distribution, Homebrew's dylib on macOS),
  ahead of the bundled OpenH264 and after any hardware encoder. Nothing of
  x264 is in the binary; it is loaded at run time through the parts of
  its interface that are the same across builds 155 to 175. The notes of
  a render say which encoder was used. `GENEVA_X264=off` turns the lookup
  off; a path names the library. On four cores, a 70 s SD resize: 5.7 s
  and 5.1 MB with x264 against 6.5 s and 4.7 MB for ffmpeg's libx264,
  where OpenH264 wrote 11 MB.

## 0.1.7

### Fixes

- `geneva audio --extract` re-encoded the audio of files at fractional
  frame rates (29.97, 59.94) instead of copying it: the verb's compiled
  timeline printed the output duration with six decimals, which no longer
  matched the clip's exact length. The extract now takes its length from
  the clip, and the copy planner accepts a duration within a millisecond
  of the clip's, so hand-written timelines get the same treatment.
- The VideoToolbox H.264/H.265 encoders ignored the quality setting and
  ran at their default bitrate, writing files up to four times larger
  than ffmpeg at the same `-q:v`; the quality is now passed the way
  ffmpeg stores it.

## 0.1.6

### Fixes

- The audio track was mixed and encoded after the last video frame, which
  added about a second per minute of AAC to every render; it is now
  encoded on its own thread while the frames flow, and the encoder thread
  interleaves the packets. A 70 s SD resize on four cores went from
  6.7 s to 4.8 s.
- `check.sh` shows whether a "copied" step really copied ("ok, copied" or
  "ok, RE-ENCODED") and the size of ffmpeg's output next to geneva's.

## 0.1.5

### Changes

- Last release with an Intel macOS archive. Apple stopped selling Intel Macs in 2023
  and macOS 26 is the last release for them; the build was untested and
  gated every release on the slowest runner. Intel Macs build from source.

### Fixes

- macOS archives were built against media libraries from before MXF and
  PNG support, because the build cache kept the media bindings compiled
  earlier; DNxHR, MXF and image sequences failed on the Mac with "not
  known" and "not available" errors. The cache is now keyed to the media
  libraries and the bindings are rebuilt whenever the libraries are.
- VP9 on Apple silicon and Intel Macs ran several times slower than
  ffmpeg: libvpx's configure does not recognize macOS 15 and fell back to
  a build without NEON or SSE. The target is now spelled out.
- Verbs keep the source's color encoding (see `docs/cli.md`), so resizing
  or converting standard-definition material no longer converts it to
  BT.709 through the compositor; it takes the direct path, tagged as the
  source was.
- A resize whose even output size is up to two pixels off the exact fit
  (854×480 at height 360 is 640.5 wide, written as 642) takes the direct
  path as well.

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
