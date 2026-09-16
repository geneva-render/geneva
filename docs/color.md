# Color pipeline

Decoders deliver coded pixels plus metadata tags that files often omit or
get wrong. Everything after that point is the engine's responsibility. This
page describes what the engine does with color, in the order pixels travel.

## Working space

Compositing happens in **linear light, BT.709 primaries, premultiplied
alpha, 32-bit float per channel**. Every source is converted into this space
before it is blended, and the result is converted back once for output.
Blending encoded values (the common shortcut) makes semi-transparent edges
and fades too dark; a 50% white over black is code 188 in linear light, not
128.

The one exception is inside a markup box: an `html` source blends the way a
browser does, on sRGB-encoded values, and the finished box is converted to
linear light once before it is composited like any other clip. See the
gradients section of [timeline.md](timeline.md).

## Tags and inference

Four tags describe a source: `primaries`, `transfer`, `matrix` and `range`.
Assets may override them in the timeline (`assets.<id>.color`) for files
that are untagged or mistagged. Missing tags are filled by inference:

| Situation | Assumption |
| --- | --- |
| Untagged Y'CbCr at 1280×720 or larger | BT.709 matrix and primaries |
| Untagged Y'CbCr below that | BT.601 matrix, BT.601 (625-line) primaries |
| Untagged Y'CbCr | BT.709 transfer, limited range |
| R'G'B' (matrix identity), e.g. PNG and JPEG | sRGB transfer, BT.709 primaries, full range |

Present tags are always kept, even when they look unusual. Inferences are
reported so a wrong guess is visible.

## Decoding

1. Integer codes are normalized with the tagged range: limited range maps
   luma 16–235 and chroma 16–240 (scaled by bit depth) to the nominal range;
   full range uses all codes.
2. Y'CbCr becomes R'G'B' through the tagged matrix (BT.709, BT.601 or
   BT.2020 non-constant-luminance coefficients).
3. R'G'B' is linearized with the tagged transfer function.

## Transfer functions

| Tag | Curve |
| --- | --- |
| `srgb` | IEC 61966-2-1 piecewise curve |
| `bt709` | Inverse of the BT.709 OETF (scene-referred), using the exact BT.2020 constants so the two branches meet |
| `linear` | identity |
| `pq` | SMPTE ST 2084, scaled so 100 nits is 1.0 |
| `hlg` | ARIB STD-B67 inverse OETF, scaled so nominal peak is 10.0 |

SDR video is linearized with the inverse BT.709 OETF rather than the BT.1886
display curve. The two are close, and the scene-referred choice keeps sRGB
graphics and BT.709 video consistent: a color written as `#ff8800` in a
timeline comes back as `#ff8800` in the output when nothing is blended over
it.

## Wide gamut and HDR sources

Material whose primaries are not BT.709 (BT.2020 as a rule, the BT.601
sets for standard definition) is converted into the working space in
linear light, so its colors keep their meaning; wide-gamut colors can
land outside `[0, 1]` and are clipped by the output encoder.

PQ and HLG material is tone-mapped on the way in, per pixel, so that
highlights roll off instead of clipping at white:

1. The signal becomes light. PQ is absolute; its values are taken
   relative to HDR reference white, 203 nits (BT.2408), which is `1.0`
   in the working space. HLG is scene-referred; the OOTF of a 1000-nit
   display (system gamma 1.2 on luminance) makes it display light first.
2. A PQ source mastered above 1000 nits (per the file's metadata,
   MaxCLL or the mastering display's maximum) is first brought down to
   1000 nits with the BT.2390 EETF on luminance, a Hermite-spline knee
   in the PQ domain, so that the next step sees the range it is
   specified for.
3. ITU-R BT.2446 method A, the conversion specified for 1000-nit HDR to
   SDR: luminance is encoded with a gamma of 2.4, compressed through a
   log curve and a three-piece knee, and decoded for a 100-nit display;
   the chroma follows with the same scale, slightly reduced, and a small
   luminance correction keeps saturated reds from darkening.
4. Primaries are converted to BT.709.
5. What still leaves the SDR cube is pulled toward its own luminance
   until it fits, so a bright saturated highlight desaturates rather
   than shifting hue.

The curve is the broadcast reference for footage (libplacebo calls it
`bt.2446a`). It is deterministic and looks at no other pixel. Its
character: the source's 1000 nits land on SDR white, reference white
(203 nits) at 0.41 of it in linear light, and the range between keeps
its texture: sunlit sand, skies and skin stay graded rather than
clipping to white. An HDR frame converted this way reads a little
darker in the mid-tones than the HDR original on an HDR screen, as the
recommendation intends. Still images tagged `pq` or `hlg` on their
asset (16-bit PNGs, for instance) take the same path.

## Output

The frame is converted from linear light to the output transfer, then to
Y'CbCr with the output matrix and range, and the file is tagged with what
was actually written. The default is BT.709 SDR, limited range. PNG frames
from `geneva frame` are sRGB with straight alpha.

### HDR output

`output.color` may be `pq` or `hlg` with BT.2020 primaries, on a codec
that carries ten bits: `h265` (a hardware encoder, VideoToolbox or NVENC),
`av1`, `vp9` (profile 2) or `prores`. H.264 through x264 is eight bits and
is refused (E420). In an HDR composition the working space still has
BT.709 primaries and `1.0` is reference white (203 nits): graphics and
text land there, as BT.2408 recommends, and HDR sources keep their range
instead of being tone-mapped, so their highlights pass through. On the way
out the frame is converted to BT.2020 and encoded with the output curve.
PQ outputs carry static HDR10 metadata: the first HDR source's, or
standard defaults (a P3-D65 mastering display of 1000 nits, MaxCLL 1000,
MaxFALL 400), in the container and in the stream where the encoder writes
it. HLG needs none. The verbs take `--keep-hdr` to keep HDR sources HDR.

## Verification

`geneva-color` carries numeric tests for every conversion: reference points
on each curve, round trips, matrix anchors (white has zero chroma, primaries
hit the chroma extremes), 8- and 10-bit range anchors, and the linear-light
blending result above. The golden-frame tests in `tests/golden/` then check
the assembled pipeline on rendered scenes.
