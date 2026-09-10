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
| E102 | Required field missing. |
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
| W201 | (note) An asset is declared but never used. |

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

## Values (E400–E499)

| Code | Meaning |
| --- | --- |
| E401 | Invalid easing: cubic-bezier x control points outside 0..=1, or non-positive spring parameters. |
| E402 | A value is out of range: opacity outside 0..=1, non-positive sizes or durations, negative stroke widths or radii, font weight outside 100..=900. |
| E405 | A text source has neither `text` nor `words`. |
| E411 | Words are out of order, overlapping, or have an end not after their start. |
| E420 | The output color tags describe HDR, which is not supported. |
| W401 | Output width or height is odd; most codecs need even dimensions. |

## Rendering (E500–E599)

These are produced when rendering, not by `validate`.

| Code | Meaning |
| --- | --- |
| E500 | The clip uses a source kind this renderer does not implement yet. |
| E501 | An asset file could not be read or decoded. |
| E502 | The requested time is outside the composition. |

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. Warnings may have been printed. |
| 1 | The timeline has validation errors. |
| 2 | Usage or I/O error (bad arguments, unreadable file). |
| 3 | Rendering failed. |
