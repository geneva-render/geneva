# Command-line reference

Timeline commands take a document; verbs take media files, build a
document and render it through the same path (`--show-timeline` prints
it instead).

## Global

| | |
| --- | --- |
| `--format human\|json` | `json` prints one JSON document on stdout. In `human` mode diagnostics go to stderr. |
| Times | `1.5`, `1.5s`, `1500ms`, `45f` (frames at the source's rate), `00:00:01.5`. See [Times](timeline.md#times). |

| Exit code | Meaning |
| --- | --- |
| 0 | Done. |
| 1 | The timeline is invalid; the diagnostics say why. |
| 2 | Usage error: unknown option, missing file, bad time, unknown code. |
| 3 | Rendering or encoding failed. |

### Progress

On stderr, only when frames are composited or encoded. Human: a line
rewritten in place, `frame 240/1800`. JSON: one object per line, at the
first frame, about once a second, and at the last.

```json
{"event":"progress","frames":540,"total":1800,"seconds":8.0,"rate":67.5,"time":18.0,"duration":60.0,"remaining":18.67}
```

| Field | Meaning |
| --- | --- |
| `frames`, `total` | Frames done, frames in the output. |
| `seconds`, `rate` | Work time so far, frames per second. |
| `time`, `duration` | Output seconds done, output length. |
| `remaining` | Seconds left at that rate; `null` on the first and last lines. |

## Timeline commands

### `geneva validate <timeline>`

Parses and validates; each diagnostic has a code ([errors.md](errors.md)),
the JSON path and a fix.

| Option | Effect |
| --- | --- |
| `--probe` | Also open the media assets: checks they exist, gives open-ended clips their lengths, measures audio (N310, W311 to W314). |
| `--assets DIR` | Asset root. Default: the timeline's directory. |

### `geneva frame <timeline|video>`

One picture, `.png` or `.jpg`.

| Option | Default | Effect |
| --- | --- | --- |
| `-o FILE` | `frame.png` | Output picture. |
| `--at TIME` | timeline: `0`; video: chosen | Time of the frame: the frame a render shows then, drawn as the render draws it. The chosen frame of a video is the first past the opening (20 frames or 5% in) that is not dark and follows motion, else the one a tenth in. |
| `--frame N` | | Frame number instead of a time. |
| `--width W`, `--height H` | the frame's | Picture size; one keeps the aspect. |
| `--assets DIR` | | Asset root, for a timeline. |
| `--show-timeline` | | With a video file: print the timeline instead. |

```sh
geneva frame job.json -o still.png --at 1.5s
geneva frame talk.mp4 -o thumb.jpg --at 12s --width 640
```

### `geneva render <timeline> -o FILE|DIR`

Renders the whole timeline. The container comes from the extension
unless `output.encode.container` sets it. With an
[`outputs`](timeline.md#outputs) map, `-o` is a directory and every
entry is written in one pass; the JSON report's `outputs[]` has `name`,
`kind`, `path`, `bytes`, `mode`, `content_type`, and `width`/`height`
for pictures and video. A sprite sheet's `.vtt` is its own row (`kind`
`sprites-map`).

| Option | Effect |
| --- | --- |
| `--crf N` | Constant quality, overriding the timeline; lower is better, 18 to 30 is the useful range for H.264 and H.265. Forces a re-encode. |
| `--preset NAME` | Encoder preset, `ultrafast` to `veryslow`. Forces a re-encode. |
| `--for TARGET`, `--quality`, `--budget` | Encode for a destination; see [targets](#targets). The timeline's size is kept. |
| `--no-audio` | No audio track. |
| `--renderer auto\|cpu\|gpu` | `auto`: a hardware GPU if present, else the CPU. `gpu`: any device, software ones included, else the CPU with a note. A software device (llvmpipe, WARP) is for testing and is several times slower than the CPU renderer: `auto` never picks one. `cpu`: the reference. `GENEVA_GPU=software` forces the software device. The note names the device. `frame` and overlays on a direct-path picture always use the CPU. |
| `--exact` | Frame-accurate cuts; a [smart cut](#smart-cut) where possible. |
| `--frames START..END` | Only output frames `START` to `END` (not included), picture only: one part for `geneva join`. Not with `--for`. |
| `--audio-only` | Only the sound, as the sound part for `join`. Not with `--for` or `--no-audio`. |
| `--assets DIR` | Asset root. |

### `geneva plan <timeline> -o FILE --parts N`

Cuts a render into `N` picture parts and one sound part that separate
processes or machines can render at the same time, and prints the
command for each and the `join` that puts them together. The
boundaries come from the planner behind `encode.video.chunks`: clip
starts when one is within a quarter of a part, else the keyframe grid,
else an even split. A part is at least 2 seconds, so a short output
gets fewer than `N` (reported as N600). The parts are named after `-o`
and take its container: `out.part-000.mp4` and so on, and
`out.audio.mp4`. JSON: `parts[]` with `frames` (`[start, end]`),
`start` and `end` in seconds, `output` and `command`; `audio` with
`output` and `command`, or `null` for a container without sound; and
`join`.

```text
$ geneva plan talk.json -o out.mp4 --parts 3
3 parts of 5400 frames in all; run each of these, on any machine with the same geneva and the assets:
  geneva render talk.json --frames 0..1800 -o out.part-000.mp4
  geneva render talk.json --frames 1800..3600 -o out.part-001.mp4
  geneva render talk.json --frames 3600..5400 -o out.part-002.mp4
  geneva render talk.json --audio-only -o out.audio.mp4
then put them together:
  geneva join talk.json out.part-000.mp4 out.part-001.mp4 out.part-002.mp4 --audio out.audio.mp4 -o out.mp4
```

Refused (E504, exit 3) for a document with `outputs`, a bitrate target
or ceiling (it holds over the whole file and each part would restart
it), an image sequence, or an output with no picture.

### `geneva join <timeline> <part>... [--audio FILE] -o FILE`

Puts the parts together without re-encoding: the picture parts in
order, the sound part beside them, and the timeline's subtitle tracks
and fast start as `render` would write them. Parts that do not add up
to the timeline's frame count are refused, so a missing or repeated
part is an error rather than a shorter file. Parts whose encoder
parameters differ are refused, and the message names the difference.
Without `--audio` the output has no sound, with a warning when the
timeline has some.

What is different from one `render`:

- Every part is encoded by the CPU renderer's path (or the direct path
  where the picture is a source's own). The `copy`, `copy-picture` and
  `smart` [modes](#output-modes) are not used, so a timeline that one
  render would copy is slower and larger in parts.
- Each part restarts the encoder's rate control, so the bits differ
  from a single run; the pixels barely do (above 40 dB PSNR in the
  tests).
- Every machine needs the same geneva and the same encoder: a part
  from a machine with x264 and one from a machine without are refused
  at the join. Give every part the same `--crf` and `--preset`.
- The assets must be on every machine, at the same paths relative to
  the timeline (or `--assets`).

### `geneva probe <file>`

Container, duration, streams, sizes, rates and colour tags, with the
tags that had to be assumed. Sizes are as displayed; `rotation` (0, 90,
180, 270, clockwise) and the stored size are reported beside them.

### `geneva schema`

The JSON Schema of the current format version.

### `geneva targets`

The `--for` table below, with each platform number's source and date.

### `geneva guide [TOPIC] [--list]`

A page of this manual, carried in the binary. No topic: the agent
guide. Topics: `agents`, `timeline`, `cli`, `errors`, `color`,
`architecture`. JSON: `{"topic", "about", "text"}`.

### `geneva explain <CODE> [--list]`

What a diagnostic code means, its section and severity. Any case. Every
meaning of a code with several is printed. `--list`: every code.
Unknown code: exit 2.

```text
$ geneva explain e302
error[E302]: Clips in the same layer or audio track overlap without a transition.
  in Timing (E300-E399)
```

## Output modes

The report's `mode`, chosen per output, cheapest first.

| Mode | When | Picture | Sound |
| --- | --- | --- | --- |
| `copy` | Sources used as they are, one codec with identical parameters, nothing asks for a re-encode. | Packets copied; cuts move back to a keyframe, and the report gives the times used. | Copied. |
| `copy-picture` | As `copy`, but the sound must change: loudness, hygiene, or another codec, bitrate, rate or channel count. | Copied. | Mixed and encoded. `frames: 0`. |
| `smart` | H.264 shown as it is, with an exact cut or an overlay for part of the time. | Copied where untouched, encoded around the changes. | Copied as coded when it can be, else encoded. |
| `direct` | Picture untouched or only scaled, cropped or barred, but not copyable (another codec or size, a quality setting, `--exact`). | Decoded frames go straight to the encoder; overlays are drawn on only the pixels they cover. | Encoded. |
| `render` | Anything else. | Composited. | Mixed and encoded. |

Re-encoded audio keeps the source's sample rate and channel count when
every input agrees, else 48 kHz stereo.

### Smart cut

Needs: H.264 4:2:0 shown as it is at the output size and rate; MP4, MOV
or Matroska out; no `--crf`, `--preset` or `--for`; at least a fifth of
the frames copyable; x264 installed on the system (`libx264-164` on Debian 12 and Ubuntu 24.04, `brew install x264` on macOS, `libx264-<build>.dll` beside `geneva.exe` or named by `GENEVA_X264` on Windows).

- With `--exact`, each cut re-encodes up to the source's next keyframe;
  an overlay re-encodes the frames it touches, to the next keyframe. A
  `concat` join on a keyframe costs nothing.
- Encoded runs are CRF 18. Copied frames are the source's bytes.
- Audio of one kind in every clip is copied: the file starts on the
  exact sample, later joins land within half a packet (11 ms for AAC),
  and the error does not accumulate. A join can click softly on loud
  material.
- A source with a keyframe every ten seconds may have none inside a
  short trim, and then the whole trim is encoded.

## Verbs

### Common options

Every verb takes `-o FILE` and these. Verbs write the sources' colour
encoding when every video input shares one; `render` defaults to BT.709.

| Option | Effect |
| --- | --- |
| `--crf N`, `--preset NAME` | As for `render`. Force a re-encode. |
| `--codec NAME` | `h264`, `h265`, `vp9`, `av1`, `prores`, `dnxhd`, `png`, `mjpeg`. Default: the container's. |
| `--profile NAME` | ProRes `proxy`, `lt`, `standard`, `hq`, `4444`, `4444-xq`; DNxHR `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq`, `dnxhr-hqx`, `dnxhr-444`. Implies the codec. |
| `--tune NAME` | x264 names: `film`, `animation`, `grain`, `stillimage`, `fastdecode`, `zerolatency`. Unsupported ones are reported. Forces a re-encode. |
| `--keyframe-interval SECONDS` | Forces a re-encode. |
| `--fixed-keyframes` | Keyframes at the interval only, not at scene changes. Needs `--keyframe-interval` or `--for`. |
| `--max-bitrate RATE` | Video ceiling: `6M`, `800k`, or a bare number of kb/s. See [rate control](#rate-control). Forces a re-encode; wins over `--for`. |
| `--audio-codec NAME` | `aac`, `opus`, `mp3`, `vorbis`, `ac3`, `flac`, `alac`, `pcm`, `pcm24`. Default: the container's. |
| `--audio-bitrate RATE` | `128k` or kb/s; default 160. None for lossless and PCM. |
| `--sample-rate HZ` | `48000` or `44.1k`. Default: the sources' if shared, else 48 kHz. |
| `--channels 1\|2` | Default: the sources', at most 2. |
| `--no-audio` | No audio track. |
| `--keep-hdr` | Keep HDR tags and a ten-bit codec (`h265` unless `--codec`; it needs a hardware encoder, `av1` and `vp9` are software). Default: tone-map to SDR. |
| `--chunks N\|auto` | Encode in `N` stretches at once; `auto` default, `1` off. See `encode.video.chunks` in [timeline.md](timeline.md#output). |
| `--for TARGET` | Encode for a destination; see [targets](#targets). A source that already fits is copied; one whose sound alone falls short gets `copy-picture`. `--crf`, `--quality`, `--budget` and `--exact` re-encode regardless. |
| `--quality best\|good\|eco` | Tier for `--for`; `good` default. |
| `--budget SIZE` | With `--for`: lower the bitrate cap to fit `SIZE` (`25MB`); W412 if it still does not. |
| `--fill bars\|blur` | Around a picture that does not cover the frame. `--for` portrait canvases default to `blur`, a `--width`/`--height` change to `bars`. |
| `--renderer auto\|cpu\|gpu` | As for `render`; affects only composited frames. |
| `--exact` | Frame-accurate cuts; a [smart cut](#smart-cut) when possible. |
| `--show-timeline` | Print the document instead of rendering; the asset root is printed on stderr. Inputs on two Windows drives share no root, so their paths are written whole; `geneva render` refuses such a document, since a written one keeps its paths under its root. |

### Rate control

Constant quality (`--crf`) with an optional ceiling (`--max-bitrate`),
ffmpeg's `-crf 23 -maxrate 6M -bufsize 12M`. The only mode.

- No fixed average bitrate, CBR or two-pass. For a size, use
  `--for TARGET --budget SIZE`.
- The ceiling holds over a two-second buffer, so short files can exceed
  it by up to the buffer: capped at 100k, 190 kb/s over 2 s, 110 kb/s
  over 20 s.
- VP9: the ceiling is libvpx's constrained quality bitrate, an average.
- VideoToolbox and NVENC at constant quality, and the bundled OpenH264
  always, ignore the ceiling, and the report says so. `--budget` puts
  VideoToolbox in bitrate mode.
- Sound is copied when it already matches what is asked, else encoded;
  the picture is still copied if nothing else changes it. More than two
  source channels are copied as they are unless the sound is encoded.

### `geneva convert <input> -o FILE`

Re-encode, optionally changing container, codec, size or rate.

| Option | Effect |
| --- | --- |
| `--width W`, `--height H` | Output size; one keeps the aspect. Rounded to even. |
| `--fps FPS` | `30` or `30000/1001`. |
| `--fit contain\|cover\|fill` | When the shape changes, and on a canvas `--for` builds. `contain` default. |
| `--crop RECT` | `X,Y,WxH` or `WxH` (centred), pixels or percentages: `240,0,1440x1080`, `56%x100%`. The output takes its size unless one is given. Scaled straight from the decoder. |
| `--speed FACTOR` | `2` twice as fast, `0.5` half; the pitch follows. |

`--fill blur` adds a first layer named `fill` (a blurred, cover-fitted
copy with no audio); it decodes the source twice.

```sh
geneva convert talk.mov -o talk.mp4
geneva convert talk.mov -o talk.mp4 --crf 20 --preset slow --fps 30 --height 1080
geneva convert talk.mp4 -o talk.webm --codec vp9 --crf 32
geneva convert talk.mov -o talk.mkv --max-bitrate 6M --audio-codec opus --audio-bitrate 128k
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fit cover
geneva convert talk.mp4 -o square.mp4 --crop 1080x1080
geneva convert talk.mp4 -o frames/%04d.png
```

### `geneva resize <input> -o FILE`

`convert` with `--width` or `--height` required. Also `--crop`, `--fit`.

### `geneva trim <input> -o FILE`

| Option | Effect |
| --- | --- |
| `--from TIME` | Start; default the beginning. |
| `--to TIME` or `--duration TIME` | End, or length; default the end. |
| `--crop RECT` | As on `convert`; re-encodes. |
| `--speed FACTOR` | As on `convert`. |

Copied by default, the cut moved back to a keyframe; `--exact` for the
exact frame.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s
geneva trim talk.mp4 -o clip.mp4 --from 1:02:10 --duration 45s --exact
```

### `geneva concat <input>... -o FILE`

Joins back to back. Identical stream parameters are copied; anything
else is rendered, other shapes fitted into the first input's frame.

| Option | Effect |
| --- | --- |
| `--crossfade TIME` | Dissolve; sound crosses at constant power. Renders. |
| `--fade TIME` | Dip through a colour; sound goes to silence at the midpoint. Renders. |
| `--fade-color COLOR` | Colour for `--fade`; black default. |

```sh
geneva concat part1.mp4 part2.mp4 part3.mp4 -o all.mp4
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s
geneva concat a.mp4 b.mp4 -o ab.mp4 --fade 0.6s --fade-color white
```

### `geneva overlay <input> <overlay> -o FILE`

An image or a video (without its audio) over the input.

| Option | Default | Effect |
| --- | --- | --- |
| `--at POS` | `top-right` | `top-right`, `top-left`, `bottom-left`, `bottom-right`, `center`. |
| `--margin PX` | 24 | Distance from the edges. |
| `--scale F` | 1 | Scale of the overlay. |
| `--opacity F` | 1 | 0 to 1. |
| `--start TIME`, `--duration TIME` | the whole video | When it shows. |

```sh
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5 --opacity 0.8
```

### `geneva audio <input> -o FILE`

One operation:

| Operation | Output |
| --- | --- |
| `--extract` | The audio alone: `.m4a` (AAC), `.ogg` (Opus), `.flac`, `.wav`. Copied when the codec already fits. |
| `--extract --speech` | 16 kHz mono, for speech recognizers. |
| `--mute` | The video without audio. |
| `--replace FILE` | Another file's audio, cut to the video's length. |
| `--mix FILE [--gain DB]` | Both mixed; `--gain` applies to the added file. |

```sh
geneva audio talk.mp4 -o talk.wav --extract --speech
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
```

### `geneva subtitles <input> -o FILE`

One operation:

| Operation | Output |
| --- | --- |
| `--add FILE` (repeatable) `[--language CODE]...` | `.srt` or `.vtt` attached as text streams; languages pair with files in order. Picture and sound copied when possible. |
| `--burn FILE` | Cues drawn into the picture. `.srt`, `.vtt`, or a speech recognizer's `.json`; words grouped into cues of at most two lines. |
| `--extract [--track N]` | Subtitle stream `N` (default 0) as `.srt` or `.vtt`, by extension. Text subtitles only. |

`--burn` options:

| Option | Default | Effect |
| --- | --- | --- |
| `--position bottom\|top\|center` | `bottom` | Where cues sit. |
| `--margin PX` | the safe inset | Distance from the edge. |
| `--safe PERCENT` | 5 | Title-safe inset from each edge; 0 off. |
| `--style JSON` | white, semi-bold, outline, soft shadow, 90% wide | [Text source](timeline.md#sources) fields merged over the default; [CSS shorthands](timeline.md#css-shorthands) work. |
| `--highlight COLOR` | | Colour of the word being said; needs word times, else W453. |
| `--fit` | off | Shrink a cue that does not fit the safe area, down to half size (N405). |

Every cue is laid out before rendering: one leaving the frame is W403,
one outside the safe area N404. Long cues wrap; the size changes only
with `--fit`. Tags such as `<i>` are removed.

```sh
geneva subtitles talk.mp4 -o talk.mkv --add en.srt --add fr.srt --language en --language fr
geneva subtitles talk.mp4 -o burned.mp4 --burn transcript.json --highlight "#ffd233"
geneva subtitles talk.mp4 -o burned.mp4 --burn en.srt --style '{"font": "600 40px Inter", "background": "#00000080", "padding": "8px"}'
geneva subtitles talk.mkv -o fr.vtt --extract --track 1
```

## Targets

`--for TARGET` sets the encode block from a table: `geneva targets`
prints it (JSON with `--format json`), and `--show-timeline` shows what
was picked.

| Target | Ceiling | Loudness | About |
| --- | --- | --- | --- |
| `phone` | 1920×1080 | | Fullscreen on a phone. |
| `tablet` | 2560×1600 | | A tablet. |
| `desktop` | 2560×1440 | | A laptop or desktop screen. |
| `tv` | 3840×2160 | | A television. |
| `web` | 1920×1080 | | A page or player on the web. |
| `youtube` | 3840×2160 | -14 LUFS | Keyframes every 0.5 s, AAC 384 kb/s, per its upload guide. |
| `instagram` | 1440×2560, 9:16 | -14 LUFS | Reels. |
| `tiktok` | 1080×1920, 9:16 | -14 LUFS | |
| `podcast` | 1920×1080 | -16 LUFS | Also turns on `output.audio.hygiene`. |
| `x` | 1920×1200 | | 2:20 and 512 MB limits. |
| `linkedin` | 4096×2304 | | 10 min and 5 GB limits. |
| `email` | 1280×720 | | 25 MB budget. |

Rules:

1. Never upscale; shrink to even dimensions within the ceiling.
2. Keep the aspect, except 9:16 targets: a landscape source goes on a
   portrait canvas of its own width over a blurred copy (`--fill bars`
   for the background colour, `--fit cover` to crop the centre).
3. H.264 High at the level the size and rate need (`3.1` for 720p, `4.0`
   or `4.2` for 1080p, `5.1` or `5.2` for 4K). `--codec` overrides.
4. CRF by tier, `best`/`good`/`eco`: 20/23/26 for most targets, two lower
   at 4K, 18/20/23 for YouTube, 23/26/28 for email. `--crf` overrides.
5. The target's bitrate cap, raised by half above 30 fps and lowered to
   fit a size limit or `--budget`. VideoToolbox ignores the cap (noted);
   `--budget` puts it in bitrate mode.
6. Keyframes every 2 s, fast start, at most 60 fps.
7. Limits that cannot be met are warnings: too long (W411), too large
   after encoding (W412), not MP4 (W413), budget too small (W414).
8. Loudness targets bring the mix to the level with true peak under
   -1 dBTP. A source counts as already fitting only if its audio is
   within 1 LU with peaks under the ceiling, and never for `podcast`.

Notes in the report: one N410 with every choice, and one N600 each for
what hygiene and loudness measured and did.

```text
note[N410]: target phone: 3840×2160 scaled to 1920×1080 (ceiling 1920×1080);
  CRF 23 (good); H.264 High level 4.0; capped at 10000 kb/s;
  keyframes every 2 s; fast start; AAC 128 kb/s 48 kHz stereo
note[N600]: loudness: measured -23.4 LUFS, +9.4 dB to reach -14 LUFS, true peak held under -1 dBTP
```

```sh
geneva convert master.mov -o phone.mp4 --for phone
geneva convert master.mov -o reel.mp4 --for instagram --quality best
geneva trim master.mov -o clip.mp4 --from 1m --duration 2m --for x
geneva convert talk.mp4 -o talk-email.mp4 --for email --budget 20MB
```

## Containers and codecs

| Extension | Container | Video | Audio | Defaults |
| --- | --- | --- | --- | --- |
| `.mp4`, `.m4v` | MP4 | H.264, H.265, VP9, AV1, Motion JPEG | AAC, MP3, ALAC, AC-3, Opus, FLAC | H.264, AAC |
| `.mov` | QuickTime | all | AAC, MP3, ALAC, AC-3, PCM, FLAC | H.264, AAC |
| `.mkv` | Matroska | all but PNG | all | H.264, AAC |
| `.webm` | WebM | VP9, AV1 | Opus, Vorbis | VP9, Opus |
| `.mxf` | MXF | DNxHR, ProRes, H.264 | PCM | DNxHR HQ, 24-bit PCM |
| `.png`, `.jpg` with a `%04d` pattern | image sequence | PNG, Motion JPEG | none | by extension |
| `.m4a` | MP4, audio only | | AAC, MP3, ALAC, AC-3 | AAC |
| `.ogg`, `.oga`, `.opus` | Ogg, audio only | | Opus, Vorbis, FLAC | Opus |
| `.flac` | FLAC | | FLAC | FLAC |
| `.wav` | WAV | | PCM | 16-bit PCM |
| `.mp3` | MP3 | | MP3 | MP3 |

Subtitle streams: MP4, MOV, MKV, WebM.

| Codec | Layout | Encoder |
| --- | --- | --- |
| H.264 | 8-bit 4:2:0 | Hardware if present and allowed, else the system's x264, else the bundled OpenH264. The notes say which. |
| H.265 | 8-bit 4:2:0 | Hardware only. |
| VP9, AV1 | 8-bit 4:2:0 | Software. |
| ProRes | 10-bit 4:2:2; 4:4:4 for `4444`, `4444-xq` | `hq` default. |
| DNxHR | 8-bit 4:2:2; 10-bit for `dnxhr-hqx`; 10-bit 4:4:4 for `dnxhr-444` | `dnxhr-hq` default. At least 256×120. |
| PNG | 8-bit RGBA | Lossless, keeps alpha. |
| Motion JPEG | 8-bit 4:2:0, full range | `--crf` maps to JPEG quality (0 best). |

Also read, not written: VP8, MPEG-2, MPEG-4 part 2, DNxHD, raw video,
GIF, E-AC-3, MPEG-TS, AVI.
