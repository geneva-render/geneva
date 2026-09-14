# Geneva for programs and agents

This page is for code, scripts and AI agents that drive `geneva`. It is
short on purpose; the [command-line reference](cli.md) and the
[timeline reference](timeline.md) have the details.

## The contract

- **One binary, no services.** `geneva` reads files, writes files and
  prints one report. It never needs a network.
- **`--format json` everywhere.** Put it before the subcommand. Stdout is
  then exactly one JSON document; stderr carries only progress text.
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

`render` and the verbs add `output`, `mode`, `frames`, `duration` and
`seconds`. A render of a document with an `outputs` map, and the
`poster`, `sprites` and `publish` verbs, add `directory` and `outputs`
instead of `output` and `mode`: one entry per file with `name`, `kind`,
`path`, `bytes` and its own `mode`. `mode` is `"copy"` (source streams copied, no decoding),
`"smart"` (packets copied where nothing changes, the frames around cuts
and under overlays encoded into the same stream),
`"direct"` (decoded frames handed straight to the encoder, scaled or
repacked when needed) or `"render"`
(frames composited by the renderer). `frame` adds `output`, `time`,
`frame`, `width`, `height`. `probe` returns the media description directly.

Diagnostics carry a stable `code` ([errors.md](errors.md)), a JSON
pointer `path` into the document, and a `help` string when there is a
concrete fix. Errors stop the run; warnings and notes do not.

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
its own. `geneva render -o DIR` writes them all in one pass. The
`poster`, `sprites` and `publish` verbs build such documents;
`publish` is the one to reach for when a page needs a video, a poster
and seek previews, and it copies a source that already fits the web.

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
