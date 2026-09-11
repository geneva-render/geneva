# Architecture

Geneva is a Rust workspace with one crate per concern. Data flows in one
direction:

```text
timeline JSON ──parse──▶ Timeline ──resolve──▶ Composition ──render──▶ Frame
                (serde)     (document)   (validation)   (exact times,     (linear f32,
                                                          tracks)           premultiplied)
```

## Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `geneva-anim` | Easing curves and keyframe tracks. No I/O, no clocks. | serde |
| `geneva-color` | Tags, inference, transfer functions, matrices, the `LinearRgba` working format and CSS color parsing. | geneva-anim |
| `geneva-timeline` | The format: document types (also the JSON Schema source), exact `Ratio` time, parsing with path-precise errors, resolution into `Composition`, diagnostics. | geneva-anim, geneva-color |
| `geneva-render` | The `Renderer` trait, `Frame`, asset loading, and `CpuRenderer`. | geneva-timeline, geneva-color |
| `geneva-golden` | Perceptual comparison (per-channel epsilon, PSNR, SSIM), diff images, and the golden case runner. | geneva-render |
| `geneva-media` | Probe, decode, encode and audio mixing over the bundled media libraries, behind the `media` feature; the only crate that knows about containers and codecs. | geneva-render, geneva-color |
| `geneva-cli` | The `geneva` binary. | everything above |

Lower crates never depend on higher ones. `geneva-timeline` knows nothing
about pixels; `geneva-render` knows nothing about JSON.

## Timeline

`Timeline` is a plain serde model with `deny_unknown_fields` on every
struct. Doc comments on its fields are the JSON Schema descriptions, so the
schema, the parser and the reference documentation cannot drift.

Parsing uses `serde_path_to_error` so a structural error carries the JSON
pointer of the failing element. `Animated<T>` has a hand-written
deserializer that keeps that path precise inside keyframe lists.

`resolve` is a single pass that performs all semantic validation while
building the `Composition`. Times become `Ratio` values (exact rationals),
sequential clips get absolute starts, open-ended clips are closed against
the output duration, lengths become pixels, and every animated property
becomes a `Track` that can be sampled at any time. The composition is only
returned when there are no errors, so a renderer never sees an inconsistent
document.

## Rendering

`Renderer::render_into(&Composition, Ratio, &mut Frame)` is the whole
contract (`render_frame` is the allocating convenience). A `Frame` is
premultiplied linear-light RGBA; conversion to 8-bit sRGB or to the
encoder's Y'CbCr happens at the edge.

`CpuRenderer` is the reference implementation:

- For each clip visible at the requested time, it samples the clip's tracks
  at clip-local time and builds an affine placement (fit → anchor → scale →
  rotation → position).
- Output pixels inside the placement's bounding box are mapped back into the
  clip's box; coverage and color come from a 2×2 supersample, except when the
  placement is pixel-aligned, in which case one center sample keeps images
  bit-exact. An image that is only moved and scaled is resampled span by
  span: well inside the picture, a magnified or unit-scale image takes one
  bilinear sample at the pixel center (the usual resampling), a minified one
  keeps the 2×2 supersample so that detail is filtered rather than dropped,
  and the pixels near the picture's edges keep the general path for their
  coverage. Nested compositions draw from their own frame's buffer, which
  is kept for the next one instead of being allocated each frame.
  `cargo run --release -p geneva-cli --example profile` times these paths.
- The result is scaled by opacity (and by the crossfade ramp when a
  transition is active) and composited with the clip's blend mode.
- Rows are independent, so drawing, the decoder's 16-bit-to-linear
  conversion and the packing to 4:2:0 run row-parallel on a thread pool.
  Determinism is unaffected: every pixel depends only on its inputs, never
  on the order rows finish.

`geneva render` keeps the encoder on its own thread with a short queue of
converted frames, so decoding, compositing and encoding overlap.

Frames leave the renderer in whichever sample layout the codec takes:
8-bit 4:2:0 for the distribution codecs, 8-bit or 10-bit 4:2:2 and 10-bit
4:4:4 for the intermediate codecs, RGBA for PNG. The packer quantizes from
the float working space directly to the target depth, so a 10-bit output
is not an 8-bit one widened.

Subtitle tracks never touch the picture. Their files are parsed into cues
and written as text packets on their own streams, interleaved with the
frames as the output reaches each cue's start time, in whichever text
codec the container uses.

Assets reach the renderer through the `AssetSource` trait: images, font
bytes, and video frames by source time. The file implementation resolves
`assets.<id>.src` under one root directory; the validator has already
rejected absolute paths and `..`, so the root is a real boundary. The
media crate's implementation adds video, keeping one decoder open per asset
so sequential frames decode once.

Text is laid out by `cosmic-text` (shaping, bidi, line breaking, fallback)
and rasterized by `swash` into coverage masks. The engine composites the
masks in linear light, draws the background box, outline and shadow, and
hands the result to the same placement code as images.

## Stream copy

Before rendering, `geneva render` checks whether the composition is one
layer of video clips shown as they are: natural size at the frame center,
full opacity, no rotation, no transitions, nothing else on top, output
size and rate equal to the sources', the same codec in every source with
identical coded parameters, and audio that is either absent, the sources'
own, or one untouched track spanning the output. When that holds and no
quality setting asks for a re-encode, the coded packets are copied into
the output instead. Cuts move back to the keyframe at or before the
requested time and the command reports the times used; `--exact` forces
decoding and encoding for frame-accurate cuts. An audio-only output copies
its one track under the same rule.

## Direct transcode

When the same analysis finds the picture untouched but the streams cannot
be copied (a different codec was asked for, a quality setting was given,
or `--exact`), the decoded frames are handed to the encoder as they come
out of the decoder, provided they carry the output's color tags. Frames
already in the encoder's sample layout at the output size are copied as
they are; frames that differ only in size (a resize that fills the whole
frame, to within the two pixels that rounding to an even size can leave) or in layout (8-bit 4:2:0 into the 10-bit 4:2:2 that ProRes and
DNxHR take) are scaled and repacked by libswscale in their coded YCbCr
encoding, bicubic and on several threads, the way a plain transcode does
it. A picture that does not cover the frame (a `contain` fit with bars,
as a landscape video on a portrait canvas) is served the same way when
the background is one opaque color: the frame is cleared to that color
in the encoder's layout once, and each picture is scaled to its place,
rounded to even pixels so that subsampled chroma lines up. A `cover` fit
that crops is served too: the region of the decoded frame that the output
shows (a view sharing the frame's buffers) is scaled to the whole frame.
This is the one
place Geneva resamples outside linear light: with a single picture and
nothing composited over it there is no blending to get wrong, and the
difference from the reference renderer is the difference between
gamma-space and linear-light filtering at hard edges, which the tests
bound. An RGB output (an image sequence) from an 8-bit YCbCr source is
served the same way: the scaler applies the source's matrix and range and
a per-channel table re-encodes its transfer curve as the output's, which
is what the compositor computes for such a frame. RGB sources and
conversions between YCbCr encodings still go through the compositor,
which owns matrix and primaries conversions.

Layers above such a video do not take the picture off this path either.
When every clip above the first layer composites normally, the renderer
draws only those clips, onto a transparent frame the size of their
bounding box, and the result is laid over the decoded planes in the
encoder's own layout: each covered pixel is converted to linear light,
composited, and converted back; pixels the overlays do not touch stay
byte-identical, so there is no seam between frames with and without a
caption, and a burned-in subtitle costs only its own box. Text that does
not change with time is laid out once per clip. Frame selection
follows the same rule as the renderer (the last decoded frame at or
before each output time), and the report names this mode `direct`.

## Smart cut

Between stream copy and the direct path sits a third: when the picture
is an H.264 source shown as it is but something changes somewhere (an
exact cut inside a group of pictures, an overlay or a burned-in
subtitle for part of the time), the planner copies the source's packets
for every stretch that stays untouched and encodes only the rest, into
the same track. A copied stretch starts at an IDR picture and ends
where the source's decode order is clean, that is, where every picture
before the boundary is shown before every picture after it, so no
picture is left without its references; the pictures in between that
the cut wants are decoded and encoded from the previous keyframe
instead. The encoded runs come from the system's x264 with its
parameter sets under an id the source does not use, and both sets go
into the container header side by side, so each picture names its own
and the decoder switches at the IDR that starts every run and every
stretch. Decode timestamps are assigned in one sequence across copied
and encoded pictures, running a fixed number of pictures ahead of
display (the larger of the source's reorder depth and the encoder's),
which keeps every picture decodable in time without touching the
pictures themselves. The report names the mode `smart` and counts the
frames copied and encoded; the copied frames are the source's bytes.
The planner indexes only a window of each source, from the keyframe
before the first wanted picture to the IDR after the last, so a short
cut of a long file reads a few groups of pictures, not the file.

The clips' own audio rides along as coded when every clip has one of
the same kind the container takes. The output's audio is one packet
grid: every slot holds exactly one copied packet, so the track is
gapless and never overlaps itself, and a player that runs packets back
to back keeps time with the video whatever the joins. The first packet
is the one that straddles the start, placed early by the part before
the cut (a negative start the container turns into an edit, so the
sound begins on the sample); each later segment starts with the packet
nearest to its cut, so a join is off by at most half a packet and the
error never accumulates. Anything else (a separate audio track, gain,
fades, clips of different kinds) mixes and encodes the audio as usual.

## Verbs

`trim`, `concat`, `convert`, `resize`, `overlay` and `audio` do not have
code paths of their own. Each probes its inputs, builds a timeline
document (`crates/geneva-cli/src/verbs.rs`), and hands it to the same
load, validate, plan and render sequence a timeline file goes through, so
stream copy, diagnostics and the JSON report behave identically.
`--show-timeline` prints the document; asset paths in it are relative to
the deepest directory containing every input.

## Media

The media libraries are built from pinned sources by
`scripts/build-media-libs.sh` with only the demuxers, muxers, decoders and
encoders Geneva supports, and linked statically. Every component is LGPL
or BSD licensed; see `THIRD-PARTY-NOTICES.md`. Software H.264 encoding is
OpenH264 at a constant quantizer, unless the system has an x264 library:
that is loaded at run time (`codecs/x264.rs`, through the parts of its
interface that have stayed the same across builds) and preferred, with
nothing of x264 in the binary. Hardware encoders (NVIDIA on Linux,
VideoToolbox on macOS) are tried first when the encode settings allow.

Decoded frames are converted by the scaler to 16-bit 4:4:4 (or RGBA for
R'G'B' sources) and then, in Rust, through range normalization, the
tagged matrix and the tagged transfer function into the compositing
format. Encoding goes the other way: linear to the output transfer, to
Y'CbCr, 2×2 chroma averaging, 8-bit 4:2:0, then the encoder. Color tags
travel with the stream in both directions. Audio is decoded and resampled
to stereo `f32` at the output rate, shaped by gain tracks and fades, and
summed.

## Golden tests

`tests/golden/<case>/` holds `scene.json`, `golden.json` (frame times and
optional tolerance) and `expected/<time>.png`. The harness renders each
frame, compares with per-channel epsilon, mismatch fraction, PSNR and SSIM,
and writes `<time>.actual.png` and `<time>.diff.png` under
`target/golden-failures/` when a frame fails. `GENEVA_UPDATE_GOLDEN=1`
rewrites the references.

Tolerances are perceptual on purpose: different GPUs round shader math
differently, and the harness must accept those differences while catching
real regressions.

## Not here yet

- **GPU renderer.** A second `Renderer` implementation with the same
  contract, validated against the CPU renderer by the golden harness.
- **Software H.265 encoding.** Only hardware encoders are available for it.
- **Streaming audio mixing.** The mixer currently holds the whole mix in
  memory.
