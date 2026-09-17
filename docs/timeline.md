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

### Points

A point (`transform.position`, `transform.anchor`) is written as an object,
a pair, or a string:

```json
{ "x": 30, "y": 36 }        [30, 36]        "30 36"        "0% 50%"
```

A string component is a length or one of the CSS keywords `left`, `right`,
`top`, `bottom`, `center`. A keyword names its own axis, so `"left 36"`,
`"36 top"` and `"top 36"` all read the way CSS reads them, and a pair of
keywords works in either order (`"bottom right"`, `"right bottom"`). One
keyword centers the axis it says nothing about, so `"left"` is
`{ "x": "0%", "y": "50%" }`; one length goes on both axes, so `"12"` is
`{ "x": 12, "y": 12 }`.

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

A keyframe can also be written as its fields in order, `[t, v]` or
`[t, v, ease]`, which is easier to read down a column:

```json
"opacity": { "keyframes": [ [0, 0], ["0.5s", 1, "ease-out"] ] }
```

- `t` is relative to the start of the clip.
- Keyframes must be in strictly increasing time order.
- Before the first keyframe the value is held at the first value; after the
  last it is held at the last.
- `ease` applies to the segment that starts at that keyframe. It is one of
  the names `linear` (default), `ease`, `ease-in`, `ease-out`,
  `ease-in-out`, `hold` (jump at the next keyframe); an object
  `{ "cubic-bezier": [x1, y1, x2, y2] }` with CSS semantics; an object
  `{ "steps": [n, "jump-end"] }` (a staircase of `n` equal steps, the
  position one of `jump-start`, `jump-end`, `jump-none`, `jump-both`, as
  CSS `steps()`); an object `{ "linear": [[0, 0], [0.3, 0.8], [1, 1]] }`
  (straight lines through `[input, output]` points, inputs from 0 to 1 in
  order, as CSS `linear()`); or an object
  `{ "spring": { "stiffness": 170, "damping": 26, "mass": 1 } }`. A spring
  is solved analytically and rescaled so it settles exactly at the next
  keyframe; its parameters shape the overshoot and bounce, not the duration.

### Animation

Motion can also be written the way a stylesheet writes it. A top-level
`keyframes` map holds the rules, and a clip's `animation` field plays one
or more of them:

```json
"keyframes": {
  "slide-in": { "from": "translate: -656px", "to": "translate: 0" },
  "fade-in":  { "from": "opacity: 0", "to": "opacity: 1" },
  "fade-out": { "from": "opacity: 1", "to": "opacity: 0" }
},
...
"animation": "slide-in 0.5s ease-out, fade-in 0.3s, fade-out 0.3s 3.7s"
```

A rule maps an offset (`from`, `to` or a percentage) to a declaration
block. The properties a block may set are the ones the renderer animates:

| Property | Effect |
| --- | --- |
| `transform` | A list of `translate`, `translateX`, `translateY`, `scale`, `scaleX`, `scaleY` and `rotate` functions. Functions of the same kind compose: translations add, scales multiply, rotations add. A distance is in pixels or a percentage of the box it moves (see below). |
| `translate`, `scale`, `rotate` | The same three as separate properties, as CSS also allows: `"translate: 10px 20px"`, `"scale: 2"`, `"rotate: 45deg"`. |
| `opacity` | A number or a percentage. |

A percentage distance is a share of the box being moved, as in CSS:
the element carrying the animation when a rule comes from markup, and the
clip's own box when the clip's `animation` plays it. So
`translateX(-100%)` slides a card in by exactly its own width, whatever
that turns out to be. A clip whose size is only known once its file is
open (a video, an image, a text run) has no box to take a share of, and
a percentage there is E442.

A property interpolates between the offsets that set it, and holds its
first and last value outside them, which is CSS's `animation-fill-mode:
both`; there is no `fill-mode` field yet.

What a rule sets is laid over what the clip already has. A translation
adds to `transform.position`, a scale multiplies `transform.scale`, a
rotation adds to `transform.rotation`, and `opacity` replaces the clip's,
since it is the same property either way. The clip's own value must be
constant where a rule drives it; setting it with `keyframes` as well is
an error (E443).

The `animation` value is the CSS shorthand, in any order, with the parts
geneva understands:

| Part | Default | Notes |
| --- | --- | --- |
| rule name | required | A key of the document's `keyframes` (E440). |
| duration | required | A time with its unit: `0.5s`, `500ms`. Must be more than zero. |
| delay | `0s` | The second time in the entry. |
| timing function | `linear` | `linear`, `ease`, `ease-in`, `ease-out`, `ease-in-out`, `step-end`, `cubic-bezier(x1, y1, x2, y2)`, `steps(n[, position])`, `linear(...)` with its points as CSS writes them (`linear(0, .5 30%, 1)`), or geneva's own `spring(stiffness[, damping[, mass]])`. It applies between each pair of offsets. |
| iteration count | `1` | A number, or `infinite`, which runs until the clip ends. |
| direction | `normal` | `normal`, `reverse`, `alternate`, `alternate-reverse`. |

Several animations are separated by commas. Two of them may drive one
property as long as their times do not collide (E444), which is how a
fade-in and a fade-out live together above. Where one run ends and the
next begins on a different value, the value snaps rather than ramping
back.

A rule can also come from the markup a clip draws: `@keyframes` inside an
[HTML source](#markup) is in scope for that clip, and if its outermost
element carries an `animation`, that is what the clip plays. A rule named
in both places is the document's, and says so (W451).

A property set at only one offset of a rule has nothing to interpolate
with, so it is dropped; a rule where nothing is left is W440. That is what
makes `to { transform: none }` mean "back where it started" rather than
"and reset the scale and rotation too".

Everything here resolves to ordinary keyframe tracks, so an animated clip
is composited exactly as a hand-written one is, and `--show-timeline`
prints the document as it was written.

## Document structure

### Top level

| Field | Required | Description |
| --- | --- | --- |
| `geneva` | yes | Format version, `"0.3"`. A `"0.1"` or `"0.2"` document is read as it is: 0.2 added optional clip fields (`crop`, `effects`, `mask`, `speed`), 0.3 adds the optional `outputs` map. |
| `output` | yes | Frame size, rate, duration, background, color, audio and encoding settings. |
| `outputs` | no | Map of name to [output entry](#outputs): the files one render writes from the composition, when there is more than one. |
| `assets` | no | Map of asset id to asset. |
| `compositions` | no | Map of name to reusable composition. |
| `keyframes` | no | Map of name to an [animation rule](#animation): offset to declaration block. |
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
| `audio.loudness.target_lufs` | no | none | Integrated loudness to bring the mix to, in LUFS, measured as ITU-R BS.1770-4 (EBU R128) measures it: K-weighted and gated over the whole output. The mix is read once to measure it, then written with one gain. Platforms normalise to -14 (YouTube, TikTok, Instagram) or ask for -16 (podcasts); broadcast asks for -23 or -24. -40 to -5 (E423). A silent mix is left as it is. Reading the mix twice decodes every audio source twice. |
| `audio.loudness.true_peak_dbtp` | no | -1 | Ceiling for the true peak (four times oversampled) in dBTP, held by a limiter after the gain: 5 ms of look-ahead, 100 ms release. -20 to 0 (E423). On material whose loudness is carried by its peaks, the limiter takes what the gain added and the output lands under the target; the render's note says by how much. |
| `audio.hygiene` | no | `false` | Audio hygiene for speech, fixed curves: a second-order high-pass at 80 Hz (rumble, handling noise, plosive energy, DC offset), and notches 2 Hz wide on mains hum, 50 or 60 Hz and the harmonics that show, applied only when the mix has hum (the mix is read once to look for it). On music the high-pass takes the bass under 80 Hz with it. The render's note says what was found. An output with hygiene, denoising or a loudness target is always re-encoded, picture included: nothing copies the video and mixes the audio alone yet. |
| `audio.denoise` | no | `false` | Speech denoising with the DeepFilterNet model embedded in the binary, before hygiene and loudness. Speech only, and it says so: it damages music and overlapping speakers. The mix is processed at 48 kHz as mid and side, so the stereo image is kept, on one core at about 20x the speed of the audio. A binary built without the `denoise` feature refuses the document with E424 before rendering anything. |
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
| `transition` | no | | How this clip arrives from the one before it. See [Transitions](#transitions). |
| `crop` | no | the whole source | A rectangle of the source that becomes the clip's box; see below. |
| `fit` | no | `contain` for video, `none` otherwise | `none`, `contain`, `cover`, `fill`: how the source box is sized to the frame before the transform. |
| `effects` | no | `[]` | Effects on the placed picture, in order; see below. |
| `mask` | no | | A shape cut from the clip's box, or a luma image over it; see below. |
| `speed` | no | `1` | How fast the source plays: `2` is twice as fast, `0.5` half speed. The clip lasts its source range divided by it; a video's audio is resampled, so the pitch follows; the clip's own keyframes stay in output time. A clip with a speed is always composited. |
| `animation` | no | | [Animation](#animation) from the document's `keyframes`, spelled like the CSS shorthand. |
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
| `position` | `"center"` | Animatable [point](#points) in the output frame. |
| `anchor` | `"center"` | [Point](#points) on the clip's box; percentages refer to the box. |
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

`html`. See [Markup](#markup).

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `html` | unless `asset` | | The markup, written in the document. |
| `asset` | unless `html` | | Id of an asset of kind `html`, read as the markup. Give one of `html` and `asset`, not both (E450). |
| `css` | no | | A stylesheet applied after any `<style>` in the markup, so it wins ties. |
| `width` | no | the frame width | Box width: a length, a percentage of the frame, or `"auto"` to fit the content. |
| `height` | no | the frame height | Box height, same forms. `"auto"` fits the content, so a card sizes itself to its text. |

`text`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `text` | unless `words` | | The text. |
| `words` | no | | Timed words, in order and non-overlapping (E411). Each is `{ "text", "start", "end" }` or its fields in order, `["word", start, end]` or `["word", start]`. Times are clip-relative. Without an `end` a word is current until the next one starts; the last word needs its own (E102). |
| `highlight` | no | | Style overrides for the word whose range contains the current time. |
| `font` | no | system sans-serif | Id of a `font` asset, a family name, or the CSS `font` shorthand (`"600 40px/1.2 Inter"`), whose parts fill in `size`, `weight`, `italic` and `line_height` unless those are set. A font asset also supplies its weight and style unless set here. |
| `size` | no | `48` | Font size in pixels. |
| `weight` | no | `400` | 100 to 900. |
| `italic` | no | `false` | |
| `color` | no | `white` | Animatable. |
| `fill` | no | | A gradient over the glyphs in place of `color`: `"linear-gradient(90deg, #7A51CF, #C28072)"` or `"radial-gradient(...)"` as CSS writes them (see [Gradients](#gradients)), or `{ "gradient", "width", "height", "x", "y" }`. The gradient is drawn on a tile the size of the text's box unless `width` and `height` say otherwise, and the tile repeats. `x` and `y` are where the tile starts, in pixels from the box's top left, and are animatable: a tile twice the text's width with `x` keyframed from `0` to minus the text's width sweeps the gradient across. A `highlight` keeps the base fill unless it sets a `color` or a `fill` of its own. An outline or shadow keeps its own colour. |
| `letter_spacing` | no | `0` | Pixels. |
| `max_width` | no | output width | Wrap width. |
| `align` | no | `center` | `left`, `center`, `right`. |
| `line_height` | no | `1.2` | Multiple of the font size. |
| `padding` | no | `0` | Space between text and background box, as a number or `"8px"`. |
| `background` | no | none | Background box color. |
| `radius` | no | `0` | Background box corner radius. |
| `outline` | no | | `{ "color", "width" }`, or the shorthand `"2px black"` (width and color in either order). |
| `shadow` | no | | `{ "color", "x", "y", "blur" }`, the `text-shadow` shorthand `"0 2px 8px #0008"` (x, y, optional blur, optional color), or a list of either, front to back as CSS lists them; a string may list several with commas. Each of the four is animatable in the object form; the shorthand is a constant. The image is sized for the furthest reach of any shadow over the clip, so a shadow that grows or moves does not shift the text. |

`captions`

One clip that becomes one clip per cue on the timeline, so the frames
between cues can still be copied rather than composited.

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `asset` | yes | | Id of an asset of kind `captions` or `subtitle`: a `.srt`, a `.vtt`, or the `.json` a speech recogniser writes. Unreadable, missing or wordless files are an `E453`. |
| `position` | no | `bottom` | `bottom`, `top` or `center`. |
| `margin` | no | the title-safe inset | Distance from the top or bottom edge; a length or a percentage of the frame height. |
| `safe` | no | `5` | Title-safe inset as a percentage of the frame height, used as the default margin and checked against it (`N453`). `0` turns the check off. |
| `follow_file` | no | `true` | Take `line`, `position`, `align` and `size` from a WebVTT cue that sets them. `false` places every cue the same way. |
| `max_lines` | no | `2` | Lines per cue, when the cues are grouped from a word file. |
| `min_duration` | no | `1.2s` | How long a cue stays up at least, when grouping. |
| `merge_gap` | no | `0.1s` | Gaps up to this are closed rather than left blank, when grouping. |
| `style` | no | | A `text` source's fields, minus `text` and `words`: `font`, `color`, `highlight`, `outline`, `shadow`, `background`, `max_width`, `align` and the rest. |

A word file gives every cue its `words`, so `style.highlight` picks out
the word being said. SubRip and WebVTT time whole cues, so a `highlight`
on one is a `W453` rather than a silent difference.

Word files are read forgivingly: whisper's `{"segments": [{"words": [...]}]}`,
a bare `{"words": [...]}` or a bare list all work, `word` and `text` are
both read as the word, and every other key (`probability`, `seek`,
`tokens`, WhisperX's `score`) is ignored. Times are seconds; times that
are plainly milliseconds are an `E453` rather than a caption track that
starts twenty minutes in.

The same file can instead be muxed as a stream rather than drawn into the
picture: see [`subtitles[]`](#subtitles).

#### CSS shorthands

Where an author would write CSS, the timeline takes the same strings, for
the values that map one to one:

| CSS | Timeline field | Example |
| --- | --- | --- |
| `text-shadow` | `shadow` | `"0 2px 8px #0008"`, `"0 0 4px #fff8, 0 0 12px #0ff8"` |
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

## Markup

A clip with a source of kind `html` draws a box of HTML and CSS. It is not
a browser: there is a strict HTML parser, a CSS subset, and flexbox and
block layout from
[taffy](https://github.com/DioxusLabs/taffy). Everything it cannot do it
refuses by name, so a document that would look different in a browser says
so rather than drawing something else.

```json
{ "kind": "html", "asset": "card", "width": 560 }
```

```html
<style>
  .card {
    display: flex; flex-direction: column; gap: 2px;
    padding: 16px 28px;
    background: #0b1016d9;
    border-radius: 8px;
    border-left: 5px solid #4ade80;
  }
  .card h1 { margin: 0; font: 700 32px Liberation Sans; color: #ffffff }
  .card p  { margin: 0; font: 400 20px Liberation Sans; color: #9fb0bf }
</style>

<div class="card">
  <h1>Dragon CRS-17</h1>
  <p>Berthing at the ISS &middot; NASA</p>
</div>
```

The box the markup is drawn into is the drawing surface, the way `<body>`
is the page: block layout, and by default the size of the frame, so CSS
places things in the picture the way it places them on a page and the
clip needs no `transform`. Set either side to `"auto"` to fit the content
instead.

A surface the size of the frame costs no more than a small one to
composite: the painter reports the rectangle it actually marked, and the
compositor reads that rather than the whole surface.

### Files it points at

`<img src="logo.png">` and `<link rel="stylesheet" href="house.css">`
take paths, relative to the markup itself, as they are on a page. A card
kept in `cards/lower-third.html` finds `cards/logo.png` with
`src="logo.png"`, and opening that file in a browser shows the same
picture.

The rule every asset path follows applies here too: no leading `/`, no
`..`, no drive letter, no URL. Anything that would leave the asset root
is E452, and so is a file that is not there. Both are checked while the
document is validated, so a missing picture is an error before anything
is drawn rather than a hole in the frame. `--assets DIR` moves the root,
and the paths move with it.

`<link>`, `<meta>`, `<base>` and `<title>` belong in the head and draw
nothing; they never become boxes, so they cannot take a slot in a flex
row.

### What it parses

Markup must be well formed: every element that is not void
(`br`, `hr`, `img`, `input`, `link`, `meta` and the rest) is closed, or
closes itself with `/>`. There is no tag inference and no error recovery;
a mismatch is E451 with a line and column. `<style>` is collected, a stylesheet
`<link>` is read, comments and doctypes are skipped, `<script>` is an
error. Named entities
cover the common set (`&amp;`, `&nbsp;`, `&middot;`, `&mdash;`, …) along
with `&#39;` and `&#x41;`.

Selectors are type, class, id and `*`, joined by the descendant and child
(`>`) combinators, in a comma-separated list. There are no pseudo-classes,
no attribute selectors, no sibling combinators and no at-rules; each is
E451 by name. The cascade is the usual one: specificity as
(ids, classes, types), source order to break ties, `!important` above
both, a `style` attribute above every selector, and the text properties
inherited by children. A handful of tags carry a user-agent style that an
author overrides freely: `h1`, `h2`, `h3`, `b`, `strong`, `i`, `em`,
`small`.

### What it draws

| Group | Properties |
| --- | --- |
| Box | `display` (`flex`, `block`, `none`), `position` (`relative`, `absolute`), `top`, `right`, `bottom`, `left`, `inset`, `width`, `height`, `min-width`, `min-height`, `max-width`, `max-height`, `aspect-ratio`, `box-sizing`, `overflow` (and `-x`, `-y`) |
| Spacing | `margin`, `padding` and their per-side forms and one-to-four-value shorthands |
| Flex | `flex-direction`, `flex-wrap`, `justify-content`, `align-items`, `align-self`, `align-content`, `gap`, `row-gap`, `column-gap`, `flex-grow`, `flex-shrink`, `flex-basis`, `flex` |
| Border | `border`, `border-top`, `border-right`, `border-bottom`, `border-left`, `border-width`, `border-color`, `border-style` (`solid`, `none`), `border-radius` |
| Paint | `background`, `background-color` (a colour or a gradient), `background-size`, `background-position`, `background-clip` (`text` or `border-box`, with or without `-webkit-`), `opacity`, `filter` (`blur()` only), `clip-path` (`polygon()` only, or `none`), `box-shadow` (a list; `inset` is not drawn) |
| Text | `color`, `-webkit-text-fill-color` (read as `color`), `font`, `font-family`, `font-size`, `font-weight`, `font-style`, `line-height`, `letter-spacing`, `text-align`, `white-space`, `text-shadow` (a list) |
| Motion | `animation` and its longhands `animation-name`, `-duration`, `-delay`, `-timing-function`, `-iteration-count`, `-direction`, `-fill-mode` |

Lengths are `px`, `em`, `rem` and `%`; `em` is the element's own font
size, settled before anything else uses it, and a percentage `font-size`
or `line-height` is of the inherited one. Percentage widths, heights,
margins, padding, gaps and insets resolve against the containing block,
as in CSS. `box-sizing` is `content-box` by default, as in CSS, so
padding and border are added to a width rather than taken out of it.
Colours are the ones the rest of the format takes. Anything else is W450:
the declaration is skipped and the message names it, so the rest of the
document still draws.

### Gradients

`background` takes `linear-gradient()` and `radial-gradient()` as well as
a colour.

```css
background: linear-gradient(160deg, #07100C, #123a2c);
background: linear-gradient(to bottom right, #00EEE1, #FFD233 70%);
background: radial-gradient(circle at 30% 40%, #00EEE1, #00EEE100);
```

A linear gradient takes an angle (`deg`, `turn` or `rad`), or `to` and a
side or corner, and runs down the box when it says neither. A corner's
angle depends on the box's proportions, as in CSS, so a wide box points
the ramp more sideways. A radial gradient is a circle or an ellipse, `at`
a position given in percentages or keywords, centred when it says
neither.

Stops are a colour and an optional position. Positions left out are
spread evenly, the first at the start and the last at the end, and one
that runs backwards is pulled up to the one before it so the ramp never
reverses. A colour may carry two positions, which is a band of flat
colour between them.

Inside a markup box, colours blend the way a browser blends them: a
gradient's stops, a translucent box over another, a shadow, a blur and
the edges of text are worked out on sRGB-encoded values with alpha
premultiplied, so a 30% teal over a dark ground comes out as it does on
a page, and a ramp from black to white has a browser's midpoint. The
finished box is converted to linear light once and composited onto the
frame like every other clip, so the clip's own transform, opacity, blend
mode and transitions stay in the linear light the rest of geneva works
in (see [color.md](color.md)). One thing is still done in linear light:
the glyphs of a text are drawn by the same engine as a `text` clip, so
a gradient `fill` on text mixes its stops in linear light before the
text joins the page.

A background is drawn on a tile that repeats across the box, as in CSS:
`background-size` is the tile, one or two of a length, a percentage or
`auto`, and `background-position` is where it starts, lengths,
percentages or the side keywords. `cover`, `contain` and more than one
layer are named as undrawn.

`background-clip: text` (and `-webkit-background-clip: text`) fills the
element's glyphs, and its descendants', with the background instead of
the box, the way a browser does with `-webkit-text-fill-color:
transparent`. The tile is sized against each text run's own box rather
than the element's, which is the same box for an element that holds one
line of text and differs for one whose text wraps. This is the markup
form of a text clip's `fill`.

What is not drawn, each named rather than skipped: `conic-gradient`, any
`repeating-` gradient, the size keywords (`closest-side` and the rest; a
radial gradient is always sized to the farthest corner), colour hints,
and `url()` background images. A gradient is clipped to its own tile, as
in CSS, so a radial that has not reached its last stop by the edge stops
there with a visible edge.

### Motion

`@keyframes` in the markup's stylesheet are in scope for the clip that
draws it, and an `animation` on the **outermost element** is what that
clip plays, so a file that moves in a browser moves here too:

```html
<style>
  @keyframes slide-in { from { transform: translateX(-656px) } to { transform: none } }
  .card { animation: slide-in 0.5s ease-out; /* ... */ }
</style>
<div class="card">...</div>
```

The clip's own `animation` replaces it, since only the document knows
where the clip sits in time; when both are set, W451 says which one won.
The clip plays `transform` and `opacity`; a rule it is given that sets
anything else is E442, and it holds the animation's start and end
outside its runs whatever `animation-fill-mode` says.

An `animation` on an element **inside** the outermost one is played
there, the way a browser plays it. The element is composited as a group:
its subtree is painted into a buffer of its own and laid onto the
picture with the transform, opacity and blur the animation gives it at
that moment. Several animations on one element stack in the order they
are written, and a rule that sets only `to` starts from whatever the
rules under it, or the style, leave the element at; `animation-delay`,
`-duration`, `-iteration-count`, `-direction` and `-fill-mode` are read
as longhands too, comma lists and all. A keyframe inside sets
`transform`, `opacity`, `filter: blur()`, `color`, `text-shadow`,
`letter-spacing`, `width`, `height`, `max-width`, `min-width`,
`background-position` or `clip-path`; anything else is E442. `color` and
`text-shadow` reach the element's text and the descendants that
inherited them. Two polygons with the same number of points mix point by
point; any other pair, as in CSS, is a step at the halfway mark. A length
and a percentage do not mix either (a browser folds them into a `calc()`;
here the change is a step), except that a zero mixes with either.

A markup box whose elements do not move is laid out and painted once per
clip and reused for every frame. One with an animation inside is painted
at each frame, and laid out again at each frame when the animation sets
`width`, `height`, `max-width`, `min-width` or `letter-spacing`, since
those move the boxes around it. That is what a typing effect that grows
a character's box costs; a card on screen for a minute with a fade
inside it costs a paint per frame and one layout.

### What it does not do

- **A `text-shadow` does not move the text.** Each shadow in the list is drawn behind the
  glyphs, the first on top, and spills outside the box, as on a page, but layout is done as
  though it were not there. It is clipped at the edge of the clip's own
  box, so a glow on text at the very edge of a card is cut off.
- **No inline layout.** An element's text is one paragraph, and a child
  element is a box of its own, so a `<span>` inside a sentence becomes a
  block rather than flowing with the words around it.
- **`opacity` groups**, as in CSS: a box with an opacity below one is
  painted into a buffer of its own with its children at their own
  opacity, and the buffer is laid onto the picture at the box's, so
  overlapping children do not show through each other. The same buffer
  carries `filter: blur()`, which is the only filter drawn, and
  `clip-path: polygon()`, which is the only clip path: its points are
  lengths or shares of the border box, may lie outside it, and cut the
  box and its children after the blur and before any transform. A group
  is painted whole where CSS puts a stacking context, so a child inside
  one cannot rise above a box outside it with `z-index`.
- There is no `float`, no grid, no transition and no media
  query. The clip's own `transform` and `animation` move the whole box.
- **Nothing is fetched over the network.** A path is a file; a URL is
  E452. Elements with a renderer of their own (`<iframe>`, `<svg>`,
  `<canvas>`, `<video>`, `<object>`, `<embed>`) are W450. A rule in the
  markup's own `<style>` that matches no element is W452, so a misspelt
  class name is named rather than quietly doing nothing.
- **`z-index` orders the painting**, on a positioned box or a flex item,
  which is where CSS applies it. A box with one is painted whole, where
  its number puts it; negative numbers go under everything in flow. A box
  without one is painted in document order, and one deeper in can still
  rise above an uncle, since a plain box opens no stacking context of its
  own. A group (opacity, filter or an animation) does open one. Setting
  `z-index` where it does not apply is W454 rather than a quiet
  difference.
- `body` and `html` both select the box the markup is drawn into, which
  is the clip's own box. A `background` or `border` on it fills that box,
  and `padding` goes inside it, the way it does against a page. Left
  unstyled the box marks no pixels, so only the content counts towards
  what is composited.

Layout and painting depend on time only through an animation on an
element inside the markup; see [Motion](#motion) for what each costs.

### Transitions

A `transition` on a clip says how it arrives; a `transition_out` says how
it leaves. Between two clips on a layer they overlap for `duration`, and a
clip with no explicit `start` is moved that much earlier to make the
overlap, so the previous clip must be long enough to cover it (E306).

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `kind` | yes | | `crossfade` or `fade`. |
| `duration` | yes | | The overlap between a pair of clips, or the ramp at the head or tail of a layer. |
| `color` | no | `black` | The color a `fade` dips through. On a `crossfade` it is a W304, since nothing would show it. |
| `ease` | no | `linear` | Shape of the ramp, taking the same values a keyframe's easing takes. `ease-in-out` is the usual choice for a slow dissolve. |

`crossfade` brings the arriving clip up over the one leaving, which stays
at full opacity underneath. Both are on screen at once, so the picture
dissolves. The sound crosses at constant power (each gain is the square
root of its linear ramp), which holds the level across the overlap; a
linear pair would dip about 3 dB in the middle on material that is not
correlated.

`fade` dips through a color instead. The leaving clip fades out over the
first half of the overlap, the arriving one fades in over the second, and
the color covers the frame in between, strongest at the midpoint. Only
one clip is ever visible. The sound follows the picture: it reaches
silence at the midpoint and comes back.

At the head or tail of a layer there is no second clip, so the ramp runs
the whole `duration` rather than handing over in the middle. A `fade`
opening a layer comes up out of its color and one closing a layer goes
out to it, which is how a piece fades up from black and fades out again.
A `crossfade` there has no colour to use, so it fades against whatever is
behind: the layers below, or `output.background`.

```json
{ "source": { "kind": "video", "asset": "a" },
  "transition":     { "kind": "fade", "duration": "1s" },
  "transition_out": { "kind": "fade", "duration": "1s", "ease": "ease-in-out" } }
```

Where another clip follows, that clip's `transition` already covers the
join, and setting `transition_out` as well is an E307. Keep one.

Two things to know. The dip color covers the whole frame for the length
of the transition, including layers below the one it is on, because it is
the picture that dips and not one layer of it. And a fade on a layer
above the first makes the whole composition take the compositing path
rather than the overlay path, so none of its frames are copied.

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
can show or hide. That is the whole distinction the format draws between
the two words: a *subtitle* travels alongside the picture as a stream, a
*caption* is drawn into it. The same `.srt` or `.vtt` can do either: here
as a track, or as a [`captions` source](#sources) on a clip. Each track is
one subtitle asset, a SubRip `.srt` or WebVTT `.vtt` file.

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
blend mode and transition, in picture and in sound alike. `geneva render` mixes audio tracks and the audio
of video clips. Hardware encoders are not used yet; `encode.video.hardware`
is accepted and ignored.

Text uses fonts from `font` assets first and falls back to fonts installed
on the system. Output that depends on system fonts can differ between
machines; ship the fonts as assets when the result must be identical.
