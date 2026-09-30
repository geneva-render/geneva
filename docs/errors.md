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
| E301 | An empty or inverted range: `out` not after `in` (on a clip, an audio clip or a captions source), `duration` of zero, or a duration longer than the source range. |
| E302 | Clips in the same layer or audio track overlap without a transition. |
| E303 | Keyframes are not in strictly increasing time order. |
| E305 | A length cannot be determined: an open-ended clip with no `duration` and no `output.duration`. |
| E306 | A transition is longer than the overlap the previous clip provides. |
| E307 | A clip sets `transition_out` while the clip after it sets `transition`. Both describe the same join; keep one. |
| E308 | A transition's `ease` is malformed. The message says what is wrong. |
| W300 | A keyframe lies after the end of its clip and will never be reached. |
| W301 | A clip starts after or runs past the output duration; it is cut. |
| W302 | A layer or track has no clips. |
| W304 | A transition has a `color` but its kind never shows one. Only `fade` dips through a color. |
| W305 | `out` is past the end of the file; the clip ends where the file ends. |
| W306 | An audio clip's fades add up to more than its length. |
| N310 | (note, `validate --probe`) What an asset's audio measures: integrated loudness, true peak, noise floor and how far under the signal it sits, rate and channels; and, where the document sets `output.audio.loudness`, how far from the target it is. Measuring decodes the track once. |
| W311 | (`validate --probe`) An asset's audio is clipped: runs of three or more samples at full scale. The message counts them and gives the longest and the first; a loudness target does not undo the distortion. |
| W312 | (`validate --probe`) An asset's audio carries a DC offset over 1% of full scale. `output.audio.hygiene` takes it out. |
| W313 | (`validate --probe`) An asset's audio hums at 50 or 60 Hz; the message gives the level and how many harmonics show. `output.audio.hygiene` notches it. |
| W314 | (`validate --probe`) An asset's audio is silent: nothing above -60 dBTP. |
| N315 | (note, `validate --probe`) An asset's audio is two channels of one signal, which reads 3 dB louder than the same recording in one channel. |
| W316 | (`validate --probe`) An asset's audio could not be decoded for measuring; the rest of the probe stands. |

## Values (E400–E499)

| Code | Meaning |
| --- | --- |
| E401 | Invalid easing: cubic-bezier x control points outside 0..=1, or non-positive spring parameters. |
| E402 | A value is out of range: opacity outside 0..=1, non-positive sizes or durations, negative stroke widths or radii, font weight outside 100..=900, a captions `max_chars` under 8. |
| E405 | A text source has neither `text` nor `words`. |
| E411 | Words are out of order, overlapping, or have an end not after their start. |
| E420 | The output color tags describe HDR (`pq` or `hlg`) but `encode.video.codec` carries eight bits or is unset (the default, H.264); set it to `h265`, `av1`, `vp9` or `prores`. |
| E421 | `encode.video.profile` names a profile of another codec, or is set without a codec. |
| E422 | `encode.video.fixed_keyframes` is set without `encode.video.keyframe_interval`. |
| E423 | `audio.loudness` is out of range: `target_lufs` outside -40 to -5, or `true_peak_dbtp` outside -20 to 0. |
| E430 | An `outputs` entry has a field its kind does not take (`at` on a video, `every` on a poster, `encode` on a picture). |
| E431 | An `outputs` entry's `path` is not a plain file name inside the output directory, or its extension is not one the kind can write. |
| E432 | Two `outputs` entries would write the same file. |
| E433 | An `outputs` value is out of range: a poster `at` outside the composition, a sprite `every` that is not positive, or `columns` of 0. |
| E434 | A second `sprites` entry. A document writes one sprite sheet; posters can be several. |
| E440 | A clip's `animation` names a rule that is not in the document's `keyframes`. |
| E441 | An `animation` value is malformed: no rule name, no duration, a duration of zero, two timing functions, or three times. |
| E442 | A `keyframes` rule is malformed: an offset that is not `from`, `to` or a percentage in 0% to 100%, two offsets at the same place, a property that cannot be animated, an unknown transform function, a percentage distance on a clip whose box is not known until its file is opened, or a property a clip cannot play (anything past `transform` and `opacity`) in a rule the clip is given rather than an element inside markup. |
| E443 | A property is driven by the clip's own `keyframes` and by an animation at once. |
| E444 | Two animations on one clip set the same property at the same time. |
| W440 | An animation rule sets nothing at an offset, or, for a rule a clip plays, has no property at two offsets, so nothing interpolates. An element inside markup plays a rule with a lone `to` from the value under it, so it is not warned about there. |
| E450 | An `html` source has both `html` and `asset`, or neither. |
| E451 | The markup or its styles did not parse. The message says what is wrong and the location points at it: a tag that closes the wrong element, an unclosed element, a selector or at-rule geneva does not support, a declaration block that is never closed. |
| E452 | A file the markup points at, an `<img src>` or a stylesheet `<link href>`, is not there, or its path leaves the asset root (a leading `/`, a `..`, a drive letter or a URL). Paths inside markup are relative to the markup itself. |
| W451 | Two things ask for the same motion: a clip's `animation` and one in the markup it draws (the clip's is played), or a rule name that is in both the document's `keyframes` and the markup's (the document's is played). |
| W454 | An element sets `z-index` where it does not apply. It takes effect on a positioned box or a flex item, and a browser ignores it elsewhere too, so the drawing is right and the declaration is not doing what it looks like. |
| W455 | (`render`) A group in markup asks for a buffer bigger than the bound allows, so what lies past it is not drawn. The bound is an area, nine times the frame's; an element wider than the frame is fine, one many times the frame in both directions is not. Put a very large moving picture in a composition, which has no such bound. |
| W452 | A rule in the markup's own `<style>` parsed and then matched no element, so its declarations reached nothing. Usually a misspelt class or a tag the markup does not use. A stylesheet the markup links to is not checked, since it is written for more than one file. |
| W450 | The markup names something geneva does not draw: a property it has no support for, or an element with its own renderer (`<iframe>`, `<svg>`, `<canvas>`, `<video>`, `<object>`, `<embed>`). It is skipped and the rest is drawn; the message names it. |
| E453 | A caption file could not be read: it is not there, it is not a format geneva reads (`.srt`, `.vtt`, or the `.json` a speech recogniser writes), it did not parse, no words could be found in it, or its word times are plainly milliseconds rather than seconds. |
| W453 | A `highlight` was asked for on a file that times whole cues rather than words, so nothing is picked out as it is said; or a caption file parsed but holds no cues. |
| N453 | (note, `validate --probe`) How many cues were read from a caption file, and where they came from; also when a `margin` puts captions inside the title-safe inset. Reading the file is what counts them, so a plain `validate`, which does not open assets, does not report it. |
| W401 | Output width or height is odd; most codecs need even dimensions. |
| W402 | `subtitles --burn` found no cues in the file; the picture is written unchanged. |
| W403 | A burned-in subtitle cue extends off-screen. The message gives the box size and by how much it leaves the frame; the path names the clip. |
| W404 | A clip names no `fit` and lands in the frame in a way its author probably did not mean: a picture bigger than the frame, drawn at its own size so the edges are cut, or a video shown whole in under two thirds of the frame. The message gives both sizes and the share. The defaults are right for what each source is usually for, so this says where the result is surprising rather than changing them; a clip with a `crop`, a `scale` or an `animation` is left alone, since each is a decision about size. Needs the source's size, so it appears where the assets are read: `render`, `frame`, the verbs, and `validate --probe`. |
| N404 | (note) A burned-in subtitle cue lies outside the title-safe area (`--safe`, 5% in from each edge by default). |
| N405 | (note) A burned-in subtitle cue was shrunk to fit (`--fit`); the message gives the sizes. |
| W405 | A document names a font family this machine has no face of, in the `font` of a text or captions source or in a markup `font-family`. The text still draws, in whatever the shaper falls back to, which is a different picture on a machine with different fonts; add the file as an asset of kind `font`, which a text source names by its asset id and markup by the family the file declares. A family a font asset of the document declares counts as present, so carrying the font is enough to silence it. |
| W406 | An `outputs` entry gives both `width` and `height` in a shape other than the canvas's. The whole canvas is stretched into it; give one of the two to keep the shape. |
| N410 | (note) What `--for TARGET` chose and why: size, codec and level, quality, caps, keyframes, audio. |
| W411 | The video is longer than the `--for` target allows. Geneva never shortens a video on its own. |
| W412 | The written file is larger than the `--for` target or `--budget` allows. |
| W413 | The `--for` target expects an MP4 file and the output has another extension. |
| W414 | The `--budget` (or the target's size limit) leaves under 200 kb/s for the picture at this length; the budget cannot be met. |

## Rendering (E500–E599)

These are produced when rendering, not by `validate`.

| Code | Meaning |
| --- | --- |
| E500 | The clip uses a source kind, or a feature, this renderer does not implement; the message names it. Both renderers implement the whole format today, so it is not reported. |
| E501 | An asset file could not be opened, read or decoded. With `--probe` this is reported at validation time, at `/assets/<id>/src`. |
| E502 | The requested time is outside the composition. |
| E503 | The renderer's device failed or cannot do what the frame needs (a GPU lost or out of memory). The CPU renderer never reports it. |
| E504 | `plan` or `farm` cannot split this output into parts: it has `outputs`, a bitrate target or ceiling, no picture, or is an image sequence. The message says which. |
| E505 | A `geneva worker` could not take part: the farm could not be reached, refused its token, runs another geneva build or encoder, or this machine reads the timeline differently (a font or a file it lacks). The message says which. |

## Notes from rendering

| Code | Meaning |
| --- | --- |
| N600 | What `render`, `plan` or `join` did that you may want to know: streams were copied instead of re-encoded, a cut moved to a keyframe, or fewer parts than asked for. |

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. Warnings may have been printed. |
| 1 | The timeline has validation errors. |
| 2 | Usage or I/O error (bad arguments, unreadable file). |
| 3 | Rendering failed. |
