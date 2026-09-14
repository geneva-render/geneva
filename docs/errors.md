# Diagnostics

Every diagnostic has a stable code, a severity, a JSON pointer to the
location in the timeline, a one-sentence message and, where it applies, the
offending value and a suggested fix. Human output looks like:

```text
error[E302]: clip 1 of overlay starts at 1s, before clip 0 of overlay ends at 2s
  --> /layers/1/clips/1/start = "1s"
   = help: start it at 2s or later, move it to another layer, or add a transition to blend the overlap
```

With `--format json` the same diagnostic is:

```json
{
  "severity": "error",
  "code": "E302",
  "path": "/layers/1/clips/1/start",
  "message": "clip 1 of overlay starts at 1s, before clip 0 of overlay ends at 2s",
  "value": "1s",
  "help": "start it at 2s or later, move it to another layer, or add a transition to blend the overlap"
}
```

Errors (`E`) stop rendering. Warnings (`W`) do not, but usually mean the
result is not what was intended. Notes are informational.

## Structure (E100–E199)

| Code | Meaning |
| --- | --- |
| E100 | The file is not valid JSON. The location points at the syntax error. |
| E101 | Unknown field. The message lists the fields allowed at that location. |
| E102 | Required field missing, including the `end` of the last timed word, which has no next word to take it from. |
| E103 | Wrong type or invalid value for a field, including malformed times, lengths and colors. |
| E104 | Duplicate field. |
| E105 | Unknown enum value. The message lists the accepted values. |
| E110 | Unsupported format version in `geneva`. |

## References (E200–E299)

| Code | Meaning |
| --- | --- |
| E200 | Unknown asset id. The help suggests the closest declared id. |
| E201 | Asset kind does not match its use (for example an image source pointing at a video asset). |
| E202 | Asset path is absolute, empty, or contains `..`. Paths must stay under the asset root. |
| E203 | Asset kind could not be inferred from the extension; set `kind`. |
| E206 | Unknown composition name. The help suggests the closest declared name. |
| E207 | A composition contains itself, directly or through another composition, or nesting exceeds 8 levels. |
| W201 | (note) An asset is declared but never used. |
| W202 | (note) A composition is declared but never used. |
| W203 | (note) An animation rule is declared but no clip plays it. |

## Timing (E300–E399)

| Code | Meaning |
| --- | --- |
| E300 | A time is negative. |
| E301 | An empty or inverted range: `out` not after `in`, `duration` of zero, or a duration longer than the source range. |
| E302 | Clips in the same layer or audio track overlap without a transition. |
| E303 | Keyframes are not in strictly increasing time order. |
| E305 | A length cannot be determined: an open-ended clip with no `duration` and no `output.duration`. |
| E306 | A transition is longer than the overlap the previous clip provides. |
| W300 | A keyframe lies after the end of its clip and will never be reached. |
| W301 | A clip starts after or runs past the output duration; it is cut. |
| W302 | A layer or track has no clips. |
| W303 | The first clip of a layer has a transition, which has nothing to blend from. |
| W304 | An audio clip's fades add up to more than its length. |
| W305 | `out` is past the end of the file; the clip ends where the file ends. |

## Values (E400–E499)

| Code | Meaning |
| --- | --- |
| E401 | Invalid easing: cubic-bezier x control points outside 0..=1, or non-positive spring parameters. |
| E402 | A value is out of range: opacity outside 0..=1, non-positive sizes or durations, negative stroke widths or radii, font weight outside 100..=900. |
| E405 | A text source has neither `text` nor `words`. |
| E411 | Words are out of order, overlapping, or have an end not after their start. |
| E420 | The output color tags describe HDR (`pq` or `hlg`) but `encode.video.codec` carries eight bits or is unset (the default, H.264); set it to `h265`, `av1`, `vp9` or `prores`. |
| E421 | `encode.video.profile` names a profile of another codec, or is set without a codec. |
| E422 | `encode.video.fixed_keyframes` is set without `encode.video.keyframe_interval`. |
| E430 | An `outputs` entry has a field its kind does not take (`at` on a video, `every` on a poster, `encode` on a picture). |
| E431 | An `outputs` entry's `path` is not a plain file name inside the output directory, or its extension is not one the kind can write. |
| E432 | Two `outputs` entries would write the same file. |
| E433 | An `outputs` value is out of range: a poster `at` outside the composition, a sprite `every` that is not positive, or `columns` of 0. |
| E440 | A clip's `animation` names a rule that is not in the document's `keyframes`. |
| E441 | An `animation` value is malformed: no rule name, no duration, a duration of zero, two timing functions, or three times. |
| E442 | A `keyframes` rule is malformed: an offset that is not `from`, `to` or a percentage in 0% to 100%, two offsets at the same place, a property that cannot be animated, or a value geneva cannot read (a percentage in a translation, an unknown transform function). |
| E443 | A property is driven by the clip's own `keyframes` and by an animation at once. |
| E444 | Two animations on one clip set the same property at the same time. |
| W440 | An animation rule sets nothing at an offset, or has fewer than two offsets, so nothing interpolates. |
| E450 | An `html` source has both `html` and `asset`, or neither. |
| E451 | The markup or its styles did not parse. The message says what is wrong and the location points at it: a tag that closes the wrong element, an unclosed element, a selector or at-rule geneva does not support, a declaration block that is never closed. |
| W450 | A declaration names something geneva does not draw. It is skipped and the rest of the document is drawn; the message names the property and the element. |
| W401 | Output width or height is odd; most codecs need even dimensions. |
| W402 | `subtitles --burn` found no cues in the file; the picture is written unchanged. |
| W403 | A burned-in subtitle cue extends off-screen. The message gives the box size and by how much it leaves the frame; the path names the clip. |
| N404 | (note) A burned-in subtitle cue lies outside the title-safe area (`--safe`, 5% in from each edge by default). |
| N405 | (note) A burned-in subtitle cue was shrunk to fit (`--fit`); the message gives the sizes. |
| N410 | (note) What `--for TARGET` chose and why: size, codec and level, quality, caps, keyframes, audio. |
| W411 | The video is longer than the `--for` target allows. Geneva never shortens a video on its own. |
| W412 | The written file is larger than the `--for` target or `--budget` allows. |
| W413 | The `--for` target expects an MP4 file and the output has another extension. |
| W414 | The `--budget` (or the target's size limit) leaves under 200 kb/s for the picture at this length; the budget cannot be met. |

## Rendering (E500–E599)

These are produced when rendering, not by `validate`.

| Code | Meaning |
| --- | --- |
| E500 | The clip uses a source kind this renderer does not implement yet. |
| E501 | An asset file could not be opened, read or decoded. With `--probe` this is reported at validation time, at `/assets/<id>/src`. |
| E502 | The requested time is outside the composition. |

## Notes from rendering

| Code | Meaning |
| --- | --- |
| N600 | What `render` did that you may want to know: streams were copied instead of re-encoded, or a cut moved to a keyframe. |

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. Warnings may have been printed. |
| 1 | The timeline has validation errors. |
| 2 | Usage or I/O error (bad arguments, unreadable file). |
| 3 | Rendering failed. |
