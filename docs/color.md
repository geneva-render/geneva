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
2. Luminance goes through the BT.2390 EETF from the source's peak (the
   file's mastering metadata, MaxCLL or the mastering display's maximum;
   1000 nits when the file has none) down to reference white: a
   Hermite-spline knee in the PQ domain. The three channels are scaled
   by the same ratio, so hue holds.
3. Primaries are converted to BT.709.
4. What still leaves the SDR cube is pulled toward its own luminance
   until it fits, so a bright saturated highlight desaturates rather
   than shifting hue.

The curve is the broadcast reference and what ffmpeg's tone-mapper
calls `bt2390`. It is deterministic and looks at no other pixel. Its
character: the source's peak lands on SDR white, and a 1000-nit source's
reference white lands at about 78% of it, so an HDR frame tone-mapped
this way reads slightly darker than an SDR grade of the same scene, as
the standard prescribes. Still images tagged `pq` or `hlg` on their
asset (16-bit PNGs, for instance) take the same path.

## Output

The frame is converted from linear light to the output transfer, then to
Y'CbCr with the output matrix and range, and the file is tagged with what
was actually written. The default is BT.709 SDR, limited range. PNG frames
from `geneva frame` are sRGB with straight alpha.

HDR output (`pq` or `hlg` transfer) is rejected with E420 in this version.
HDR input is decoded to linear values above 1.0 and will be tone-mapped once
that path is implemented; it is never silently clipped by the color
conversion itself.

## Verification

`geneva-color` carries numeric tests for every conversion: reference points
on each curve, round trips, matrix anchors (white has zero chroma, primaries
hit the chroma extremes), 8- and 10-bit range anchors, and the linear-light
blending result above. The golden-frame tests in `tests/golden/` then check
the assembled pipeline on rendered scenes.
