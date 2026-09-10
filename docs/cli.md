# Command-line reference

`geneva` has two kinds of commands. The timeline commands take a timeline
document and validate, render or inspect it. The everyday verbs take media
files and options, build a timeline from them, and render it through the
same path; `--show-timeline` prints that document instead of rendering, so
any verb can be turned into a timeline you edit and run with `geneva
render`.

Every command takes `--format json` before the subcommand to print one JSON
document on stdout instead of readable text. In human mode diagnostics go
to stderr and stdout stays clean.

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | Done. |
| 1 | The timeline is invalid; the diagnostics say why. |
| 2 | Usage error: unknown option, missing file, bad time. |
| 3 | Rendering or encoding failed. |

## Times

Wherever a command takes a time, these spellings work: `1.5` or `1.5s`
(seconds), `1500ms`, `45f` (frames at the source's rate), `00:00:01.5`
(timecode). See [Times](timeline.md#times) in the format reference.

## Timeline commands

### `geneva validate <timeline> [--probe] [--assets DIR]`

Parses and validates a timeline. Each diagnostic has a code
([docs/errors.md](errors.md)), the JSON path of the value, and a
suggested fix. With `--probe` the media assets are opened too, which
checks that they exist and lets open-ended clips learn their lengths.
Asset paths are relative to the timeline's directory unless `--assets`
says otherwise.

### `geneva frame <timeline> [--at TIME | --frame N] -o FILE`

Renders one frame to a PNG file through the same renderer as `render`.

### `geneva render <timeline> -o FILE [options]`

Renders the whole timeline. The output container comes from the file
extension unless the timeline sets `output.encode.container`.

| Option | Effect |
| --- | --- |
| `--crf N` | Constant-quality level, overriding the timeline. Lower is better; 18 to 30 is the useful range for H.264 and H.265. |
| `--preset NAME` | Encoder speed preset, `ultrafast` to `veryslow`. |
| `--no-audio` | Write no audio track. |
| `--exact` | Always decode and re-encode, even when the streams could be copied. |

When the composition uses its sources as they are, the coded streams are
copied instead of re-encoded and the report says so
([stream copy](architecture.md#stream-copy)). Cuts then move to the
nearest earlier keyframe, which the report also lists; `--exact` makes cuts
frame-accurate at the cost of a re-encode.

### `geneva probe <file>`

Shows the container, duration, streams, sizes, rates, and the color tags of
a media file, including which tags had to be assumed.

### `geneva schema`

Prints the JSON Schema of the current timeline format version.

## Everyday verbs

All verbs take `-o FILE` for the output and these encoding options:

| Option | Effect |
| --- | --- |
| `--crf N`, `--preset NAME` | As for `render`. Setting either forces a re-encode. |
| `--codec h264\|h265\|vp9\|av1` | Video codec. Defaults to the container's usual one. |
| `--no-audio` | Write no audio track. |
| `--exact` | Always decode and re-encode. |
| `--show-timeline` | Print the timeline the verb built instead of rendering it. Asset paths in it are relative to the directory printed on stderr. |

### `geneva convert <input> -o FILE [--width W] [--height H] [--fit contain|cover|fill] [--fps FPS]`

Re-encodes a video, optionally to another container, codec, size or frame
rate. Giving one of `--width` and `--height` keeps the aspect ratio.
Dimensions are rounded to even numbers.

```sh
geneva convert talk.mov -o talk.mp4
geneva convert talk.mp4 -o talk.webm --codec vp9 --crf 32
geneva convert talk.mp4 -o talk-720.mp4 --height 720 --fps 30
```

### `geneva resize <input> -o FILE --width W | --height H [--fit contain|cover|fill]`

`convert` with a required size. `contain` (the default) shows the whole
picture with bars when the shape differs; `cover` fills the frame and
crops; `fill` stretches.

### `geneva trim <input> -o FILE [--from TIME] [--to TIME | --duration TIME]`

Keeps a range of the input. Without `--crf`, `--preset` or `--exact` the
streams are copied and the cut moves back to the previous keyframe; the
report gives the time actually used.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s
geneva trim talk.mp4 -o clip.mp4 --from 1:02:10 --duration 45s --exact
```

### `geneva concat <input>... -o FILE [--crossfade TIME]`

Joins inputs back to back. Sources with identical stream parameters are
joined by copying packets; anything else is rendered, with inputs of a
different shape fitted inside the first one's frame. `--crossfade` blends
each pair over the given time and always renders.

```sh
geneva concat part1.mp4 part2.mp4 part3.mp4 -o all.mp4
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s
```

### `geneva overlay <input> <overlay> -o FILE [options]`

Places an image or a video (without its audio) over the input.

| Option | Default | Effect |
| --- | --- | --- |
| `--at top-right\|top-left\|bottom-left\|bottom-right\|center` | `top-right` | Position. |
| `--margin PX` | 24 | Distance from the edges. |
| `--scale F` | 1 | Scale factor for the overlay. |
| `--opacity F` | 1 | Opacity from 0 to 1. |
| `--start TIME`, `--duration TIME` | whole video | When the overlay is shown. |

```sh
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5 --opacity 0.8
```

### `geneva audio <input> -o FILE --extract | --mute | --replace FILE | --mix FILE [--gain DB]`

| Operation | Output |
| --- | --- |
| `--extract` | Only the audio, to `.m4a` (AAC), `.ogg` (Opus), `.flac` or `.wav`. Copied without re-encoding when the codec already matches the container. |
| `--mute` | The video without its audio track. |
| `--replace FILE` | The video with another file's audio, cut to the video's length. |
| `--mix FILE` | The original audio and another file's mixed together; `--gain` adjusts the mixed-in file in decibels. |

```sh
geneva audio talk.mp4 -o talk.m4a --extract
geneva audio talk.mp4 -o dubbed.mp4 --replace voice.wav
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
```

## Containers and codecs

| Extension | Container | Default video | Default audio |
| --- | --- | --- | --- |
| `.mp4`, `.m4v` | MP4 | H.264 | AAC |
| `.mov` | QuickTime | H.264 | AAC |
| `.mkv` | Matroska | H.264 | AAC |
| `.webm` | WebM | VP9 | Opus |
| `.m4a` | MP4, audio only | | AAC |
| `.ogg`, `.oga`, `.opus` | Ogg, audio only | | Opus |
| `.flac` | FLAC, audio only | | FLAC |
| `.wav` | WAV, audio only | | 16-bit PCM |

H.264 is encoded in software, or by a hardware encoder when one is present
and the timeline's `encode.video.hardware` policy allows it. H.265 needs a
hardware encoder. VP9 and AV1 are encoded in software.
