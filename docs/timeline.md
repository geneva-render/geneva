# Timeline format 0.3

A timeline is a JSON document that describes a video composition: the output
frame, a table of assets, visual layers made of clips, and audio tracks.
This page is the reference for humans and for programs that generate
timelines. The machine-readable schema is in
[`schema/geneva-timeline-0.3.schema.json`](../schema/geneva-timeline-0.3.schema.json)
and is printed by `geneva schema`.

Unknown fields are errors everywhere. That is deliberate: a misspelled
property fails validation with the list of allowed names instead of being
silently ignored.

## Minimal document

```json
{
  "geneva": "0.3",
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
| `geneva` | yes | Format version, `"0.3"`. A `"0.1"` or `"0.2"` document is read as it is: 0.2 added optional clip fields (`crop`, `effects`, `mask`, `speed`), 0.3 adds the optional `outputs` map. |
| `output` | yes | Frame size, rate, duration, background, color, audio and encoding settings. |
| `outputs` | no | Map of name to [output entry](#outputs): the files one render writes from the composition, when there is more than one. |
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
| `color` | no | BT.709 SDR, limited range | Color tags for the output; see [color.md](color.md). `pq` and `hlg` need a ten-bit codec, `h265`, `av1`, `vp9` or `prores` (E420). |
| `audio.sample_rate` | no | 48000 | Output sample rate. |
| `audio.channels` | no | 2 | 1 or 2. |
| `encode.container` | no | from the output file extension | `mp4`, `mov`, `mkv`, `webm`, `mxf`; audio only: `m4a`, `ogg`, `flac`, `wav`, `mp3`; `image-sequence` (one PNG or JPEG file per frame, the output path being a pattern such as `frames/%04d.png`). Audio-only containers write no video; image sequences write no audio. |
| `encode.video.codec` | no | `h264` (`vp9` for webm, `dnxhd` for mxf, `png` for image sequences) | `h264`, `h265`, `vp9`, `av1`, `prores`, `dnxhd`, `png`, `mjpeg`. H.265 needs a hardware encoder. ProRes is 10-bit 4:2:2 (4:4:4 for the 4444 profiles); DNxHR is 8-bit 4:2:2 except HQX (10-bit) and 444, and needs a picture of at least 256×120. See [containers and codecs](cli.md#containers-and-codecs) for what each container holds. |
| `encode.video.profile` | no | `hq` for prores, `dnxhr-hq` for dnxhd | ProRes: `proxy`, `lt`, `standard`, `hq`, `4444`, `4444-xq`. DNxHR: `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq`, `dnxhr-hqx`, `dnxhr-444`. The profile must belong to the codec (E421). |
| `encode.video.crf` | no | per codec | Constant quality; lower is better. |
| `encode.video.preset` | no | per codec | Encoder speed preset name (`ultrafast` to `veryslow`); ignored by encoders without presets. |
| `encode.video.hardware` | no | `auto` | `auto`, `never`, `require`. |
| `encode.video.keyframe_interval` | no | the encoder's own | Seconds between keyframes; 2 is usual for anything played over a network. |
| `encode.video.max_bitrate_kbps` | no | none | Bitrate ceiling in kb/s; quality stays constant until it bites (x264). VideoToolbox has no such mode and takes the ceiling only together with `bitrate_kbps`. |
| `encode.video.bitrate_kbps` | no | none | Average bitrate in kb/s to aim for: bitrate mode for hardware encoders, which cannot hold constant quality under a ceiling; x264 ignores it and keeps constant quality under the ceiling. `--budget` sets it. |
| `encode.video.level` | no | the encoder's own | H.264 or H.265 level such as `"4.1"`, for the decoders that check it. |
| `encode.video.tune` | no | none | What the picture is like, in x264's names: `film`, `animation`, `grain`, `stillimage`, `fastdecode`, `zerolatency`. x264 applies all of them; VP9 takes `film`, AV1 `fastdecode`, NVENC and VideoToolbox `zerolatency`. An encoder with no equivalent ignores it and the report says so. |
| `encode.video.fixed_keyframes` | no | `false` | Keyframes at `keyframe_interval` only, never at scene changes, as streaming platforms and segmenters want; needs the interval (E422). x264, VP9, AV1, NVENC and VideoToolbox place them so; OpenH264 cannot and the report says so. |
| `encode.video.chunks` | no | `auto` | How many stretches the output is encoded in at once, each on its own share of the cores, joined afterwards without re-encoding. `auto` chunks only where it has been measured to help, VP9 and OpenH264, into the machine's cores divided by two; it never chunks a hardware encoder, which is its own bottleneck; a number forces it for any encoder; `1` turns it off. Boundaries fall on clip starts or the keyframe grid when one is near, so each stretch starts at a keyframe that was due anyway. Not with `max_bitrate_kbps` or `bitrate_kbps`, a smart cut, or an image sequence. The report says how many ran. |
| `encode.fast_start` | no | `true` | Whether MP4, MOV and M4A files carry their index at the front so playback can start before the download ends. |
| `encode.audio.codec` | no | `aac` (`opus` for webm and ogg, `flac` for flac, `pcm` for wav, `mp3` for mp3, `pcm24` for mxf) | `aac`, `opus`, `mp3`, `vorbis`, `flac`, `alac`, `ac3`, `pcm` (16-bit), `pcm24`. |
| `encode.audio.bitrate_kbps` | no | 160 | Audio bitrate. |

### `outputs`

A composition is usually rendered to the one file `geneva render -o FILE`
names. The `outputs` map instead lists every file one render should
write from it, each made from the same composited frames in a single
pass: renditions at several sizes, a poster, a sprite sheet for seek
previews, the audio alone. `geneva render -o DIR` then writes them all
into `DIR`, and the report lists each file with its size and how it was
made. The frames are composited once and scaled per rendition; the
source is decoded once, whatever the number of entries.

```json
"outputs": {
  "1080p":   { "kind": "video" },
  "720p":    { "kind": "video", "height": 720 },
  "poster":  { "kind": "poster" },
  "sprites": { "kind": "sprites", "every": "5s", "columns": 10 },
  "speech":  { "kind": "audio", "audio": { "sample_rate": 16000, "channels": 1 } }
}
```

| Field | Kinds | Default | Description |
| --- | --- | --- | --- |
| `kind` | all | required | `video`, `poster` (one still), `sprites` (a sheet of thumbnails and a WebVTT file mapping times to tiles) or `audio` (the sound alone). |
| `path` | all | the entry's name with the kind's usual extension: `.mp4`, `.jpg`, `.jpg`, `.wav` | File name inside the output directory: a plain name, no directories (E431), with an extension the kind can write: `mp4`, `mov`, `mkv`, `webm`, `mxf` for video; `jpg`, `jpeg`, `png` for pictures; `wav`, `m4a`, `mp3`, `flac`, `ogg` for audio. Two entries cannot write the same file (E432). A sprite sheet also writes `<name>.vtt` beside it. |
| `width`, `height` | video, poster, sprites | the canvas size; sprites: tiles 90 pixels high | Picture size. Giving one of the two keeps the canvas's aspect (video sizes rounded to even, W401 when both are given odd). For sprites, the size of one tile; `height` only, the width follows. |
| `encode` | video, audio | `output.encode` for video; the container's usual codecs for audio | Encoder settings for this file, the same block as `output.encode`. |
| `audio` | video, audio | `output.audio` | Sample rate and channel count for this file. |
| `at` | poster | the first clear frame | Time of the still: a [time](#times) inside the composition (E433). Without it, the poster is the first frame after the opening (5% in, at least 20 frames) that is not dark and follows some motion, so a fade-in or a black leader is skipped; failing that, the frame a tenth of the way in. |
| `every` | sprites | about a hundred tiles over the length, at least a second apart | Time between tiles (E433 when zero or negative). |
| `columns` | sprites | 10 | Tiles per row (E433 when zero). |

A field that does not belong to the entry's kind is an error (E430).

A `video` entry at the canvas size with no encode settings of its own
is a candidate for [stream copy](architecture.md#stream-copy) like a
single-file render, and is copied when the composition allows it; the
other renditions are encoded from the composited frames, each in its own
thread. With `--crf`, `--preset` or `--exact` on the command line every
video entry is encoded. When no entry is a video, only the frames the
pictures need are composited.

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
| `crop` | no | the whole source | A rectangle of the source that becomes the clip's box; see below. |
| `fit` | no | `contain` for video, `none` otherwise | `none`, `contain`, `cover`, `fill`: how the source box is sized to the frame before the transform. |
| `effects` | no | `[]` | Effects on the placed picture, in order; see below. |
| `mask` | no | | A shape cut from the clip's box, or a luma image over it; see below. |
| `speed` | no | `1` | How fast the source plays: `2` is twice as fast, `0.5` half speed. The clip lasts its source range divided by it; a video's audio is resampled, so the pitch follows; the clip's own keyframes stay in output time. A clip with a speed is always composited. |
| `transform` | no | centered | Position, anchor, scale, rotation. |
| `opacity` | no | `1` | Animatable, 0 to 1. |
| `blend` | no | `normal` | `normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `difference`, `soft-light`, `add`. Computed in linear light. |

### `crop`

A rectangle of the source that becomes the clip's box: the rest is
discarded before `fit` and the transform see the picture, so a 1920×1080
video with `"crop": { "x": 420, "width": 1080 }` is a 1080×1080 clip.

| Field | Default | Description |
| --- | --- | --- |
| `x`, `y` | `0` | Top-left corner, in source pixels or percentages of the source's own size. |
| `width`, `height` | the rest of the source | Size, in the same units. |

Values must lie inside the source as written (E402); a rectangle that
reaches past the source's actual size is clamped to it, and one that
leaves nothing paints nothing. The anchor's percentages refer to the
cropped box. A video shown as it is with a crop still takes the direct
path: the region is scaled straight from the decoder.

### `effects`

A list of effects applied to the picture after the transform, so their
sizes are in output pixels, and before opacity and blending. Each has a
`kind`.

`blur`

| Field | Default | Description |
| --- | --- | --- |
| `radius` | | Standard deviation of a Gaussian blur in output pixels, as CSS's `blur()`; animatable; 0 leaves the picture as it is. |

The blur spreads past the picture's edges into transparency, so a
blurred clip fades out at its border rather than stopping. A wide blur
is computed on a smaller layer and brought back, which looks the same
and keeps the cost flat. A clip with effects is always composited; the
copy, smart-cut and direct paths do not apply.

The idiom for a portrait canvas is the picture whole over a blurred,
scaled-up copy of itself:

```json
"layers": [
  { "clips": [ { "source": { "kind": "video", "asset": "v", "audio": false }, "fit": "cover",
      "effects": [ { "kind": "blur", "radius": 45 } ] } ] },
  { "clips": [ { "source": { "kind": "video", "asset": "v" }, "fit": "contain" } ] }
]
```

`--fill blur` on the verbs builds exactly this.

### `mask`

A mask limits what the clip shows. It is defined in the clip's box (the
source after the crop, before `fit` and the transform), so it moves,
scales and rotates with the clip: pixels of the box, or percentages of
its size.

| Field | Default | Description |
| --- | --- | --- |
| `shape` | `rect` | `rect` or `ellipse`, inscribed in the box below. |
| `x`, `y` | `0` | Top-left corner of the shape's box. |
| `width`, `height` | the clip's box | Size of the shape's box. |
| `radius` | `0` | Corner radius of a rectangle, in box pixels. |
| `feather` | `0` | Width of the soft edge in box pixels; 0 is a hard edge. |
| `asset` | | Id of an image asset whose luma (times alpha) is the coverage, stretched over the clip's box: white shows, black or transparent hides. The shape fields are ignored. |
| `invert` | `false` | Show what the mask hides and hide what it shows. |

Rounded corners on a picture-in-picture, for instance:

```json
{ "source": { "kind": "video", "asset": "cam" }, "fit": "none",
  "mask": { "radius": 24, "feather": 1 },
  "transform": { "position": { "x": "85%", "y": "85%" }, "scale": 0.25 } }
```

A masked clip is always composited; the copy, smart-cut and direct
paths do not apply.

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
| `in` | no | `0` | Source time at the clip start. Time 0 of a file is its first video frame; an audio track that starts later keeps its offset. |
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
| `font` | no | system sans-serif | Id of a `font` asset, a family name, or the CSS `font` shorthand (`"600 40px/1.2 Inter"`), whose parts fill in `size`, `weight`, `italic` and `line_height` unless those are set. A font asset also supplies its weight and style unless set here. |
| `size` | no | `48` | Font size in pixels. |
| `weight` | no | `400` | 100 to 900. |
| `italic` | no | `false` | |
| `color` | no | `white` | |
| `letter_spacing` | no | `0` | Pixels. |
| `max_width` | no | output width | Wrap width. |
| `align` | no | `center` | `left`, `center`, `right`. |
| `line_height` | no | `1.2` | Multiple of the font size. |
| `padding` | no | `0` | Space between text and background box, as a number or `"8px"`. |
| `background` | no | none | Background box color. |
| `radius` | no | `0` | Background box corner radius. |
| `outline` | no | | `{ "color", "width" }`, or the shorthand `"2px black"` (width and color in either order). |
| `shadow` | no | | `{ "color", "x", "y", "blur" }`, or the `text-shadow` shorthand `"0 2px 8px #0008"` (x, y, optional blur, optional color). |

#### CSS shorthands

Where an author would write CSS, the timeline takes the same strings, for
the values that map one to one:

| CSS | Timeline field | Example |
| --- | --- | --- |
| `text-shadow` | `shadow` | `"0 2px 8px #0008"` |
| `outline` (`-webkit-text-stroke`) | `outline` | `"2px black"` |
| `font` | `font` | `"italic 600 40px/1.2 Inter"` |
| `padding` | `padding` | `"8px"` (one value) |
| `color`, `background-color` | `color`, `background` | `"#ffdd00"`, `"rgba(0, 0, 0, 0.5)"` |

The object form stays canonical: a printed timeline (`--show-timeline`)
writes `shadow` and `outline` as objects, and a `font` shorthand is
expanded into its fields when the document is resolved. There is no
cascade, box model or selector; each value belongs to one text source.
A string that does not parse is an `E103` at the field, with the form
expected.

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
| `speed` | no | `1` | How fast the source plays; the clip lasts its range divided by it and the pitch follows. |

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
frame-accurate rendering. With an `outputs` map, `-o` names a directory
and every entry is written from one pass over the composition.

The CPU reference renderer draws every source kind: `solid`, `shape`,
`image`, `video`, `text` and `composition`, with every transform, opacity,
blend mode and transition. `geneva render` mixes audio tracks and the audio
of video clips. Hardware encoders are not used yet; `encode.video.hardware`
is accepted and ignored.

Text uses fonts from `font` assets first and falls back to fonts installed
on the system. Output that depends on system fonts can differ between
machines; ship the fonts as assets when the result must be identical.
