# Architecture

A Rust workspace, one crate per concern, data flowing one way:

```text
JSON ──parse──▶ Timeline ──resolve──▶ Composition ──plan──▶ copy | smart cut | direct | composite ──▶ encoder
       serde,     document    validation,   exact Ratio      per output
       JSON-path              tracks        times
```

## Crates

| Crate | Role |
| --- | --- |
| `geneva-anim` | Easing and keyframe tracks. No I/O, no clock. |
| `geneva-color` | Colour tags, inference, transfer functions, matrices, `LinearRgba`, CSS colour parsing. |
| `geneva-audio` | BS.1770-4 loudness, true peak, limiter, high-pass and hum notches, on interleaved `f32`. No I/O. |
| `geneva-html` | HTML and CSS subset: parser, cascade, block and flex layout (taffy), display list. |
| `geneva-timeline` | Document types (the JSON Schema source), `Ratio` time, parsing, resolution into `Composition`, diagnostics. Markup is checked here, so its errors carry a path. |
| `geneva-render` | `Renderer` trait, `Frame`, `AssetSource`, `CpuRenderer`, text engine, markup painter. |
| `geneva-gpu` | `GpuRenderer` on wgpu (Vulkan, Metal, DirectX 12). Feature `gpu`, on by default. |
| `geneva-media` | Probe, decode, encode, mux, stream copy, smart cut, chunking, audio mix, over the static media libraries. Feature `media`. The only crate that knows containers and codecs. |
| `geneva-media-link` | Link directives for the static media libraries; a build dependency of the crates above. |
| `geneva-golden` | Perceptual image and audio comparison, golden case runner. |
| `geneva-cli` | The `geneva` binary: commands, verbs, `--for` targets, and the choice of output path. |

No crate depends on one above it. `geneva-timeline` knows nothing of
pixels; `geneva-render` nothing of JSON.

## Timeline

- Serde model with `deny_unknown_fields` on every struct. Field doc
  comments are the schema descriptions.
- `serde_path_to_error` gives structural errors a JSON pointer.
  `Animated<T>` has its own deserializer so paths stay exact inside
  keyframe lists; the object form of a style is flattened, so its errors
  point at the enclosing source.
- `resolve` is one pass: validation, absolute clip times as `Ratio`,
  open-ended clips closed against the output duration, lengths to
  pixels, every animated property to a sampleable `Track` (a clip's CSS
  `animation`, or the one on its markup's outermost element, becomes
  tracks too), `captions` expanded into one text clip per cue. It returns a `Composition` only
  when there is no error.

## Frames and renderers

`Renderer::render_into(&Composition, Ratio, &mut Frame)` is the contract.
A `Frame` is premultiplied linear-light `f32` RGBA; conversion to sRGB or
Y'CbCr happens at the edges.

**CPU (`CpuRenderer`)**, the reference and the fallback:

- Per visible clip: sample tracks at clip time, build the affine
  placement (fit, anchor, scale, rotation, position), map output pixels
  back into the clip's box.
- Coverage by 2×2 supersampling; one centre sample when the placement is
  pixel-aligned, so images stay bit-exact. A moved and scaled image is
  resampled per span: bilinear when magnified or at unit scale, 2×2 when
  minified, the general path near edges.
- Opacity and transition ramps, then the blend mode, in linear light.
- Rows are independent and run on a thread pool, as do decode
  conversion and packing. Output does not depend on scheduling.
- Nested compositions render into a buffer kept between frames.

**GPU (`GpuRenderer`)**: the same contract, transcribed function by
function into WGSL (`composite`, `blur`, `markup`, `pack`); placement,
transition logic and the painter are shared Rust.

- Working format `Rgba16Float`; blur layers `Rgba32Float`.
- Unchanging pictures (image assets, static text, still markup) cached
  on the device under 96 MiB, least recently drawn out first.
- Output packed on the device per plane into integer textures and read
  through a ring of 3 staging buffers: a 1080p 4:2:0 frame reads back as
  3 MB, and frame n+1 is submitted before frame n is mapped.
- Blend modes read a copy of the target taken before the blending draw.
- Held to the CPU by the golden cases: within one 8-bit code, except the
  blur (up to three codes on 0.12% of pixels, the same on Metal and
  Vulkan, none on lavapipe).
- Not on the device: 16-bit and HDR sources (converted by the decoder
  and uploaded as pictures), and markup painting (below).

`--renderer auto` takes a hardware device when there is one; `gpu` takes
a software one too; both fall back to the CPU with a note. `frame` and
the overlays on a direct-path picture always use the CPU.

**Text**: shaping, bidi, line breaking and fallback by `cosmic-text`,
rasterized by `swash` into coverage masks, composited in linear light
with box, outline and shadow, placed like an image.

**Markup**: parsed and styled once per clip by `geneva-html`, laid out
and painted by the renderer into a picture placed like an image.

- Painted in sRGB-encoded premultiplied values, as a browser does; the
  finished box is converted to linear light once.
- A box with nothing animated inside is painted once per clip.
- An element with an animation, opacity, filter, blend mode or clip is a
  group: a buffer of its own, bounded to what its parent can show
  (through the inverse transform, padded for blur, at most nine frames of
  area), composited with those applied. Groups off the frame are
  skipped. An animation that changes a size re-lays out each frame.
- On the GPU, boxes, glyphs, shadows and polygon coverage are still
  painted on the CPU and uploaded; groups are composited on the device.

## Output paths

The planner picks the cheapest path that reproduces the composition,
per output. The report names the mode.

### Stream copy

One layer of video clips shown as they are (natural size, centred, full
opacity, no rotation, transitions or overlays), output size and rate
equal to the sources', one codec with identical coded parameters, and
audio absent, the sources' own, or one untouched track. No setting asks
for a re-encode (quality, bitrate, keyframes, or audio that differs
from the source in codec, rate or channels). Packets are copied; cuts
move back to the keyframe at or before the requested time and the
report gives the times used. An audio-only output copies its one track
the same way.

When only the sound must change (a loudness target, hygiene, another
audio format), the video is copied and the mix encoded beside it
(`copy-picture`).

### Smart cut

H.264 source shown as it is, but something changes for part of the
time: an exact cut inside a GOP, an overlay, a burned-in cue. Untouched
stretches are copied as packets; the rest is encoded by the system's
x264 into the same track.

- A copied stretch starts on an IDR and ends where decode order is
  clean (every earlier picture displays before every later one).
- The encoded runs use a parameter set id the source does not use; both
  sets are in the container header and each run starts with an IDR.
- DTS is one sequence across copied and encoded pictures, running ahead
  of display by the larger of the two reorder depths.
- Only a window of each source is indexed: keyframe before the first
  wanted picture to the IDR after the last.
- Audio is copied on a packet grid (one packet per slot, the first one
  straddling the cut, placed early through an edit), so joins are off by
  at most half a packet and errors do not accumulate. Anything else
  mixes and encodes.

### Direct transcode

Picture untouched but not copyable (another codec, a quality setting,
`--exact`), and the source already carries the output's colour tags.

- Frames in the encoder's layout at the output size go to it unchanged.
- A size change, a crop (`cover`), bars on one opaque background, or a
  layout change (8-bit 4:2:0 to 10-bit 4:2:2) is done by libswscale in
  the coded Y'CbCr, bicubic, threaded. This is the one place geneva
  resamples outside linear light; with nothing composited there is no
  blend to get wrong.
- Overlays on higher layers keep the path: they are rendered on a
  transparent frame the size of their bounding box and laid over the
  decoded planes, converting only covered pixels to linear light and
  back. Untouched pixels stay byte-identical.
- RGB outputs from 8-bit Y'CbCr sources go through the scaler with the
  source matrix and range and a per-channel transfer table. Other
  matrix or primaries conversions go through the compositor.

### Composite

Everything else: decode to 16-bit 4:4:4 (RGBA for R'G'B' sources),
normalize range, apply the tagged matrix and transfer into the working
format, render, then linear to the output transfer, to Y'CbCr, 2×2
chroma average and quantize straight to the target depth (a 10-bit
output is not a widened 8-bit one). A video drawn at half its size or less
is fetched shrunk by a whole factor per axis (`AssetSource::video_frame_shrunk`,
libswscale widening and shrinking in one pass) and its placement carried to
the smaller texels (`Placement::in_texels`); the remaining reduction is
resampled in linear light. The encoder runs on its own thread
behind a short queue, so compositing and encoding overlap.

### Chunked encoding

An encode is split into stretches encoded at once, each with its own
decoders, compositor and encoder on a share of the cores, then joined by
the stream-copy join `concat` uses; the first stretch also encodes the
whole audio. Boundaries are clip starts, or the keyframe grid within a
quarter of a stretch, otherwise one extra keyframe. Stretches are at
least 2 s, at most 16.

`auto` splits into `cores / 2` stretches for VP9 and for OpenH264 (H.264
without x264 or a hardware encoder), the two measured to gain (a fifth
and a tenth on four cores), and never for the rest: AV1 gained nothing,
x264 and DNxHD lost, and VideoToolbox sessions contend. Not used with a
bitrate ceiling (the buffer cannot restart at a boundary), a smart cut,
or an image sequence.

### Multi-output render

With an `outputs` map, one pass: each frame is composited once,
converted once per distinct colour encoding and layout, scaled per
rendition by `PlaneScaler` (threaded swscale) and sent over a bounded
channel to that rendition's encoder thread, which returns the buffers.
Audio entries mix on their own threads. A canvas-size video entry with
no encode settings of its own is offered to the copy planner first. The
poster is the composited frame at its time, or the first frame past the
opening that is not dark and follows motion (a 16×9 luma gist per
frame); sprite tiles are scaled from the composited frames. With no
video entry only the frames the pictures need are composited.

## Verbs

`convert`, `resize`, `trim`, `concat`, `overlay`, `audio`, `subtitles`
and `frame` on a video file probe their inputs, build a timeline
(`crates/geneva-cli/src/verbs.rs`) and go through the same load,
validate, plan and render sequence as a document. `--show-timeline`
prints it. Every verb writes the source's own sample rate and channel
count into the document, so the planner compares requested audio with
the source rather than treating a set field as a change.

## Media

- The media libraries are built from pinned sources by
  `scripts/build-media-libs.sh` with only the components geneva uses,
  and linked statically. LGPL or BSD; see `THIRD-PARTY-NOTICES.md`.
- H.264 in software: the system's x264 when installed, loaded at run
  time (`codecs/x264.rs`, through the stable part of its interface; no
  x264 code in the binary), otherwise the bundled OpenH264. Hardware
  encoders (VideoToolbox on macOS, NVENC on Linux and Windows, Media
  Foundation on Windows, 8-bit only and only with a GPU behind it) first
  when the settings allow. No software H.265.
- Rate control is constant quality with an optional VBV ceiling
  (bufsize twice the rate); VP9 takes the ceiling as its constrained
  quality bitrate. VideoToolbox takes a ceiling only in bitrate mode,
  NVENC at a constant QP not at all; both are reported.
- Audio is decoded and resampled to stereo `f32` at the output rate,
  shaped by gain tracks and fades, summed, then treated if asked
  (high-pass and notches, gain to a loudness target, true-peak limiter),
  a block at a time with one decoder and resampler per voice. What is
  measured first is measured on a pass before the blocks go out.
- Subtitle tracks never touch the picture: cues become text packets on
  their own stream in the container's text codec, interleaved by time.
- `AssetSource` resolves `assets.<id>.src` under one root; validation
  has already rejected absolute paths and `..`. One decoder is kept open
  per asset so sequential frames decode once.

## Tests

- **Golden frames**: `tests/golden/<case>/` holds `scene.json`,
  `golden.json` (times, tolerance) and `expected/<time>.png`. Compared
  by per-channel epsilon, mismatch fraction, PSNR and SSIM, on both
  renderers; failures write `.actual.png` and `.diff.png` under
  `target/golden-failures/`. `GENEVA_UPDATE_GOLDEN=1` rewrites.
- **A/V sync corpus**: `tests/media/sync/`, twelve files from
  `scripts/make-sync-corpus.sh`, each a flash at 1.0 and 2.0 s and a
  1 kHz tone from the same times, each with one trap (edit lists,
  negative composition offsets, VFR, streams starting at 10 s, audio
  starting 0.5 s late, 29.97 and 23.976 fps, 44.1 kHz, Opus in WebM,
  MPEG-TS). Every path must keep flash and tone to the frame and within
  3 ms. Time zero is a file's first video frame, for picture and sound;
  MPEG-TS AAC starts 21 ms late, as in ffmpeg, since TS carries no
  priming.

## Vulkan in a container

A CUDA image has NVIDIA's compute libraries, not the graphics half, so
Vulkan finds no device and the renderer falls back. The Vulkan driver
(`libGLX_nvidia.so.0`) needs the X11 client and GLVND libraries:

```sh
apt-get install -y libx11-6 libxext6 libglvnd0 libegl1 libgl1 libgles2
# and start the container with NVIDIA_DRIVER_CAPABILITIES=all
```

The probe's note names what is missing. NVENC uses the compute half and
is unaffected.
