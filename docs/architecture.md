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

`Renderer::render_frame(&Composition, Ratio) -> Frame` is the whole
contract. A `Frame` is premultiplied linear-light RGBA; conversion to 8-bit
sRGB happens at the edge.

`CpuRenderer` is the reference implementation:

- For each clip visible at the requested time, it samples the clip's tracks
  at clip-local time and builds an affine placement (fit → anchor → scale →
  rotation → position).
- Output pixels inside the placement's bounding box are mapped back into the
  clip's box; coverage and color come from a 2×2 supersample, except when the
  placement is pixel-aligned, in which case one center sample keeps images
  bit-exact.
- The result is scaled by opacity (and by the crossfade ramp when a
  transition is active) and composited with the clip's blend mode.

Assets reach the renderer through the `AssetSource` trait. The file
implementation resolves `assets.<id>.src` under one root directory; the
validator has already rejected absolute paths and `..`, so the root is a
real boundary.

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

The following are planned and have their seams in place:

- **Media I/O.** A backend trait for demux, decode and encode with an
  implementation over libav, isolated in its own crate. `ResolvedSource::Video`
  and the `Encode` settings already carry what it needs.
- **Text layout.** Shaping and layout for `TextSource` producing glyph runs
  the renderer can paint.
- **GPU renderer.** A second `Renderer` implementation with the same
  contract, validated against the CPU renderer by the golden harness.
- **Rendering whole outputs.** Iterating `Composition::frame_time` over
  `frame_count` and feeding an encoder, plus audio mixing.
