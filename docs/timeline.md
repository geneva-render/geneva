# Timeline format 1.1

A timeline is a JSON document that describes a video composition: the
output frame, the assets, the visual layers and the audio tracks. The
editing commands compile to timelines, and a timeline can also be written
directly. This page is the format reference.

- Schema: [`schema/geneva-timeline-1.1.schema.json`](../schema/geneva-timeline-1.1.schema.json), also printed by `geneva schema`.
- Version 1.1 adds `in` and `out` to the [`captions`](#sources) source. A `"1.0"` document is read as it is: within 1.x the format only gains optional fields.
- Unknown fields are errors everywhere; the message lists the allowed names.

## Minimal document

The smallest valid document: three seconds of a solid colour.

```json
{
  "geneva": "1.1",
  "output": { "width": 1280, "height": 720, "fps": 30, "duration": "3s" },
  "layers": [
    { "clips": [ { "source": { "kind": "solid", "color": "#1d2230" } } ] }
  ]
}
```

## Value conventions

### Times

Times and durations accept these forms:

| Form | Example | Meaning |
| --- | --- | --- |
| number | `1.5` | seconds |
| seconds | `"1.5s"` | seconds |
| milliseconds | `"1500ms"` | milliseconds |
| frames | `"45f"` | frames at the output frame rate |
| timecode | `"00:01:02.5"`, `"1:02.5"` | hours, minutes, seconds |

- Kept as exact rationals: `0.1` is exactly one tenth, and a frame at 29.97 fps is exactly 1001/30000 s.
- Never negative.

### Frame rate

- A positive number (`30`, `25`) or a ratio string (`"30000/1001"`).
- `23.976`, `29.97`, `59.94` snap to their 1001-based ratios.

### Lengths

| Form | Meaning |
| --- | --- |
| `120`, `"120px"` | pixels in the output frame |
| `"50%"` | of the output width (horizontal) or height (vertical); in `transform.anchor`, of the clip's box |

Pixel-only fields take `8` or `"8px"`, no percentage: text `size`,
`letter_spacing`, `padding`, `radius`; outline `width`; shadow `x`, `y`,
`blur`; fill `width`, `height`, `x`, `y`; shape `radius`; mask `radius`,
`feather`; blur `radius`; frame and picture sizes (whole numbers only).
Keyframe values take either spelling. A printed timeline writes the
number.

### Points

`transform.position` and `transform.anchor` take a point in any of these
forms:

```json
{ "x": 30, "y": 36 }        [30, 36]        "30 36"        "0% 50%"
```

- A string component is a length or `left`, `right`, `top`, `bottom`, `center`.
- Keywords name their axis and read as in CSS: `"left 36"`, `"36 top"`, `"top 36"`, `"bottom right"`, `"right bottom"`.
- One keyword centres the other axis: `"left"` = `{ "x": "0%", "y": "50%" }`.
- One length applies to both axes: `"12"` = `{ "x": 12, "y": 12 }`.

### Colors

- sRGB: `"#rgb"`, `"#rgba"`, `"#rrggbb"`, `"#rrggbbaa"`, `"rgb(255, 136, 0)"`, `"rgba(255, 136, 0, 0.5)"`.
- Names: `black`, `white`, `red`, `green`, `blue`, `yellow`, `cyan`, `magenta`, `gray`/`grey`, `orange`, `transparent`.
- Animated colors interpolate in linear light.

### Animated values

A property marked *animatable* takes a constant or a `keyframes` object.
The two keyframe forms below are equivalent.

```json
"opacity": { "keyframes": [ { "t": 0, "v": 0 }, { "t": "0.5s", "v": 1, "ease": "ease-out" } ] }
"opacity": { "keyframes": [ [0, 0], ["0.5s", 1, "ease-out"] ] }
```

- `t` is relative to the clip start; strictly increasing.
- The first value holds before the first keyframe, the last after the last.
- `ease` shapes the segment starting at its keyframe:

| `ease` | Meaning |
| --- | --- |
| `linear` (default), `ease`, `ease-in`, `ease-out`, `ease-in-out` | CSS curves |
| `hold` | jump at the next keyframe |
| `{ "cubic-bezier": [x1, y1, x2, y2] }` | CSS `cubic-bezier()` |
| `{ "steps": [n, "jump-end"] }` | CSS `steps()`; `jump-start`, `jump-end`, `jump-none`, `jump-both` |
| `{ "linear": [[0, 0], [0.3, 0.8], [1, 1]] }` | CSS `linear()`: `[input, output]` points, inputs 0 to 1 in order |
| `{ "spring": { "stiffness": 170, "damping": 26, "mass": 1 } }` | solved analytically, rescaled to settle exactly at the next keyframe; parameters shape overshoot, not duration |

### Animation

Motion can also be written as in CSS: rules in a top-level `keyframes`
map, played by a clip's `animation`.

```json
"keyframes": {
  "slide-in": { "from": "translate: -656px", "to": "translate: 0" },
  "fade-in":  { "from": "opacity: 0", "to": "opacity: 1" },
  "fade-out": { "from": "opacity: 1", "to": "opacity: 0" }
},
...
"animation": "slide-in 0.5s ease-out, fade-in 0.3s, fade-out 0.3s 3.7s"
```

A rule maps offsets (`from`, `to`, percentages) to declaration blocks:

| Property | Effect |
| --- | --- |
| `transform` | `translate`, `translateX`, `translateY`, `scale`, `scaleX`, `scaleY`, `rotate`. Same-kind functions compose: translations add, scales multiply, rotations add. |
| `translate`, `scale`, `rotate` | The same as separate properties: `"translate: 10px 20px"`, `"scale: 2"`, `"rotate: 45deg"`. |
| `opacity` | Number or percentage. |

- **Percentage distances** are of the box being moved: the animated element (markup) or the clip's box. `translateX(-100%)` slides by exactly its own width. A clip sized only once its file is open (video, image, text) has no box: E442.
- **Fill**: a clip's `animation` interpolates between the offsets that set a property and holds the first and last values outside them (`animation-fill-mode: both`, not configurable). An `animation` on an element inside markup is plain CSS: `fill-mode: none` by default.
- **Position is one animated point**: `{"keyframes": [["0s", [1072, 14]], ["4s", [-7700, 14]]]}`. `x` or `y` alone takes a constant length only; keyframes under `x` are E103.
- **Composition with the clip**: translation adds to `transform.position`, scale multiplies `transform.scale`, rotation adds to `transform.rotation`, `opacity` replaces the clip's. The clip's own value must be constant where a rule drives it (E443).
- **One-sided properties**: a property set at only one offset is dropped; a rule left empty is W440. So `to { transform: none }` means "back where it started", not "reset scale and rotation".
- Resolves to ordinary keyframe tracks; an animated clip composites like a hand-written one.

The `animation` value is the CSS shorthand, parts in any order:

| Part | Default | Values |
| --- | --- | --- |
| rule name | required | a key of `keyframes` (E440) |
| duration | required | `0.5s`, `500ms`; more than zero |
| delay | `0s` | the second time in the entry |
| timing function | `linear` | `linear`, `ease`, `ease-in`, `ease-out`, `ease-in-out`, `step-end`, `cubic-bezier(x1, y1, x2, y2)`, `steps(n[, position])`, `linear(0, .5 30%, 1)`, `spring(stiffness[, damping[, mass]])`; applies between each pair of offsets |
| iteration count | `1` | a number, or `infinite` (until the clip ends) |
| direction | `normal` | `normal`, `reverse`, `alternate`, `alternate-reverse` |

- Comma-separated animations may drive one property if their times do not collide (E444). Where one run ends and the next starts on another value, the value snaps.
- Rules may also come from the clip's [markup](#markup): its `@keyframes` are in scope, and an `animation` on its outermost element is what the clip plays. A name in both places resolves to the document's (W451).

## Document structure

### Top level

| Field | Required | Description |
| --- | --- | --- |
| `geneva` | yes | `"1.1"`, or `"1.0"`. Anything else is E110. |
| `output` | yes | Frame, rate, duration, background, color, audio, encoding. |
| `outputs` | no | Name to [output entry](#outputs): several files from one render. |
| `assets` | no | Id to asset. |
| `compositions` | no | Name to reusable composition. |
| `keyframes` | no | Name to [animation rule](#animation). |
| `layers` | no | Visual layers, bottom to top. |
| `audio` | no | Audio-only tracks. |
| `subtitles` | no | [Subtitle tracks](#subtitles), muxed as text streams. |

### `output`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `width`, `height` | yes | | Frame size in pixels. Odd values: W401. |
| `fps` | yes | | Frame rate. |
| `duration` | no | end of the last clip | Clips past it are cut (W301). A partial last frame counts as a frame, unless it would start less than 1 ms before the end. |
| `background` | no | `"black"` | Clear color; `"transparent"` for alpha output. |
| `color` | no | BT.709 SDR, limited range | Color tags; see [color.md](color.md). `pq`, `hlg` need a ten-bit codec: `h265`, `av1`, `vp9`, `prores` (E420). |
| `audio.sample_rate` | no | 48000 | |
| `audio.channels` | no | 2 | 1 or 2. Source sound is copied when rate, channels and `encode.audio.codec` match it and no audio bitrate is set; otherwise encoded, the picture still copied where nothing else changes it. More than two channels is copied as is unless something re-encodes the sound. |
| `audio.loudness.target_lufs` | no | none | Integrated loudness, ITU-R BS.1770-4 / EBU R128 (K-weighted, gated, whole output). Measured in a first read, applied as one gain. -40 to -5 (E423). Silent mixes untouched. Decodes every audio source twice. Usual targets: -14 (YouTube, TikTok, Instagram), -16 (podcasts), -23/-24 (broadcast). |
| `audio.loudness.true_peak_dbtp` | no | -1 | True-peak ceiling (4× oversampled), held by a limiter after the gain: 5 ms look-ahead, 100 ms release. -20 to 0 (E423). Peaky material can land under the target; the note says by how much. |
| `audio.hygiene` | no | `false` | Speech cleanup: 2nd-order high-pass at 80 Hz; 2 Hz notches on 50/60 Hz mains hum and visible harmonics, only when hum is found (one extra read). Removes bass under 80 Hz on music. The note says what was found. |
| `encode.container` | no | from the extension | Video: `mp4`, `mov`, `mkv`, `webm`, `mxf`. Audio only: `m4a`, `ogg`, `flac`, `wav`, `mp3`. `image-sequence`: one PNG/JPEG per frame, path a pattern like `frames/%04d.png`, no audio. |
| `encode.video.codec` | no | `h264` (`vp9` webm, `dnxhd` mxf, `png` image sequence) | `h264`, `h265` (hardware encoder only), `vp9`, `av1`, `prores` (10-bit 4:2:2; 4:4:4 for 4444), `dnxhd` (8-bit 4:2:2, HQX 10-bit, 444; at least 256×120), `png`, `mjpeg`. See [containers and codecs](cli.md#containers-and-codecs). |
| `encode.video.profile` | no | `hq` prores, `dnxhr-hq` dnxhd | ProRes: `proxy`, `lt`, `standard`, `hq`, `4444`, `4444-xq`. DNxHR: `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq`, `dnxhr-hqx`, `dnxhr-444`. Must match the codec (E421). |
| `encode.video.crf` | no | per codec | Constant quality; lower is better. |
| `encode.video.preset` | no | per codec | `ultrafast` … `veryslow`; ignored without presets. |
| `encode.video.hardware` | no | `auto` | `auto`, `never`, `require`. |
| `encode.video.keyframe_interval` | no | encoder's | Seconds; 2 for network playback. Re-encodes. |
| `encode.video.max_bitrate_kbps` | no | none | Ceiling over a 2 s buffer; constant quality until it bites. x264, AV1: held. VP9: libvpx constrained-quality average. VideoToolbox: only with `bitrate_kbps`. NVENC, OpenH264: ignored, reported. Re-encodes. See [rate control](cli.md#rate-control). |
| `encode.video.bitrate_kbps` | no | none | Average target: bitrate mode for hardware encoders; x264 ignores it. Set by `--budget`. Re-encodes. |
| `encode.video.level` | no | encoder's | H.264/H.265 level, e.g. `"4.1"`. |
| `encode.video.tune` | no | none | x264 names: `film`, `animation`, `grain`, `stillimage`, `fastdecode`, `zerolatency`. VP9: `film`. AV1: `fastdecode`. NVENC, VideoToolbox, Media Foundation: `zerolatency`. Others ignore it, reported. |
| `encode.video.fixed_keyframes` | no | `false` | Keyframes on the interval only, never at scene changes (streaming, segmenters). Needs `keyframe_interval` (E422). Not OpenH264 (reported). |
| `encode.video.chunks` | no | `auto` | Stretches encoded in parallel, joined without re-encoding. `auto`: VP9 and OpenH264 only, half the cores, never hardware encoders. A number forces it; `1` disables. Boundaries on clip starts or the keyframe grid. Not with bitrate settings, smart cut or image sequences. Reported. |
| `encode.fast_start` | no | `true` | MP4/MOV/M4A index at the front. |
| `encode.audio.codec` | no | `aac` (`opus` webm/ogg, `flac` flac, `pcm` wav, `mp3` mp3, `pcm24` mxf) | `aac`, `opus`, `mp3`, `vorbis`, `flac`, `alac`, `ac3`, `pcm` (16-bit), `pcm24`. |
| `encode.audio.bitrate_kbps` | no | 160 | Encodes the sound rather than copying it. |

An output with `hygiene` or a loudness target has its sound encoded; the
picture is still copied where nothing else re-encodes it.

### `outputs`

Several files written by one render from the same composited frames:
renditions at different sizes, a poster, seek sprites, the audio alone. `geneva render -o DIR` writes them
into `DIR`. Frames are composited once and scaled per entry; sources are
decoded once.

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
| `kind` | all | required | `video`, `poster` (one still; several entries for several stills), `sprites` (thumbnail sheet + WebVTT map; one per document, E434), `audio`. |
| `path` | all | entry name + `.mp4` (video), `.jpg` (poster, sprites), `.wav` (audio) | Plain file name, no directories (E431). Video: `mp4`, `mov`, `mkv`, `webm`, `mxf`; pictures: `jpg`, `jpeg`, `png`; audio: `wav`, `m4a`, `mp3`, `flac`, `ogg`. No two entries on one file (E432). Sprites also write `<name>.vtt`. |
| `width`, `height` | video, poster, sprites | canvas size; sprites: 90 px high tiles | One of the two keeps the aspect (video rounded to even; W401 when odd). Both in another shape stretch the whole canvas into it (W406); a different framing, such as a square cut of a vertical video, needs a document with a canvas of that shape. Sprites: one tile. |
| `encode` | video, audio | `output.encode`; audio: container defaults | Same block as `output.encode`. |
| `audio` | video, audio | `output.audio` | Rate and channels. |
| `at` | poster | first clear frame | A [time](#times) inside the composition (E433). Default: first frame after the opening (5% in, at least 20 frames) that is not dark and follows motion; else 10% in. |
| `every` | sprites | ~100 tiles, at least 1 s apart | E433 when zero or negative. |
| `columns` | sprites | 10 | E433 when zero. |

- A field not belonging to the kind: E430.
- A `video` entry at canvas size with no encode settings can be [stream-copied](architecture.md#stream-copy); others are encoded, each in its own thread. `--crf`, `--preset`, `--exact` encode every video entry.
- With no video entry, only the frames the pictures need are composited.

### `assets`

Every file the timeline uses is declared here and referred to by id.
Clips never contain paths.

| Field | Required | Description |
| --- | --- | --- |
| `src` | yes | Relative to the asset root (the timeline's directory, or `--assets`). Absolute paths and `..`: E202. |
| `kind` | no | `video`, `image`, `audio`, `font`, `subtitle`, `captions` (word or cue file), `html`. Inferred from the extension (E203 when it cannot be). |
| `color` | no | Color tag overrides for untagged or mistagged files. |

### `compositions`

A nested timeline for reuse, placed with a `composition` source any
number of times, each with its own timing and transform.

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `width`, `height` | yes | | Frame size; percentages inside refer to it. |
| `background` | no | `transparent` | |
| `layers` | yes | | As at the top level. Audio tracks are top-level only. |

- Times inside are relative to the clip showing it.
- Length: the end of its last fixed-length clip, unless the clip sets `duration`; open-ended clips inside run for the clip's length.
- Nesting up to 8 levels; never itself (E207).

### `layers[]`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `id` | no | positional | Name in diagnostics. |
| `enabled` | no | `true` | `false` skips the layer. |
| `clips` | yes | | In time order. |

### `layers[].clips[]`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `id` | no | positional | Name in diagnostics. |
| `source` | yes | | See [Sources](#sources). |
| `start` | no | end of the previous clip | |
| `duration` | no | see [timing rules](#timing-rules) | |
| `transition` | no | | Arrival from the previous clip; see [Transitions](#transitions). |
| `transition_out` | no | | Exit of a layer's last clip. |
| `crop` | no | whole source | See [`crop`](#crop). |
| `fit` | no | `contain` for video, `none` otherwise | `none`, `contain`, `cover`, `fill`: sizing to the frame before the transform. W404 flags a picture larger than the frame or a video left in a corner. |
| `effects` | no | `[]` | See [`effects`](#effects). |
| `mask` | no | | See [`mask`](#mask). |
| `speed` | no | `1` | `2` twice as fast, `0.5` half. Length is the source range divided by the speed; audio resampled (pitch follows); clip keyframes stay in output time. Always composited. |
| `animation` | no | | See [Animation](#animation). |
| `transform` | no | centred | See [`transform`](#transform). |
| `opacity` | no | `1` | Animatable, 0 to 1. |
| `blend` | no | `normal` | `normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `difference`, `soft-light`, `add`. Linear light. |

### `crop`

A rectangle of the source that becomes the clip's box, applied before
`fit` and the transform. `"crop": { "x": 420, "width": 1080 }` on 1920×1080 gives a
1080×1080 clip.

| Field | Default | Description |
| --- | --- | --- |
| `x`, `y` | `0` | Top left, in source pixels or percentages of the source. |
| `width`, `height` | rest of the source | |

- Must lie inside the source as written (E402); clamped to the real size; an empty rectangle paints nothing.
- Anchor percentages refer to the cropped box.
- A plain cropped video keeps the direct path (scaled straight from the decoder).

### `effects`

Effects apply to the placed picture: after the transform (sizes are in
output pixels), before opacity and blending. The one effect is `blur`. A
clip with effects is always composited.

`blur`

| Field | Default | Description |
| --- | --- | --- |
| `radius` | | Gaussian standard deviation in output pixels, as CSS `blur()`. Animatable; 0 = none. |

- Spreads past the edges into transparency.
- Wide blurs are computed on a smaller layer; same look, flat cost.

A vertical frame from horizontal footage: the picture whole over a
blurred, scaled-up copy of itself. `--fill blur` builds this:

```json
"layers": [
  { "clips": [ { "source": { "kind": "video", "asset": "v", "audio": false }, "fit": "cover",
      "effects": [ { "kind": "blur", "radius": 45 } ] } ] },
  { "clips": [ { "source": { "kind": "video", "asset": "v" }, "fit": "contain" } ] }
]
```

### `mask`

Limits what a clip shows, by a shape or by an image's luma. Defined in
the clip's box, so it moves, scales and rotates with the clip; sizes are
box pixels or percentages. A masked clip is always composited.

| Field | Default | Description |
| --- | --- | --- |
| `shape` | `rect` | `rect` or `ellipse`, inscribed in the box below. |
| `x`, `y` | `0` | Top left of the shape's box. |
| `width`, `height` | the clip's box | |
| `radius` | `0` | Rectangle corner radius. |
| `feather` | `0` | Soft edge width; 0 is hard. |
| `asset` | | Image asset whose luma × alpha is the coverage, stretched over the box: white shows, black or transparent hides. Shape fields ignored. |
| `invert` | `false` | |

Rounded corners on a picture-in-picture:

```json
{ "source": { "kind": "video", "asset": "cam" }, "fit": "none",
  "mask": { "radius": 24, "feather": 1 },
  "transform": { "position": { "x": "85%", "y": "85%" }, "scale": 0.25 } }
```

### `transform`

The anchor (a point on the clip's box) is placed at `position`; scale and
rotation act around it.

| Field | Default | Description |
| --- | --- | --- |
| `position` | `"center"` | Animatable [point](#points) in the frame. |
| `anchor` | `"center"` | [Point](#points) on the box; percentages of the box. |
| `scale` | `1` | Animatable; number or `{ "x", "y" }`. |
| `rotation` | `0` | Animatable; degrees clockwise. |

### Sources

A clip's `source` defines what it shows. Every source has a `kind`, which
determines its other fields.

`video`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `asset` | yes | | A `video` asset. |
| `in` | no | `0` | Source time at clip start. 0 = first video frame; later-starting audio keeps its offset. |
| `out` | no | end of file | After `in` (E301). |
| `audio` | no | `true` | Mix the file's audio. |

`image`: `asset` (required), an `image` asset. Box = the image's pixel size.

`solid`: `color` (required, animatable). Box = the frame.

`shape`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `shape` | yes | | `rect` or `ellipse`. |
| `width`, `height` | yes | | Box size. |
| `fill` | no | `white` | Animatable color. |
| `stroke` | no | | `{ "color", "width" }`, inside the edge. |
| `radius` | no | `0` | `rect` corner radius. |

`composition`: `composition` (required), a key of `compositions`. Box = its frame.

`html` (see [Markup](#markup))

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `html` | unless `asset` | | Markup inline. |
| `asset` | unless `html` | | An `html` asset. Not both (E450). |
| `css` | no | | Applied after the markup's `<style>`; wins ties. |
| `width` | no | frame width | Length, frame percentage, or `"auto"` (fit content). |
| `height` | no | frame height | Same; `"auto"` sizes a card to its text. |

`text`

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `text` | unless `words` | | |
| `words` | no | | Timed words, ordered, non-overlapping (E411): `{ "text", "start", "end" }`, `["word", start, end]` or `["word", start]`. Clip-relative. Without `end`, current until the next; the last needs one (E102). |
| `highlight` | no | | Style overrides for the current word. |
| `font` | no | system sans-serif | `font` asset id, family name, a comma list of them (`"Inter, Noto Sans Arabic"`, see Fonts under markup), or CSS `font` shorthand (`"600 40px/1.2 Inter"`) filling unset `size`, `weight`, `italic`, `line_height`. A font asset supplies its weight and style unless set. |
| `size` | no | `48` | Pixels. |
| `weight` | no | `400` | 100 to 900. |
| `italic` | no | `false` | |
| `color` | no | `white` | Animatable. |
| `fill` | no | | Gradient over the glyphs instead of `color`: `"linear-gradient(90deg, #7A51CF, #C28072)"`, `"radial-gradient(...)"` ([Gradients](#gradients)), or `{ "gradient", "width", "height", "x", "y" }`. Tile = the text's box unless sized; it repeats. `x`, `y`: tile start from the box's top left, animatable (sweep: tile 2× the text width, `x` keyframed from 0 to minus the width). `highlight` keeps the base fill unless it sets `color` or `fill`. Outlines and shadows keep their colour. |
| `letter_spacing` | no | `0` | Pixels. |
| `max_width` | no | output width | Wrap width. |
| `align` | no | `center` | `left`, `center`, `right`. |
| `line_height` | no | `1.2` | × font size. |
| `padding` | no | `0` | Text to background box. |
| `background` | no | none | Background box color. |
| `radius` | no | `0` | Background box corner radius. |
| `outline` | no | | `{ "color", "width" }` or `"2px black"`. |
| `shadow` | no | | `{ "color", "x", "y", "blur" }`, `"0 2px 8px #0008"`, or a list (commas in a string), front to back. Object fields animatable; the shorthand is constant. The image is sized for the furthest reach over the clip, so a moving shadow does not shift the text. |

`captions`: draws captions from a subtitle file or a speech recogniser's
word file. It expands to one clip per cue, so the frames between cues can
still be copied rather than re-encoded.

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `asset` | yes | | A `captions` or `subtitle` asset: `.srt`, `.vtt`, or speech-recogniser `.json`. Unreadable, missing or wordless: E453. |
| `in`, `out` | no | `0`, end of file | Part of the file to use (1.1). Cues outside it are left out, one crossing an edge is cut, and `in` becomes the clip's start. `out` not after `in`: E301. |
| `position` | no | `bottom` | `bottom`, `top`, `center`. |
| `margin` | no | title-safe inset | From the top/bottom edge; length or % of frame height. |
| `safe` | no | `5` | Title-safe inset, % of frame height; default margin and N453 check. `0` disables. |
| `follow_file` | no | `true` | Use a WebVTT cue's `line`, `position`, `align`, `size`. |
| `max_lines` | no | `2` | Lines per cue (word files). |
| `max_chars` | no | `42` | Characters per line (word files). Under 8: E402. |
| `min_duration` | no | `1.2s` | Minimum cue length (word files). |
| `merge_gap` | no | `0.1s` | Gaps closed up to this (word files). |
| `style` | no | | `text` fields minus `text` and `words`. |

- Cue times are relative to the clip's `start`; a word file timed from its media follows a clip that starts that media at `"4.4s"`.
- A video clip that uses part of its media takes captions with the same `in` and `out`, at the same `start`. Three shots from one film are three captions clips on the same file.
- Cues past the clip's `duration`, or past the end of the output, are left out.
- Grouping (word files): each recogniser segment starts a cue, and so does a pause of 1 s or more between two words; a cue is filled up to `max_lines` × `max_chars` characters.
- Grouping counts characters, not pixels, before any font is loaded, so it is identical on every machine. `style.max_width` never moves a cue boundary; set `max_chars` to what the width holds. Starting point: `max_width / (0.5 × size)`, which errs narrow (700 px of 32 px Liberation Sans: 44 by the formula, 47 to 48 in practice). `validate --probe` reports the cue count and budget.
- Word files give each cue its `words`, so `style.highlight` works. On SubRip/WebVTT a `highlight` is W453.
- Word files accepted: whisper's `{"segments": [{"words": [...]}]}`, `{"words": [...]}`, a bare list. `word` or `text` is the word; other keys are ignored. Times in seconds; millisecond-looking times are E453.
- The same file can be muxed as a stream instead: [`subtitles[]`](#subtitles).

#### CSS shorthands

Text style fields accept the corresponding CSS strings:

| CSS | Field | Example |
| --- | --- | --- |
| `text-shadow` | `shadow` | `"0 2px 8px #0008"`, `"0 0 4px #fff8, 0 0 12px #0ff8"` |
| `outline` (`-webkit-text-stroke`) | `outline` | `"2px black"` |
| `font` | `font` | `"italic 600 40px/1.2 Inter"` |
| `padding` | `padding` | `"8px"` (one value) |
| `font-size`, `letter-spacing`, `border-radius` | `size`, `letter_spacing`, `radius` | `"34px"`, `"1.5px"`, `"3px"` |
| `color`, `background-color` | `color`, `background` | `"#ffdd00"`, `"rgba(0, 0, 0, 0.5)"` |

- The object form is canonical: `--show-timeline` prints `shadow` and `outline` as objects and expands `font`.
- No cascade or selectors; each value belongs to one text source.
- A bad string is E103 with the expected form, pointing at the field (at the source, for a text source's style fields).

### Transitions

`transition` is how a clip arrives, `transition_out` how a layer's last
clip leaves. Two kinds: `crossfade`, and `fade`, which dips through a
colour (black by default).

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `kind` | yes | | `crossfade` or `fade`. |
| `duration` | yes | | Overlap between two clips, or the ramp at a layer's head or tail. |
| `color` | no | `black` | Colour a `fade` dips through. W304 on a `crossfade`. |
| `ease` | no | `linear` | Any keyframe easing; `ease-in-out` for slow dissolves. |

| Kind | Between clips | At a layer's head or tail |
| --- | --- | --- |
| `crossfade` | Arriving clip fades up over the leaving one. Sound crosses at constant power (square-root gains), no mid-point dip. | Fades against what is behind: lower layers or `output.background`. |
| `fade` | Leaving clip fades out over the first half, arriving one in over the second, the colour covering the frame between. Sound dips to silence at the midpoint. | Fades up from, or out to, the colour over the transition's `duration`. |

- Clips overlap by `duration`; a clip without `start` moves earlier to make the overlap. The previous clip must cover it (E306).
- `transition_out` where a next clip exists: E307.
- A `fade`'s colour covers the whole frame: every layer, above and below. To dip the picture under overlays that stay (a ticker, a logo), put the cut in a [composition](#compositions) and the overlays on layers above it; the dip then stays inside the composition.
- A fade on any layer above the first sends the whole composition to the compositing path; no frames are copied.

```json
{ "source": { "kind": "video", "asset": "a" },
  "transition":     { "kind": "fade", "duration": "1s" },
  "transition_out": { "kind": "fade", "duration": "1s", "ease": "ease-in-out" } }
```

### `audio[]` and `audio[].clips[]`

Tracks have `id`, `enabled`, `clips`, as layers do.

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `asset` | yes | | An `audio` or `video` asset. |
| `in`, `out` | no | `0`, end of file | Source range. |
| `start` | no | end of previous clip | |
| `duration` | no | source range | |
| `gain_db` | no | `0` | Animatable. |
| `fade_in`, `fade_out` | no | `0` | Fade lengths. |

- A video clip's own sound has no fades of its own beyond its transitions. To shape it, set the video source's `audio` to `false` and add an audio clip of the same asset with the same `in`, `out` and `start`.
- A video clip's sound is heard wherever the clip is, inside a composition too.
| `speed` | no | `1` | Length is the range divided by the speed; pitch follows. |

### `subtitles[]`

Subtitle tracks muxed into the output as text streams, which players can
show or hide. A *subtitle* travels beside the
picture; a *caption* is drawn into it ([`captions` source](#sources)). The
same `.srt` or `.vtt` serves either.

| Field | Required | Description |
| --- | --- | --- |
| `id` | no | Name in diagnostics. |
| `enabled` | no | `false` leaves it out. |
| `asset` | yes | A `subtitle` asset (`.srt`, `.vtt`). |
| `language` | no | `"en"`, `"pt-BR"`; stored as the container's three-letter code. |
| `title` | no | Shown by players. In MP4 and MOV it is written as the stream's handler name, which is what players show there. |
| `offset` | no | Shifts every cue; negative moves earlier. Cues ending before zero or starting after the output ends are dropped, and one running past the end is cut. |

| Container | Stored as | Tags like `<i>` |
| --- | --- | --- |
| MP4, MOV | 3GPP timed text (`mov_text`) | stripped |
| Matroska | SubRip | kept |
| WebM | WebVTT | kept |
| others | not supported | |

```json
"assets": { "en": { "src": "captions.srt" } },
"subtitles": [ { "asset": "en", "language": "en", "title": "English" } ]
```

## Markup

An `html` source draws a box of HTML and CSS, for titles, cards and
graphics. It is not a browser: a strict HTML parser, a CSS subset aimed at
graphics, and flexbox and block layout
([taffy](https://github.com/DioxusLabs/taffy)). Anything unsupported is
reported by name rather than drawn differently.

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

- The clip's box is the page (`<body>`): block layout, the frame's size by default, so CSS positions things in the picture and the clip needs no `transform`. `"auto"` on either side fits the content.
- An element with no `display` that holds block elements (`div`, `p`, `h1`, …) is a block, and they stack. One that holds only text and inline elements sets them in a row.
- Only the marked rectangle is composited, so a frame-sized box costs no more than a small one.

### Files it points at

- `<img src>` and `<link rel="stylesheet" href>` are relative to the markup file, as on a page.
- Asset path rules apply: no leading `/`, no `..`, no drive letter, no URL. Outside the root or missing: E452, at validation. `--assets DIR` moves the root.
- `<link>`, `<meta>`, `<base>`, `<title>` draw nothing and take no layout slot.

### What it parses

- Well-formed only: non-void elements closed or self-closed (`/>`). No tag inference or recovery; a mismatch is E451 with line and column. Void: `br`, `hr`, `img`, `input`, `link`, `meta`, …
- `<style>` collected, stylesheet `<link>` read, comments and doctypes skipped, `<script>` is an error.
- Entities: the common named set (`&amp;`, `&nbsp;`, `&middot;`, `&mdash;`, …), `&#39;`, `&#x41;`.
- Selectors: type, class, id, `*`, descendant and child (`>`) combinators, comma lists. Pseudo-classes, attribute selectors, sibling combinators, at-rules: E451.
- Cascade: specificity (ids, classes, types), then source order; `!important` above both; `style` attribute above every selector; text properties inherit.
- User-agent styles (overridable): `h1`, `h2`, `h3`, `b`, `strong`, `i`, `em`, `small`.

### What it draws

| Group | Properties |
| --- | --- |
| Box | `display` (`flex`, `block`, `none`), `position` (`relative`, `absolute`), `top`, `right`, `bottom`, `left`, `inset`, `width`, `height`, `min-width`, `min-height`, `max-width`, `max-height`, `aspect-ratio`, `box-sizing`, `overflow` (and `-x`, `-y`) |
| Spacing | `margin`, `padding`, per-side forms, one-to-four-value shorthands |
| Flex | `flex-direction`, `flex-wrap`, `justify-content`, `align-items`, `align-self`, `align-content`, `gap`, `row-gap`, `column-gap`, `flex-grow`, `flex-shrink`, `flex-basis`, `flex` |
| Border | `border`, `border-top`, `border-right`, `border-bottom`, `border-left`, `border-width`, `border-color`, `border-style` (`solid`, `none`), `border-radius` |
| Paint | `background`, `background-color` (colour or gradient), `background-size`, `background-position`, `background-clip` (`text`, `border-box`, `-webkit-` too), `opacity`, `mix-blend-mode` (`normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `difference`, `soft-light`), `filter` (`blur()`), `clip-path` (`polygon()`, `none`), `box-shadow` (list; no `inset`), `backdrop-filter` (`-webkit-` too; `blur()`, `saturate()`, `brightness()`, `contrast()`, in order), `transform`, `transform-origin`, `anchor-name`, `mask-image` (`linear-gradient()`, `radial-gradient()`, `none`), `mask-size`, `mask-position`, `mask-repeat` (`repeat`, `no-repeat`, `repeat-x`, `repeat-y`; the four with `-webkit-` too) |
| Text | `color`, `-webkit-text-fill-color` (as `color`), `font`, `font-family`, `font-size`, `font-weight`, `font-style`, `line-height`, `letter-spacing`, `text-align` (`start`, `end`, `left`, `center`, `right`), `text-transform` (`none`, `uppercase`, `lowercase`, `capitalize`), `direction` (`ltr`, `rtl`), `white-space`, `text-shadow` (list), `-webkit-text-stroke` and its `-width` and `-color` longhands, `paint-order` |
| Motion | `animation`, `animation-name`, `-duration`, `-delay`, `-timing-function`, `-iteration-count`, `-direction`, `-fill-mode` |

`align-items: baseline` and `align-self: baseline` line up the bottoms of the boxes, not their text baselines.

Anything else is W450: the declaration is skipped, the element and
property are named, and the rest of the document is drawn. A `<style>` rule matching nothing is W452.

**Lengths**

- `px`, `em`, `rem`, `%`. `em` = the element's own font size; a percentage `font-size` or `line-height` is of the inherited one.
- `calc()`, `min()`, `max()`, `clamp()` over `px`, `em`, `rem` and numbers, nested, wherever a length is read: `max(1px, 0.06em)`. A percentage inside one is W450.
- Percentages only on widths, heights, margins, padding, gaps and insets (against the containing block). Elsewhere absolute only: `border-radius: 50%` is W450.
- `box-sizing: content-box` by default, as in CSS.

**Inline text**

- Text with inline elements inside (`<p>Go for <b>launch</b> at nine</p>`) is set as one run: wraps as a whole, whitespace collapsed across pieces, each piece keeps colour, weight, style, size, family, letter-spacing. `<br>` breaks the line.
- Inline tags: `a`, `abbr`, `b`, `br`, `cite`, `code`, `em`, `i`, `kbd`, `mark`, `q`, `s`, `small`, `span`, `strong`, `sub`, `sup`, `time`, `u`, `var`. `mark`, `u`, `s`, `sub`, `sup` draw as plain text.
- An inline element with a background, border, padding, margin, shadow, effect, animation or position gets a box of its own, as do the children of an element with `display: flex`. The sentence around it is then set as boxes, not as one line: a word with a background goes on a line of its own. Colour, weight and size change a word inside the line.

**Painting**

- `z-index` orders positioned boxes and flex items (negative under the flow); elsewhere W454.
- A box with `opacity` < 1, `filter`, `clip-path`, `mask-image`, `mix-blend-mode` or an animation is a group: its subtree is painted into its own buffer and composited whole (a stacking context).
- A group's buffer is capped at nine times the frame's area, any shape (a news crawl fits); larger is cut back with W455.
- `box-shadow` is drawn only outside the border box, as in CSS: a translucent box does not show its own shadow through it.
- `-webkit-text-stroke` is centred on the glyph outline, with mitred joins (limit 4) and butt ends, as browsers draw it; it inherits, and its colour defaults to `currentcolor`. With `paint-order: normal` it is drawn over the fill and covers the glyph's edge; `paint-order: stroke fill` (or `stroke`) puts it underneath, so half its width shows. It does not change layout. Inside a run of inline elements the whole run takes the stroke of the element holding it.
- `mask-image` keeps the group by the gradient's alpha, laid out on the border box as a background is (size, position, repeat; a tile that does not repeat keeps nothing outside it), after the blur and `clip-path` and before the transform. A document using it renders on the CPU.
- `backdrop-filter` filters what is under the box, inside its border box, before the box is drawn: the clips on lower layers and earlier clips of the same layer, not the boxes of the same markup painted under it. As in Chromium, nothing outside the border box is read, so a blur thins out towards the edges and the unblurred picture shows through there; the result is clipped to the rounded corners and mixed in by the box's opacity. Works on sRGB-encoded values. A document using it renders on the CPU (`--renderer gpu` says so).
- A static `transform` takes the functions keyframes do (`translate`, `scale`, `rotate` and their axis forms) and makes the box a group; an animation of `transform` replaces it while it plays, and starts from it where its rules leave a value out. A function geneva does not draw is W450.
- A transform (`transform`, `translate`, `scale`, `rotate`) turns and scales about `transform-origin`: one to three values, keywords (`left`, `center`, `right`, `top`, `bottom`), lengths or percentages of the border box; a third (depth) is ignored. The centre by default. A `transform` list composes in written order: `scale(0.5) translateX(20px)` moves 10 px. A percentage translation written after a `rotate()` is scaled but not turned.
- `body` and `html` select the clip's box; unstyled, it marks nothing.

**Direction and case**

- `direction: rtl`, or `dir="rtl"` on an element (the style sheet wins over the attribute), inherits: each paragraph of its text runs right to left whatever script it starts in, `text-align: start` (the default) and `end` are the right and left edges, and a flex row runs from the right (`row` and `row-reverse` swap). `dir="auto"` takes the direction of the first strong character in the element's text. Without either, paragraphs run left to right, as CSS's default is. Block boxes are not mirrored: a narrower block child still sits at the left.
- `text-transform` applies Unicode's casing (`ß` uppercase is `SS`, a word-final sigma lowercase is `ς`); with `lang="tr"` or `"az"` on the element or an ancestor, `i` and `I` case as in Turkish. `capitalize` capitalises the first letter or digit of each word, words being separated by spaces, across inline elements.
- Variable fonts load, and `font-weight` sets their `wght` axis. Other axes (`font-variation-settings`, `font-optical-sizing`) are not set: W450.

**Fonts**

- Font files: TrueType, OpenType and collections (`.ttf`, `.otf`, `.ttc`), and the web formats WOFF and WOFF2 (`.woff`, `.woff2`), unpacked when loaded.
- A family shipped as a `font` asset replaces the installed one of that name, in markup and text sources, by asset id or declared family. Other installed families remain as fallback.
- `@font-face` in markup gives a font file a family: `font-family`, `src` (the first `url()`, relative to the stylesheet; `local()` and `format()` are not read), and optionally `font-weight` and `font-style`, which the face then answers to. The family replaces an installed one of that name, as a font asset's does. A missing file is E452 (checked by `validate --probe` and at render).
- A `font-family` list (and a text source's `font` list) is fallen back through a character at a time, as in a browser: each character is drawn in the first family with a glyph for it, at that family's nearest weight; spaces, combining marks and joiners stay with the text before them, so a word is shaped whole (Arabic joins). Generic names (`sans-serif`, `serif`, `monospace`, ...) are the machine's. Only characters no listed family has fall back to the installed fonts, so a list of font assets draws the same everywhere. Each named family that is neither installed nor an asset is reported (W405).
- Lines are stacked as in Chromium: each font's ascent, descent and line gap (OS/2 typographic metrics when the font sets `USE_TYPO_METRICS`, else `hhea`) are rounded to whole pixels; the half-leading a line height leaves is floored above the baseline; a line is as tall as the most any of its fonts reaches above the baseline plus the most below. `line-height: normal` (the default) is each font's own ascent + descent + line gap, so a fallback font with taller metrics makes its line taller, as in a browser. Every line also holds the first available family of the list (CSS's strut), so a Latin line in `font-family: "Noto Sans Arabic", Inter` is as tall as an Arabic one. A `font` shorthand without `/line-height` resets it to `normal`. Measured against Chromium 1194 for Inter, Lora, Manrope, Newsreader, Oswald, Cairo: line boxes equal, glyphs within 1 px. There is no synthetic bold or italic: a family without the face is drawn in the nearest it has.
- A paragraph is left to right, CSS's default `direction`, even when it starts with a right-to-left word; right-to-left words inside it are ordered as in a browser. `direction` and `dir` are not read.
- A missing weight resolves by the CSS rule (above 500: next heavier; below 400: next lighter; 400 to 500: heavier up to 500, then lighter). A family with no italic face is drawn upright. Decided from the family alone, so identical on every machine.

### Gradients

```css
background: linear-gradient(160deg, #07100C, #123a2c);
background: linear-gradient(to bottom right, #00EEE1, #FFD233 70%);
background: radial-gradient(circle at 30% 40%, #00EEE1, #00EEE100);
```

- Linear: an angle (`deg`, `turn`, `rad`) or `to` a side or corner; default top to bottom. Corner angles follow the box's proportions.
- Radial: circle or ellipse, `at` a position (percentages or keywords); default centred; always sized to the farthest corner.
- Stops: colour and optional position. Missing positions are spread evenly; a backwards one is pulled up to its predecessor; two positions make a flat band.
- Tiles: `background-size` (one or two lengths, percentages or `auto`) and `background-position` (lengths, percentages, side keywords); the tile repeats. Clipped to the tile, so an unfinished radial shows an edge.
- `background-clip: text` (`-webkit-` too) fills the glyphs of the element and its descendants; the tile is sized per text run. The markup form of a text clip's `fill`.
- Not drawn, named: `conic-gradient`, `repeating-*`, size keywords (`closest-side`, …), colour hints, `url()`, `cover`, `contain`, multiple layers.

**Colour space**: inside the box, blending (stops, translucent boxes,
shadows, blur, text edges) is on sRGB-encoded premultiplied values, as in a
browser. A markup clip with the `normal` blend mode is also laid over what
is under it in sRGB-encoded values, its opacity and transition ramps
included, as a browser lays a page over a video; with another blend mode
it is laid on in linear light ([color.md](color.md)). Exception: glyphs use
the text engine, so a text gradient mixes its stops in linear light.

### Motion

Animations in the markup play as they do in a browser.

```html
<style>
  @keyframes slide-in { from { transform: translateX(-656px) } to { transform: none } }
  .card { animation: slide-in 0.5s ease-out; /* ... */ }
</style>
<div class="card">...</div>
```

**Outermost element** (the document's single top-level element)

- Its `animation` is played by the clip: `transform` and `opacity` only (anything else E442), held before and after whatever `animation-fill-mode` says.
- A clip `animation` replaces it (W451 says which won).
- A document with several top-level elements has no outermost element; wrap them to get one.

**Any other animated element** is played in place, as a browser does, as a
group composited with the transform, opacity and blur of the moment.

- Several animations stack in written order; a `to`-only rule starts from what the rules under it, or the style, leave.
- Longhands read, comma lists included: `animation-delay`, `-duration`, `-iteration-count`, `-direction`, `-fill-mode`.
- Keyframe properties: `transform`, `opacity`, `filter: blur()`, `color`, `-webkit-text-stroke-color`, `text-shadow`, `letter-spacing` (`px` or `em`), `width`, `height`, `max-width`, `min-width`, `left`, `top`, `background-position`, `clip-path`, `background-color` (or a `background` that is one colour), `border-color` and `border-*-color`, `box-shadow`, `mask-position`, `mask-size` (`-webkit-` too); anything else E442. `color`, `-webkit-text-stroke-color` and `text-shadow` reach inheriting descendants.
- `left`, `top` and `width` in a keyframe may name another box: `anchor(<name> left|right|top|bottom|center)` and `anchor-size(<name> width|height)`, the name an `anchor-name` (`--word`) or an element id. They are worked out from where that box is laid out in the same frame, against the animated box's containing block, and then mixed as lengths: an underline that glides from word to word is `@keyframes { from { left: anchor(--w3 left); width: anchor-size(--w3 width) } to { left: anchor(--w4 left); width: anchor-size(--w4 width) } }`. A fallback after a comma is not read. A name that matches nothing leaves the property as the style has it.
- Colours mix as in a browser: sRGB-encoded channels, premultiplied by alpha. Shadow lists mix shadow by shadow, the shorter padded with transparent shadows. An animated `background-color` under a gradient is not drawn.
- Polygons with the same point count mix point by point; otherwise a step at halfway. A length and a percentage do not mix (step), except zero.

**Cost**

- No animation inside: laid out and painted once per clip.
- Animations that only step (a `steps()` timing, or keyframes that all say the same): laid out and painted once per interval between the moments they can change, as word-by-word highlighting does; the picture is reused for the frames in between.
- Any other animation inside: laid out each frame (text measured once per clip); an animated element whose boxes do not change keeps its picture and is only composited again.
- `filter: blur()` is rounded to 0.25 px, so frames whose radii round the same share a blurred picture, and a radius under 0.125 px is none. Over 8 px it is computed at half to a quarter of the size and scaled back, within a code of the full blur.

## Known limitations

The main ones:

- **Markup is a subset of HTML and CSS**, for titles and graphics: no JavaScript, grid, float, CSS transitions or media queries. Anything else it does not draw is named (W450).
- **Markup is local**: nothing is fetched over the network; `<iframe>`, `<svg>`, `<canvas>`, `<video>` are not drawn.
- **Audio is mono or stereo**; more channels are downmixed.
- **No fixed average bitrate**: constant quality, optionally under a ceiling ([rate control](cli.md#rate-control)). H.265 needs a hardware encoder.

## Timing rules

How clip times are resolved when they are not given explicitly:

1. A clip without `start` begins where the previous clip in its layer ends; the first at 0.
2. Video and audio clips last `duration`, else `out - in`, else to the end of the file. `render` and `validate --probe` read that length; plain `validate` opens no files and treats such a clip as open-ended (rule 3).
3. Images, solids, shapes and text are open-ended: without `duration` they run to `output.duration`; if that is unset too, E305.
4. Clips in a layer must not overlap (E302), except by a `transition`'s overlap; a clip with a transition and no `start` moves earlier by it, and the previous clip must cover it (E306).
5. `output.duration` defaults to the latest end across layers and audio; clips past it are cut (W301).
6. Keyframe and word times are relative to the clip start.

## Determinism

- A frame is a pure function of timeline, assets and time; nothing reads a clock.
- Same platform: identical frames. Across platforms: perceptually identical; the repository's golden tests (`tests/golden/`) compare with tolerance.

## Validation

- `geneva validate timeline.json` reports every problem: code, JSON pointer, value, suggested fix. Codes: [errors.md](errors.md).
- `--format json` prints the same as one JSON document.

## Renderers and encoders

- `render` stream-copies when the composition is a plain cut or join at natural size and nothing asks for a re-encode ([architecture.md](architecture.md#stream-copy)); `--exact` forces frame-accurate rendering.
- With `outputs`, `-o` names a directory; every entry is written from one pass.
- Two renderers draw everything (every source, transform, opacity, blend, mask, effect, transition): CPU reference and GPU, `--renderer`. Markup is painted on the CPU in both. They agree to the golden tolerance, not byte for byte.
- `encode.video.hardware`: `auto` uses VideoToolbox, NVENC or Media Foundation (Windows, 8-bit, GPU encoder only) when present and working; `never` software only; `require` fails without one. Notes name the encoder used.
- Fonts: `font` assets first, then installed fonts; ship fonts as assets for identical output across machines. With no fonts installed at all, Liberation Sans is built in. Markup `font-family` may name an asset by id or declared family; all font assets are registered before markup is drawn.
