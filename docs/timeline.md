# Timeline format 0.1

A timeline is a JSON document that describes a video composition: the output
frame, a table of assets, visual layers made of clips, and audio tracks.
This page is the reference for humans and for programs that generate
timelines. The machine-readable schema is in
[`schema/geneva-timeline-0.1.schema.json`](../schema/geneva-timeline-0.1.schema.json)
and is printed by `geneva schema`.

Unknown fields are errors everywhere. That is deliberate: a misspelled
property fails validation with the list of allowed names instead of being
silently ignored.

## Minimal document

```json
{
  "geneva": "0.1",
  "output": { "width": 1280, "height": 720, "fps": 30, "duration": "3s" },
  "layers": [
    { "clips": [ { "source": { "kind": "solid", "color": "#1d2230" } } ] }
  ]
}
```

## Value conventions

### Times

Times and durations accept:

| Form | Example | Meaning |
| --- | --- | --- |
| number | `1.5` | seconds |
| seconds | `"1.5s"` | seconds |
| milliseconds | `"1500ms"` | milliseconds |
| frames | `"45f"` | frames at the output frame rate |
| timecode | `"00:01:02.5"`, `"1:02.5"` | hours, minutes, seconds |

Times are kept as exact rationals internally, so `0.1` is exactly one tenth
of a second and a frame at 29.97 fps is exactly 1001/30000 s. Times must not
be negative.

### Frame rate

`fps` is a positive number (`30`, `25`) or an exact ratio string
(`"30000/1001"`). The decimal spellings `23.976`, `29.97` and `59.94` are
snapped to their exact 1001-based ratios.

### Lengths

Positions and sizes are pixels in the output frame when written as numbers
(`120`) or as strings with a unit (`"120px"`). Percentages (`"50%"`) refer to
the output width for horizontal values and the output height for vertical
values. In `transform.anchor`, percentages refer to the clip's own box.

### Colors

Colors are sRGB strings: `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`,
`"rgb(255, 136, 0)"`, `"rgba(255, 136, 0, 0.5)"`, or one of `black`,
`white`, `red`, `green`, `blue`, `yellow`, `cyan`, `magenta`, `gray`,
`orange`, `transparent`. Animated colors are interpolated in linear light.

### Animated values

Any property documented as *animatable* takes either a constant or an
object with a `keyframes` array:

```json
"opacity": { "keyframes": [
  { "t": 0, "v": 0 },
  { "t": "0.5s", "v": 1, "ease": "ease-out" }
] }
```

- `t` is relative to the start of the clip.
- Keyframes must be in strictly increasing time order.
- Before the first keyframe the value is held at the first value; after the
  last it is held at the last.
- `ease` applies to the segment that starts at that keyframe. It is one of
  the names `linear` (default), `ease`, `ease-in`, `ease-out`,
  `ease-in-out`, `hold` (jump at the next keyframe); an object
  `{ "cubic-bezier": [x1, y1, x2, y2] }` with CSS semantics; or an object
  `{ "spring": { "stiffness": 170, "damping": 26, "mass": 1 } }`. A spring
  is solved analytically and rescaled so it settles exactly at the next
  keyframe; its parameters shape the overshoot and bounce, not the duration.

## Document structure

### Top level

| Field | Required | Description |
| --- | --- | --- |
| `geneva` | yes | Format version, `"0.1"`. |
| `output` | yes | Frame size, rate, duration, background, color, audio and encoding settings. |
| `assets` | no | Map of asset id to asset. |
| `compositions` | no | Map of name to reusable composition. |
| `layers` | no | Visual layers, composited bottom to top. |
| `audio` | no | Audio-only tracks. |

### `output`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `width`, `height` | yes | | Frame size in pixels. Odd values warn (W401). |
| `fps` | yes | | Frame rate. |
| `duration` | no | end of the last clip | Total length. Clips running past it are cut (W301). |
| `background` | no | `"black"` | Clear color. Use `"transparent"` for alpha output. |
| `color` | no | BT.709 SDR, limited range | Color tags for the output; see [color.md](color.md). HDR transfers are rejected (E420). |
| `audio.sample_rate` | no | 48000 | Output sample rate. |
| `audio.channels` | no | 2 | 1 or 2. |
| `encode.container` | no | from the output file extension | `mp4`, `mov`, `mkv`, `webm`, `mxf`; audio only: `m4a`, `ogg`, `flac`, `wav`, `mp3`; `image-sequence` (one PNG or JPEG file per frame, the output path being a pattern such as `frames/%04d.png`). Audio-only containers write no video; image sequences write no audio. |
| `encode.video.codec` | no | `h264` (`vp9` for webm, `dnxhd` for mxf, `png` for image sequences) | `h264`, `h265`, `vp9`, `av1`, `prores`, `dnxhd`, `png`, `mjpeg`. H.265 needs a hardware encoder. ProRes is 10-bit 4:2:2 (4:4:4 for the 4444 profiles); DNxHR is 8-bit 4:2:2 except HQX (10-bit) and 444, and needs a picture of at least 256×120. See [containers and codecs](cli.md#containers-and-codecs) for what each container holds. |
| `encode.video.profile` | no | `hq` for prores, `dnxhr-hq` for dnxhd | ProRes: `proxy`, `lt`, `standard`, `hq`, `4444`, `4444-xq`. DNxHR: `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq`, `dnxhr-hqx`, `dnxhr-444`. The profile must belong to the codec (E421). |
| `encode.video.crf` | no | per codec | Constant quality; lower is better. |
| `encode.video.preset` | no | per codec | Encoder speed preset name (`ultrafast` to `veryslow`); ignored by encoders without presets. |
| `encode.video.hardware` | no | `auto` | `auto`, `never`, `require`. |
| `encode.video.keyframe_interval` | no | the encoder's own | Seconds between keyframes; 2 is usual for anything played over a network. |
| `encode.video.max_bitrate_kbps` | no | none | Bitrate ceiling in kb/s; quality stays constant until it bites. |
| `encode.video.level` | no | the encoder's own | H.264 or H.265 level such as `"4.1"`, for the decoders that check it. |
| `encode.fast_start` | no | `true` | Whether MP4, MOV and M4A files carry their index at the front so playback can start before the download ends. |
| `encode.audio.codec` | no | `aac` (`opus` for webm and ogg, `flac` for flac, `pcm` for wav, `mp3` for mp3, `pcm24` for mxf) | `aac`, `opus`, `mp3`, `vorbis`, `flac`, `alac`, `ac3`, `pcm` (16-bit), `pcm24`. |
| `encode.audio.bitrate_kbps` | no | 160 | Audio bitrate. |

### `assets`

Each key is an id used by clips. Assets are the only way to reach files:
clips never contain paths.

| Field | Required | Description |
| --- | --- | --- |
| `src` | yes | Path relative to the asset root (the timeline's directory, or `--assets`). Absolute paths and `..` are rejected (E202). |
| `kind` | no | `video`, `image`, `audio`, `font`, `subtitle`. Inferred from the extension; set it when inference fails (E203). |
| `color` | no | Color tag overrides for files that are untagged or mistagged. |

### `compositions`

A composition is a small timeline of its own: a frame size, an optional
background (default transparent), and layers. A clip shows it with a source
of kind `composition`, and the same composition can be placed any number
of times with different transforms and timing.

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `width`, `height` | yes | | Frame size in pixels. Percentages inside the composition refer to this frame. |
| `background` | no | `transparent` | Clear color. |
| `layers` | yes | | Layers, as at the top level. Audio tracks are top-level only. |

Times inside a composition are relative to the clip that shows it. The
clip's length is the composition's natural length (the end of its last
fixed-length clip) unless the clip sets `duration`; if the composition
contains open-ended clips, they run for the clip's length. Compositions may
contain other compositions up to 8 levels deep, and never themselves (E207).

### `layers[]`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `id` | no | positional | Name used in diagnostics. |
| `enabled` | no | `true` | `false` skips the layer. |
| `clips` | yes | | Clips in time order. |

### `layers[].clips[]`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `id` | no | positional | Name used in diagnostics. |
| `source` | yes | | What to show; see below. |
| `start` | no | end of the previous clip | Timeline time the clip appears. |
| `duration` | no | see timing rules | How long it lasts. |
| `transition` | no | | `{ "kind": "crossfade", "duration": ... }` from the previous clip. |
| `fit` | no | `contain` for video, `none` otherwise | `none`, `contain`, `cover`, `fill`: how the source box is sized to the frame before the transform. |
| `transform` | no | centered | Position, anchor, scale, rotation. |
| `opacity` | no | `1` | Animatable, 0 to 1. |
| `blend` | no | `normal` | `normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `difference`, `soft-light`, `add`. Computed in linear light. |

### `transform`

The anchor is a point on the clip's box. It is placed at `position` in the
output frame; `scale` and `rotation` act around it.

| Field | Default | Description |
| --- | --- | --- |
| `position` | `{ "x": "50%", "y": "50%" }` | Animatable point in the output frame. |
| `anchor` | `{ "x": "50%", "y": "50%" }` | Point on the clip's box; percentages refer to the box. |
| `scale` | `1` | Animatable; a number or `{ "x": ..., "y": ... }`. |
| `rotation` | `0` | Animatable; degrees, clockwise. |

### Sources

Every source has a `kind`.

`video`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `asset` | yes | | Id of a `video` asset. |
| `in` | no | `0` | Source time at the clip start. |
| `out` | no | end of file | Source time to stop at. Must be after `in` (E301). |
| `audio` | no | `true` | Mix the file's audio into the output. |

`image`

| Field | Required | Description |
| --- | --- | --- |
| `asset` | yes | Id of an `image` asset. The box is the image's pixel size. |

`solid`

| Field | Required | Description |
| --- | --- | --- |
| `color` | yes | Animatable color. The box is the output frame. |

`shape`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `shape` | yes | | `rect` or `ellipse`. |
| `width`, `height` | yes | | Box size (lengths). |
| `fill` | no | `white` | Animatable color. |
| `stroke` | no | | `{ "color": ..., "width": px }`, drawn inside the edge. |
| `radius` | no | `0` | Corner radius for `rect`. |

`composition`

| Field | Required | Description |
| --- | --- | --- |
| `composition` | yes | Name of an entry under `compositions`. The box is the composition's frame. |

`text`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `text` | unless `words` | | The text. |
| `words` | no | | `[{ "text", "start", "end" }]` with clip-relative times, in order, non-overlapping (E411). |
| `highlight` | no | | Style overrides for the word whose range contains the current time. |
| `font` | no | system sans-serif | Id of a `font` asset, or a family name. A font asset also supplies its weight and style unless set here. |
| `size` | no | `48` | Font size in pixels. |
| `weight` | no | `400` | 100 to 900. |
| `italic` | no | `false` | |
| `color` | no | `white` | |
| `letter_spacing` | no | `0` | Pixels. |
| `max_width` | no | output width | Wrap width. |
| `align` | no | `center` | `left`, `center`, `right`. |
| `line_height` | no | `1.2` | Multiple of the font size. |
| `padding` | no | `0` | Space between text and background box. |
| `background` | no | none | Background box color. |
| `radius` | no | `0` | Background box corner radius. |
| `outline` | no | | `{ "color", "width" }`. |
| `shadow` | no | | `{ "color", "x", "y", "blur" }`. |

### `audio[]` and `audio[].clips[]`

Tracks have `id`, `enabled` and `clips` like layers. Audio clips:

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `asset` | yes | | Id of an `audio` or `video` asset. |
| `in`, `out` | no | `0`, end of file | Source range. |
| `start` | no | end of previous clip | Timeline start. |
| `duration` | no | source range | Length. |
| `gain_db` | no | `0` | Animatable gain in decibels. |
| `fade_in`, `fade_out` | no | `0` | Fade lengths. |

### `subtitles[]`

Subtitle tracks are written to the output as text streams that players
can show or hide; they are not drawn into the picture (use a `text` source
for that). Each track is one subtitle asset, a SubRip `.srt` or WebVTT
`.vtt` file.

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | no | Name used in diagnostics. |
| `enabled` | no | `false` leaves the track out. |
| `asset` | yes | A `subtitle` asset. |
| `language` | no | Language code, for example `"en"` or `"pt-BR"`; stored as the three-letter code containers use. |
| `title` | no | Track title shown by players. |
| `offset` | no | Shifts every cue on the output timeline; negative values move cues earlier and cues that end before zero are dropped. |

MP4 and MOV store the text as 3GPP timed text (`mov_text`), Matroska as
SubRip, WebM as WebVTT; simple tags such as `<i>` survive in Matroska and
WebM and are stripped for MP4 and MOV. Other containers cannot hold
subtitle streams.

```json
"assets": { "en": { "src": "captions.srt" } },
"subtitles": [ { "asset": "en", "language": "en", "title": "English" } ]
```

## Timing rules

1. A clip without `start` begins where the previous clip in the same layer
   ends; the first clip begins at 0.
2. Video and audio clips last `duration` if given, otherwise `out - in`.
   With neither, they play to the end of the file. Until
   media probing is available the file length is unknown, so such a clip
   is treated like an open-ended source (rule 3).
3. Images, solids, shapes and text are open-ended: without `duration` they
   last until `output.duration`. If that is not set either, validation fails
   with E305.
4. Clips in one layer must not overlap (E302), except for the overlap a
   `transition` declares. A clip with a transition and no explicit `start`
   is moved earlier by the transition duration. The previous clip must cover
   the whole overlap (E306).
5. `output.duration` defaults to the latest end across all layers and audio
   tracks. Clips extending past it are cut with W301.
6. All keyframe and word times are relative to the clip start.

## Determinism

Rendering is a pure function of the timeline, the assets and the requested
time. Nothing reads a clock. The same inputs produce the same frame on the
same platform, and perceptually identical frames across platforms; the
golden-frame tests in `tests/golden/` compare with tolerance for that
reason.

## Validation

`geneva validate timeline.json` reports every problem it can find, each
with a code, a JSON pointer, the offending value and a suggested fix. See
[errors.md](errors.md) for the codes. `--format json` prints the same
information as one JSON document.

## Renderer support

`geneva render` copies the source streams instead of rendering when the
composition is a plain cut or join of video at natural size and no quality
setting asks for a re-encode (see the architecture page); `--exact` forces
frame-accurate rendering.

The CPU reference renderer draws every source kind: `solid`, `shape`,
`image`, `video`, `text` and `composition`, with every transform, opacity,
blend mode and transition. `geneva render` mixes audio tracks and the audio
of video clips. Hardware encoders are not used yet; `encode.video.hardware`
is accepted and ignored.

Text uses fonts from `font` assets first and falls back to fonts installed
on the system. Output that depends on system fonts can differ between
machines; ship the fonts as assets when the result must be identical.
