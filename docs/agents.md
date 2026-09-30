# Geneva for programs and agents

For code, scripts and AI agents that drive `geneva`. This page is the
calling contract and the handful of format details a generator gets
wrong most often. Everything else is in the
[command-line reference](cli.md), the
[timeline reference](timeline.md) and [errors.md](errors.md).

Contents: [the contract](#the-contract), [report shape](#report-shape),
[a working loop](#a-working-loop), [starting from a verb](#starting-from-a-verb),
[clip fields](#clip-fields), [motion](#motion),
[transitions](#transitions), [markup](#markup), [captions](#captions),
[outputs](#outputs), [shorthands](#shorthands),
[things that trip programs up](#things-that-trip-programs-up).

## The contract

- **One binary, no services.** `geneva` reads files, writes files and
  prints one report. Only `farm` and `worker`, which split a render
  across machines, use the network.
- **The manual is in the binary.** `geneva guide` prints this page,
  `geneva guide --list` names the others (`timeline`, `cli`, `errors`,
  `color`, `architecture`, `farm`), and `geneva explain E302` says what one code means. So a
  machine with `geneva` on its path has the reference for the version
  it is running, without a checkout and without fetching anything.
- **`--format json` everywhere.** Put it before the subcommand. Stdout
  is then exactly one JSON document.
- **Progress on stderr, in both modes.** Under `--format json` each
  line is a complete object, so a long render can be followed without
  waiting for the report. A run that copies its streams composites no
  frames and prints none.
- **Deterministic.** The same timeline, assets and version produce the
  same frames, so results can be cached by input hash.

| Exit | Meaning |
| --- | --- |
| 0 | Done |
| 1 | Timeline invalid |
| 2 | Anything else: bad usage, a file that could not be read |
| 3 | Render or encode failed |

A progress line, at the first frame, about once a second, and at the
last:

```json
{"event":"progress","frames":540,"total":1800,"seconds":8.0,"rate":67.5,
 "time":18.0,"duration":60.0,"remaining":18.67}
```

`remaining` is the estimate in seconds at the rate so far, and `null`
where there is nothing to estimate from: the last line, and the first
half second, which measures startup as much as work.

## Report shape

Every command that touches a timeline returns `ok`, `diagnostics` and
`summary`:

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

A diagnostic carries a stable `code`, a JSON pointer `path` into the
document, and a `help` string when there is a concrete fix. Errors stop
the run; warnings and notes do not.

What each command adds to those three keys:

| Command | Adds |
| --- | --- |
| `validate` | nothing |
| `render`, and the verbs | `output`, `content_type`, `mode`, `frames`, `duration`, `seconds` |
| `render` on a document with `outputs` | `directory`, `outputs`, `frames`, `duration`, `seconds` |
| `frame` on a timeline | `output`, `content_type`, `time`, `frame`, `width`, `height` |
| `frame` on a media file | as for `outputs`, with one entry |
| `subtitles --extract` | `output`, `content_type`, `cues` |
| `probe` | the media description itself, not this envelope |

`mode` says how the picture was made:

| `mode` | What happened |
| --- | --- |
| `copy` | Source streams copied, nothing decoded |
| `smart` | Packets copied where nothing changes, the frames around cuts and under overlays encoded into the same stream |
| `direct` | Decoded frames handed straight to the encoder, scaled or repacked when needed |
| `render` | Frames composited by the renderer |

Each entry in `outputs` is one file written: `name`, `kind`, `path`,
`bytes`, `mode`, `content_type`, and `width` and `height` for pictures
and video, which for a sprite sheet are one tile. A sprite sheet's
WebVTT map is its own row, of kind `sprites-map`. `width` and `height`
are absent, not null, where they do not apply.

The list is meant to go to an uploader as it stands, which is what
`content_type` is for: a `.vtt` or `.mp4` served as
`application/octet-stream` breaks players. Redirect the report to keep
it as a manifest.

## A working loop

1. **Write the timeline** as JSON. Start from `geneva guide timeline`
   for the fields in prose, `geneva schema` for the machine-readable
   form, or the examples in the repository's `examples/` directory,
   which the binary does not carry; every field has a description in
   the schema.
2. **Validate with probing** before rendering anything expensive:

   ```sh
   geneva --format json validate timeline.json --probe
   ```

   Fix every error using its `path` and `help`. Where that is not
   enough, `geneva explain <code>` gives the full description of the
   code, and exits 2 if the code does not exist. Probing also tells the
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

## Starting from a verb

The everyday verbs build timelines. `--show-timeline` prints the
document instead of rendering, which is the fastest way to a correct
skeleton to edit:

```sh
geneva trim in.mp4 -o out.mp4 --from 10s --to 20s --show-timeline > job.json
geneva render job.json --assets /path/printed/on/stderr -o out.mp4
```

The asset root the printed document expects goes to stderr. It is the
deepest directory containing every input. A printed timeline always
uses the long form, never the shorthands below.

## Clip fields

| Field | Shape | Effect |
| --- | --- | --- |
| `crop` | a rectangle of the source | Becomes the clip's box, before `fit` |
| `effects` | a list of `{ "kind": ... }` | `blur` with an animatable `radius` |
| `mask` | a rectangle or ellipse with `radius` and `feather`, or a luma image `asset` | In the box's own coordinates |
| `speed` | a number | Constant rate change; the clip lasts its range divided by it |

Any of these makes the clip composited, so `mode` is `render`: the
copy, smart-cut and direct paths do not apply to it. Fields in
[timeline.md](timeline.md#crop). The verbs expose the everyday cases as
`--crop X,Y,WxH` (or `WxH` for the middle), `--speed FACTOR` and
`--fill blur`.

## Motion

Keyframe tracks are one spelling. The other is CSS: a top-level
`keyframes` map of `@keyframes` rules, played by a clip's `animation`
with the CSS shorthand.

```json
"animation": "slide-in 0.5s ease-out, fade-out 0.3s 3.7s"
```

Both resolve to the same tracks, so neither is faster. Two things catch
generators out: duration and delay need their units, and a rule's
opacity replaces the clip's rather than combining, so the clip's value
must be constant where a rule drives it (E443). Transforms do combine.
Offsets, properties and `spring(170, 26)` are in
[timeline.md](timeline.md#animation).

## Transitions

A clip's `transition` says how it arrives from the clip before it on
the same layer. `transition_out` closes a layer the same way.

```json
"transition": { "kind": "crossfade", "duration": "0.5s", "ease": "ease-out" }
```

| `kind` | Picture | Sound |
| --- | --- | --- |
| `crossfade` | Both clips up at once, dissolving | Crosses at constant power |
| `fade` | One clip at a time, dipping through `color` (black by default) | To silence and back |

The pair overlaps by `duration` and a clip with no `start` is moved
earlier to make room. On the first clip of a layer there is nothing to
come from, so the transition fades the clip in over the transition's
`duration`. A `fade` covers every layer, above ones too; to dip a cut
under overlays that stay, put the cut in a composition. E306 if the
previous clip does not cover the overlap, E307 for a `transition_out`
where another clip follows, W304 for a `color` on a crossfade. Details
in [timeline.md](timeline.md#transitions).

## Markup

A clip can draw a box of markup: a source of kind `html`, with the
markup inline in `html` or in an asset of kind `html`, plus optional
`css`, a `width` (default: the frame) and a `height` (default: fits the
content). It is a strict HTML parser with a CSS subset and block and
flexbox layout: markup inside that subset looks and moves the same here
and in a browser, and what falls outside it is named rather than drawn
differently. Check a card with `geneva frame` before rendering it.

Pictures and stylesheets take paths relative to the markup, as on a
page (`<img src="logo.png">`). A path that leaves the asset root, or a
file that is not there, is E452 at validation.

Nothing is dropped in silence, which is what makes a generated card
debuggable:

| Code | When |
| --- | --- |
| E451 | Markup or a selector it cannot parse |
| W450 | A property it does not draw |
| W452 | A rule whose selector matches no element |
| W451 | Both the markup and the clip set an `animation` |
| W454 | `z-index` where it does not apply |

What it parses and draws is in [timeline.md](timeline.md#markup).

## Captions

A transcript is a source: `{ "kind": "captions", "asset": "words" }`
reads a `.srt`, a `.vtt`, or the `.json` a speech recogniser writes,
and becomes one clip per cue, so the frames between cues can still be
copied.

Word files go in as they came: whisper's `segments[].words[]`, a bare
`{"words": [...]}`, or a bare list. `word` or `text` names the word and
every other key is ignored. Word times are what let `style.highlight`
pick out the word being said.

Cue times are the file's, counted from the clip's `start`. Captions for
a video clip that uses part of its media take the same `in`, `out` and
`start` as that clip, so three shots from one film are three captions
clips on one file. A pause of a second or more ends a cue.

An empty caption track is never drawn quietly. A file with no words in
it, or with times in milliseconds, is E453; a `highlight` on a file
that times whole cues rather than words is W453.

`geneva subtitles --burn FILE [--highlight COLOR]` is the same thing
without a document. Cue placement is in
[timeline.md](timeline.md#sources).

## Outputs

The top-level `outputs` map is the files one render writes from the
composition, each of kind `video`, `poster`, `sprites` or `audio`, with
a size, a time, an interval, or encode and audio settings of its own.
`geneva render timeline.json -o DIR` writes them all in one pass, which
is how to get a rendition, a thumbnail and a speech track from one read
of the source.

Three encode fields are easy to miss: `encode.video.tune` takes x264's
names, `encode.video.fixed_keyframes` needs `keyframe_interval` set
(E422), and `output.color` may be `pq` or `hlg` only on a ten-bit codec
(E420). HDR sources are tone-mapped to SDR unless the output is HDR.
The rest is in [timeline.md](timeline.md#outputs).

## Shorthands

Four parts of a document have a shorter spelling. All are optional and
accepted alongside the long form, so a document can mix them.

- A point (`transform.position`, `transform.anchor`) as a pair or a
  string: `[30, 36]`, `"30 36"`, `"0% 50%"`, or the CSS keywords
  `"left"`, `"center"`, `"bottom right"`.
- A keyframe as its fields in order, `[t, v]` or `[t, v, ease]`, so
  `{ "keyframes": [ [0, 0], ["0.5s", 1, "ease-out"] ] }`.
- A timed word as its fields in order, `["word", start, end]`, or
  `["word", start]` where the next word's start ends it. The last word
  needs its own end (E102).
- A number of pixels as a CSS length: `"8px"` wherever `8` is accepted,
  keyframe values included (`"size": "34px"`, `"radius": "3px"`,
  `"blur": "8px"`). Frame and picture sizes must be whole pixels.

## Things that trip programs up

- **Asset paths are relative to a root**, the timeline's directory by
  default or `--assets DIR`. Absolute paths and `..` are rejected
  (E202).
- **Times are strings** in JSON when they carry a unit: `"1.5s"`,
  `"45f"`, `"00:00:01.5"`. A bare number is seconds.
- **`deny_unknown_fields`.** A misspelled key is an error, not ignored,
  so typos surface at validation rather than as a silently wrong
  render.
- **A bitrate is a ceiling.** `--max-bitrate 6M` caps a constant-quality
  encode (`--crf`); there is no fixed average bitrate, constant bitrate
  or two-pass mode. A bare number is kb/s, not ffmpeg's bits per second.
  For a file of a given size use `--for TARGET --budget 25MB`.
- **Sizes should be even** for the common codecs (W401 warns). The
  verbs round for you, timelines do not.
- **Rotated clips come out upright.** Phones store portrait video as a
  landscape stream plus a rotation flag. `probe` reports the displayed
  size, the renderer turns frames upright, and a copy keeps the flag, so
  nothing needs to read it. Sizes in a timeline are displayed sizes.
- **Containers reject codecs they cannot hold** with a plain error
  rather than a broken file. The table in
  [cli.md](cli.md#containers-and-codecs) says what goes where. Image
  sequences need a `%04d`-style pattern in the output path.
