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
