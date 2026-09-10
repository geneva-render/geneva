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
`seconds`. `mode` is `"copy"` (source streams copied, no decoding),
`"direct"` (decoded frames handed straight to the encoder) or `"render"`
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
