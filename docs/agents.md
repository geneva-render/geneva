# Geneva for programs and agents

This page is for code, scripts and AI agents that drive `geneva`. It is
short on purpose; the [command-line reference](cli.md) and the
[timeline reference](timeline.md) have the details.

## The contract

- **One binary, no services.** `geneva` reads files, writes files and
  prints one report. It never needs a network.
- **`--format json` everywhere.** Put it before the subcommand. Stdout is
  then exactly one JSON document; stderr carries one progress object per
  line (see [progress](cli.md#progress)), so a long render can be
  followed without waiting for the report.
- **Exit codes mean something.** 0 done, 1 timeline invalid, 2 usage
  error, 3 render or encode failed.
- **Deterministic.** The same timeline, assets and version produce the
  same frames, so results can be cached by input hash.

## Report shape

Every command that touches a timeline returns:

```json
{
  "ok": true,
  "diagnostics": [
    { "severity": "note", "code": "N600", "path": "",
      "message": "cut at 0.5s moved to the keyframe at 0.48s", "help": null }
  ],
  "summary": { "errors": 0, "warnings": 0, "notes": 1 }
}
```

`render` and the verbs add `output`, `content_type`, `mode`, `frames`,
`duration` and `seconds`. A render of a document with an `outputs` map,
and `frame` on a video file, add `directory` and `outputs` instead of
`output` and `mode`: one entry per file with `name`, `kind`, `path`,
`bytes`, `content_type`, its own `mode`, and `width` and `height` for
pictures and video (one tile, for a sprite sheet). Every file written
is a row, the sprite map as `sprites-map`, so a program can hand the
list to an uploader as it is; redirect the report to keep it as a
manifest. The content types are the point: a `.vtt` or `.mp4` served as
`application/octet-stream` breaks players, and the report names the
right type for each file. `mode` is `"copy"` (source streams copied, no decoding),
`"smart"` (packets copied where nothing changes, the frames around cuts
and under overlays encoded into the same stream),
`"direct"` (decoded frames handed straight to the encoder, scaled or
repacked when needed) or `"render"`
(frames composited by the renderer). `frame` adds `output`, `time`,
`frame`, `width`, `height`. `probe` returns the media description directly.

Diagnostics carry a stable `code` ([errors.md](errors.md)), a JSON
pointer `path` into the document, and a `help` string when there is a
concrete fix. Errors stop the run; warnings and notes do not.

While a render runs, each line on stderr is a complete JSON object
`{"event":"progress","frames":…,"total":…,"seconds":…,"rate":…,"time":…,
"duration":…,"remaining":…}`: the first frame, then about once a second,
then the last. `remaining` is the estimate in seconds at the rate so far,
`null` when there is nothing to estimate from (the last line, and the
first half second, which measures the startup too). Read stderr line by
line
to drive a progress bar or a timeout; a run that copies its streams
prints none, because it renders no frames.

## A working loop

1. **Write the timeline** as JSON. Start from `geneva schema` or from the
   examples in `examples/`; every field has a description in the schema.
2. **Validate with probing** before rendering anything expensive:

   ```sh
   geneva --format json validate timeline.json --probe
   ```

   Fix every error using its `path` and `help`. Probing also tells the
   resolver how long open-ended video clips are.
3. **Check one frame** at the moment that matters:

   ```sh
   geneva --format json frame timeline.json --at 2.5s -o check.png
   ```
4. **Render.**

   ```sh
   geneva --format json render timeline.json -o out.mp4
   ```

   Read `mode` and the N600 notes: a copy is instant but cuts land on
   keyframes; add `--exact` when the frame matters more than the time.

## Verbs as a starting point

The everyday verbs build timelines. `--show-timeline` prints the document
instead of rendering, which is the fastest way to get a correct skeleton
for a job and then edit it:

```sh
geneva trim in.mp4 -o out.mp4 --from 10s --to 20s --show-timeline > job.json
geneva render job.json --assets /path/printed/on/stderr -o out.mp4
```

The asset root the printed document expects is written to stderr (it is
the deepest directory containing every input).

Format 0.2 added four optional clip fields, all documented in
[timeline.md](timeline.md): `crop` (a rectangle of the source that
becomes the clip's box), `effects` (a list; `blur` with an animatable
`radius`), `mask` (a rectangle or ellipse with `radius` and `feather`,
or a luma image `asset`, in the box's own coordinates) and `speed` (a
constant rate change; the clip lasts its range divided by it). The verbs
expose the everyday cases as `--crop X,Y,WxH` (or `WxH` for the middle),
`--speed FACTOR` and `--fill blur` (a blurred, scaled-up copy of the
picture behind it on a canvas it does not cover). A clip using any of
these is composited; the copy, smart-cut and direct paths do not apply
to it, so the report's mode will be `render`.

Version 0.3.0 kept the format at 0.2 and added optional encode fields:
`encode.video.tune` (x264's names: `film`, `animation`, `grain`,
`stillimage`, `fastdecode`, `zerolatency`), `encode.video.fixed_keyframes`
(keyframes at `keyframe_interval` only; needs the interval, E422) and
`encode.video.chunks` (`auto`, a number, or `1` to encode in one run).
`output.color` may now be `pq` or `hlg` with BT.2020 on a ten-bit codec
(`h265`, `av1`, `vp9`, `prores`; E420 otherwise); HDR sources are
tone-mapped to SDR unless the output is HDR, and the verbs take
`--keep-hdr`. Time zero of a file is its first video frame, so an audio
track that starts later keeps its offset.

Format 0.3 adds the optional top-level `outputs` map: the files one
render writes from the composition, each `{ "kind": "video" | "poster" |
"sprites" | "audio", ... }` with a size, a time (`at` for a poster),
an interval and column count (sprites) or encode and audio settings of
its own. `geneva render -o DIR` writes them all in one pass; that is
the way to get a rendition, a thumbnail and a speech track from one
read of the source. `frame` on a video file builds a one-entry
document of this kind. `--for` on a verb copies a source that already
fits the target instead of re-encoding it, and the note says which
branch it took.

Three parts of a document have a shorter spelling, all of them optional
and all of them accepted alongside the long form, so a document can mix
them freely:

- A point (`transform.position`, `transform.anchor`) as a pair or a
  string: `[30, 36]`, `"30 36"`, `"0% 50%"`, or the CSS keywords
  `"left"`, `"center"`, `"bottom right"`.
- A keyframe as its fields in order: `[t, v]` or `[t, v, ease]`, so
  `{ "keyframes": [ [0, 0], ["0.5s", 1, "ease-out"] ] }`.
- A timed word as its fields in order: `["word", start, end]`, or
  `["word", start]` where the next word's start ends it. The last word
  needs its own end (E102).

A printed timeline (`--show-timeline`) always uses the long form.

Motion has a second spelling too. A top-level `keyframes` map holds CSS
`@keyframes` rules — an offset (`from`, `to`, `60%`) to a declaration
block setting `transform`, `translate`, `scale`, `rotate` or `opacity` —
and a clip's `animation` plays them with the CSS shorthand:
`"slide-in 0.5s ease-out, fade-out 0.3s 3.7s"`. Duration and delay need
their units; `infinite`, `alternate` and `spring(170, 26)` are
understood. A rule's transform is laid over the clip's own (translations
add to `transform.position`, scales multiply, rotations add) and its
opacity replaces the clip's, so the clip's value must be constant where
a rule drives it (E443). Everything resolves to the same keyframe
tracks, so neither spelling is faster or slower than the other.

A clip can also draw a box of markup: a source of kind `html` with the
markup in `html` or in an asset of kind `html`, plus optional `css`, a
`width` (default: the frame) and a `height` (default: fits the content).
It is a strict HTML parser, a CSS subset with type, class and id
selectors and the descendant and child combinators, and flexbox and block
layout from taffy. Refer to images by asset id, not by path
(`<img src="logo">`). There is no inline layout: an element's text is one
paragraph and a child element is a box. Anything geneva cannot draw is
named rather than ignored: E451 for markup or a selector it cannot parse,
W450 for a property it does not draw. `@keyframes` in the markup are in
scope for that clip, and an `animation` on its outermost element is what
the clip plays unless the clip sets its own (W451 when both do), so a
file that moves in a browser moves here. See
[timeline.md](timeline.md#markup) for the property list.

## Things that trip programs up

- **Asset paths are relative to a root**, the timeline's directory by
  default or `--assets DIR`. Absolute paths and `..` are rejected (E202).
- **Sizes should be even** for the common codecs (W401 warns); the verbs
  round for you, timelines do not.
- **Times are strings** in JSON when they carry a unit: `"1.5s"`, `"45f"`,
  `"00:00:01.5"`. A bare number is seconds.
- **`deny_unknown_fields`.** A misspelled key is an error, not ignored, so
  typos surface at validation rather than as a silently wrong render.
- **Long renders print progress on stderr** only in human mode; with
  `--format json` stderr stays quiet apart from library messages.
- **Rotated clips come out upright.** Phones store portrait video as a
  landscape stream plus a rotation flag. `probe` reports the displayed
  size, the renderer turns frames upright, and a copy keeps the flag, so
  nothing needs to read it. Sizes in a timeline are displayed sizes.
- **Containers reject codecs they cannot hold** with a plain error rather
  than a broken file; the table in [cli.md](cli.md#containers-and-codecs)
  says what goes where. Image sequences need a `%04d`-style pattern in the
  output path.
