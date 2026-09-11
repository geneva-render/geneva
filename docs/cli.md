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
frame-accurate at the cost of a re-encode. When the picture is untouched
or only scaled to fill the frame but a re-encode is still needed (another
codec, a size, a quality setting, `--exact`), decoded frames go straight
to the encoder, scaled and repacked on the way when the size or the
sample layout differs, without passing through the compositor; the report
calls this mode `direct`.

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
| `--codec h264\|h265\|vp9\|av1\|prores\|dnxhd\|png\|mjpeg` | Video codec. Defaults to the container's usual one. |
| `--profile NAME` | Codec profile: `proxy`, `lt`, `standard`, `hq`, `4444`, `4444-xq` for ProRes; `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq`, `dnxhr-hqx`, `dnxhr-444` for DNxHR. Implies the codec. |
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

### `geneva subtitles <input> -o FILE --add FILE... [--language CODE...] | --extract [--track N]`

| Operation | Output |
| --- | --- |
| `--add FILE` (repeatable) | The input with each subtitle file (`.srt` or `.vtt`) attached as a text stream; `--language` values pair with the files in order. The picture and sound are copied when nothing else changes. |
| `--extract` | The input's subtitle stream number `--track` (0 by default) written as `.srt` or `.vtt`, by the output's extension. Only text subtitles can be extracted. |

```sh
geneva subtitles talk.mp4 -o talk-subbed.mkv --add en.srt --add fr.srt --language en --language fr
geneva subtitles talk-subbed.mkv -o fr.vtt --extract --track 1
```

## Containers and codecs

| Extension | Container | Video it holds | Audio it holds | Defaults |
| --- | --- | --- | --- | --- |
| `.mp4`, `.m4v` | MP4 | H.264, H.265, VP9, AV1, Motion JPEG | AAC, MP3, ALAC, AC-3, Opus, FLAC | H.264, AAC |
| `.mov` | QuickTime | all | AAC, MP3, ALAC, AC-3, PCM, FLAC | H.264, AAC |
| `.mkv` | Matroska | all but PNG | all | H.264, AAC |
| `.webm` | WebM | VP9, AV1 | Opus, Vorbis | VP9, Opus |
| `.mxf` | MXF | DNxHR, ProRes, H.264 | PCM | DNxHR HQ, 24-bit PCM |
| `.png`, `.jpg` with a `%04d`-style pattern | image sequence | PNG, Motion JPEG | none | by extension |
| `.m4a` | MP4, audio only | | AAC, MP3, ALAC, AC-3 | AAC |
| `.ogg`, `.oga`, `.opus` | Ogg, audio only | | Opus, Vorbis, FLAC | Opus |
| `.flac` | FLAC, audio only | | FLAC | FLAC |
| `.wav` | WAV, audio only | | PCM | 16-bit PCM |
| `.mp3` | MP3, audio only | | MP3 | MP3 |

Subtitle streams go into MP4, MOV, MKV and WebM.

Video codecs and their sample layouts:

| Codec | Layout | Notes |
| --- | --- | --- |
| H.264 | 8-bit 4:2:0 | Software encoder, or a hardware encoder when present and `encode.video.hardware` allows it. |
| H.265 | 8-bit 4:2:0 | Hardware encoders only. |
| VP9, AV1 | 8-bit 4:2:0 | Software. |
| ProRes | 10-bit 4:2:2; 4:4:4 for `4444` and `4444-xq` | Profiles `proxy`, `lt`, `standard`, `hq` (default), `4444`, `4444-xq`. |
| DNxHR | 8-bit 4:2:2; 10-bit for `dnxhr-hqx`; 10-bit 4:4:4 for `dnxhr-444` | Profiles `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq` (default), `dnxhr-hqx`, `dnxhr-444`. Needs at least 256×120. |
| PNG | 8-bit RGBA | Lossless; keeps transparency. |
| Motion JPEG | 8-bit 4:2:0, full range | `--crf` maps onto JPEG quality (0 best). |

Reading is wider than writing: sources may also be VP8, MPEG-2, MPEG-4
part 2, DNxHD, raw video or GIF, with E-AC-3 audio, in MPEG-TS or AVI as
well as the containers above.

## Checking an installation

`check.sh`, shipped in every release archive (and at `scripts/check.sh`
in the repository), runs the everyday commands on one machine and prints a
table with the time each step took, the size of its output, and the time
ffmpeg takes for the same step when ffmpeg is installed:

```sh
sh check.sh                      # renders a 10-second test clip and works on it
sh check.sh input.mp4            # works on your own file instead
sh check.sh --quick              # skips the slower encodes (VP9, ProRes, DNxHR, PNG)
sh check.sh --keep               # keeps the outputs and prints where they are
sh check.sh --geneva target/release/geneva   # a specific binary
```

Every step's full output goes to `log.txt` in the working directory; when
a step fails the log is kept and its path printed.
