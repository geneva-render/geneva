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

## Progress

A render that has frames to composite says how far it has got on stderr,
so stdout stays the one document the report is. In human mode that is a
line rewritten in place, `frame 240/1800`. With `--format json` it is one
object per line, at the first frame, about once a second, and at the last:

```json
{"event":"progress","frames":540,"total":1800,"seconds":8.0,"rate":67.5,"time":18.0,"duration":60.0,"remaining":18.67}
```

`frames` of `total` are done after `seconds` of work at `rate` frames a
second, which is `time` of the output's `duration` in seconds, with
`remaining` seconds left at that rate. `remaining` is `null` when there
is nothing to estimate: on the last line, and on the first, whose half
second measures the startup as much as the work. A render that copies its
streams, or writes audio alone, has no frames to report and says nothing.

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

### `geneva frame <timeline|video> [--at TIME | --frame N] [-o FILE] [--width W] [--height H]`

One picture, as `.png` (default `frame.png`) or `.jpg`, at the frame's
size unless `--width` or `--height` says otherwise (one keeps the
aspect).

From a timeline, the frame at `--at` (default 0) through the same
renderer as `render`. From a video file, the frame at `--at`, or with no
time the chosen one: the first clear frame after the opening (past the
first twenty frames or 5%, not dark, following some motion), so a black
leader or a phone clip's opening blur is skipped; failing that, the
frame a tenth of the way in. Only the frames looked at are decoded, so
it takes a fraction of a second. This is the `<video poster>` image,
the library thumbnail, the preview card.

```sh
geneva frame job.json -o still.png --at 1.5s
geneva frame talk.mp4 -o thumb.jpg                  # chosen frame, the video's size
geneva frame talk.mp4 -o thumb.jpg --at 12s --width 640
```

### `geneva render <timeline> -o FILE|DIR [options]`

Renders the whole timeline. The output container comes from the file
extension unless the timeline sets `output.encode.container`. When the
timeline has an [`outputs`](timeline.md#outputs) map, `-o` names a
directory instead, every entry is written into it from one pass over the
composition, and the report lists each file (`outputs[]` with `name`,
`kind`, `path`, `bytes`, `mode`, `content_type` and, for pictures and
video, `width` and `height`, in JSON; one `wrote` line each in human
mode). A sprite sheet's `.vtt` map is its own row (`kind`
`sprites-map`), so the list is every file written and nothing else.

| Option | Effect |
| --- | --- |
| `--crf N` | Constant-quality level, overriding the timeline. Lower is better; 18 to 30 is the useful range for H.264 and H.265. |
| `--preset NAME` | Encoder speed preset, `ultrafast` to `veryslow`. |
| `--no-audio` | Write no audio track. |
| `--renderer auto\|cpu\|gpu` | Which renderer composites the frames. `auto` takes the GPU where the machine has a hardware one (discrete, integrated, or a virtual machine's) and the CPU otherwise; `gpu` takes whatever device opens, a software one such as Mesa's lavapipe included, and falls back to the CPU with a note when none does; `cpu` is the reference renderer. On the GPU the frame is composited and packed into the encoder's planes on the device, and the next frame is drawn while the current one is read back. The report's notes name the device. The two renderers agree to within one 8-bit code on every golden case but the blur one, where 71 pixels, 0.12%, differ by up to three; that is the blur's own rounding, not a particular device, and a Metal and a Vulkan device report the same pixels; the overlays over a copied picture, and `frame`, are always drawn on the CPU. `GENEVA_GPU=software` makes either choice take the software device, for checking. |
| `--exact` | Cut on the exact frame instead of moving cuts to keyframes; see [smart cut](#smart-cut). |
| `--for TARGET`, `--quality`, `--budget` | Encode for a destination; see [targets](#targets). On `render` the timeline's size is kept; only the encode block is set. |
| `--fill bars\|blur` | What surrounds a picture that does not cover its frame (a landscape video on a portrait canvas): the background color, or a blurred, scaled-up copy of the picture behind it. A `--for` target that builds a portrait canvas fills with the blurred copy unless this says `bars`; a `--width`/`--height` resize leaves bars unless this says `blur`, since the sizes given are a request for those sizes and not for a second pass over the source. Verbs only. |

When the composition uses its sources as they are, the coded streams are
copied instead of re-encoded and the report says so
([stream copy](architecture.md#stream-copy)). Cuts then move to the
nearest earlier keyframe, which the report also lists; `--exact` makes cuts
frame-accurate at the cost of a re-encode. When the picture is untouched
or only scaled, with or without bars, but a re-encode is still needed
(another codec, a size, a quality setting, `--exact`), decoded frames go straight
to the encoder, scaled and repacked on the way when the size or the
sample layout differs, without passing through the compositor; the report
calls this mode `direct`. Audio that is re-encoded keeps the source's
sample rate and channel count when every input agrees (mono stays mono,
44.1 kHz stays 44.1 kHz); otherwise it is written as 48 kHz stereo.

### A copied picture with encoded sound

Bringing a mix to a loudness, cleaning it, or asking for another audio
codec, bitrate, sample rate or channel count is a change no copied audio
track can carry, but it says nothing about the picture. When
nothing else asks for a re-encode, the video packets are copied and only
the sound is mixed, treated and encoded beside them; the report calls
this mode `copy-picture` and gives `frames: 0`, since no frame reaches an
encoder. The two tracks are written together, so the file interleaves as
a copy does.

This is what `--for podcast` does on a recording that is already H.264 in
an MP4 within the target's ceilings: the loudness and the hygiene are
applied to the sound alone. On three minutes of 1080p30 here that is 4 s
against 80 s for the same command re-encoding, and the video stream comes
out byte for byte identical to the source's.

### Smart cut

Between "copy everything, cuts move to keyframes" and "re-encode
everything" there is a third mode, `smart`: the source's coded packets
are copied wherever nothing changes, and only the frames that do change
are encoded, into the same stream. With `--exact`, that is the run from
each cut to the next keyframe of the source. With an overlay or a
burned-in subtitle shown for part of the time, it is the frames it
touches, again up to the next keyframe. A join in a `concat` costs
nothing when it lands on a keyframe. The report says how many frames
were copied and how many encoded, and the copied frames are
bit-identical to the source's. When every clip brings its own audio of
one kind the container takes, the audio is copied as coded too: the
file starts on the exact sample, and each later join lands within half
an audio packet (11 ms for AAC) of the cut, an error that does not add
up across joins; the first packet after a join is decoded without its
overlap, which can be heard as a soft click on loud material.

It applies when the source is H.264 4:2:0 shown as it is at the output
size and rate, the output is MP4, MOV or Matroska with H.264, no
quality setting (`--crf`, `--preset`, `--for`) asks for a re-encode,
at least a fifth of the frames can be copied (below that a plain encode
is faster, since the runs go at a higher quality), and the system's
x264 is installed (see the README). The encoded runs use
CRF 18 so that they sit next to the source's own pictures without a
visible step, and their parameter sets go into the file next to the
source's, which every H.264 decoder handles. How much is saved depends
on the source's keyframe spacing: a phone recording with a keyframe
every second re-encodes at most a second per cut; an x264 default
encode with one every ten seconds may have no keyframe inside a short
trim at all, in which case the whole trim is encoded as before.

### `geneva probe <file>`

Shows the container, duration, streams, sizes, rates, and the color tags of
a media file, including which tags had to be assumed. Sizes are as
displayed: a phone's portrait clip is stored as a landscape stream with a
rotation, and the probe reports it as portrait, with the stored size and
the rotation alongside (`rotation` in the JSON: 0, 90, 180 or 270 degrees
clockwise).

### `geneva schema`

Prints the JSON Schema of the current timeline format version.

### `geneva guide [TOPIC] [--list]`

Prints a page of this manual, which is carried in the binary. With no
topic it prints the guide for programs and agents, the page that
describes the calling contract, the report shape and the loop to write
a document in. `--list` names the pages: `agents`, `timeline`, `cli`,
`errors` and `color`. Under `--format json` the page comes back as
`{"topic", "about", "text"}` rather than Markdown on stdout.

This is here because a machine that installed the release tarball has
the binary and not `docs/`, and because a page printed by the binary is
the page for that binary's version.

### `geneva explain <CODE> [--list]`

Says what a diagnostic code means, the section it belongs to and its
severity, reading the same table `geneva guide errors` prints. Case
does not matter, so a code taken straight out of a JSON report works.
A code can have more than one meaning (`W304` has two), and every one
is printed. `--list` prints every documented code with its one-line
meaning. An unknown code exits 2.

```text
$ geneva explain e302
error[E302]: Clips in the same layer or audio track overlap without a transition.
  in Timing (E300-E399)
```

## Everyday verbs

All verbs take `-o FILE` for the output and these encoding options:

The verbs write the output in the source's color encoding when every video
input shares one (as tagged, or as inferred from its size for untagged
material), so standard-definition sources stay BT.601 and the tags travel
into the file; `render` defaults to BT.709 unless `output.color` says
otherwise. The printed timeline (`--show-timeline`) shows the choice.

| Option | Effect |
| --- | --- |
| `--crf N`, `--preset NAME` | As for `render`. Setting either forces a re-encode. |
| `--for TARGET` | Encode for a destination: a size ceiling (never upscaled), codec and level, quality, bitrate cap, keyframes, fast start and audio from one table; see [targets](#targets). A source used as it is that already fits the target (H.264 4:2:0 with AAC in MP4 or MOV, within the size, frame-rate and bitrate ceilings) is copied instead, and the note says so. When only the sound falls short, as under `--for podcast`, the picture is still copied and the mix alone is encoded. `--crf`, `--quality`, `--budget` or `--exact` re-encode it regardless. |
| `--quality best\|good\|eco` | Quality tier for `--for`; `good` by default. |
| `--budget SIZE` | With `--for`: cap the bitrate so that the file fits `SIZE` (for example `25MB`), and warn when it still does not. |
| `--codec h264\|h265\|vp9\|av1\|prores\|dnxhd\|png\|mjpeg` | Video codec. Defaults to the container's usual one. |
| `--profile NAME` | Codec profile: `proxy`, `lt`, `standard`, `hq`, `4444`, `4444-xq` for ProRes; `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq`, `dnxhr-hqx`, `dnxhr-444` for DNxHR. Implies the codec. |
| `--tune NAME` | What the picture is like, in x264's names: `film`, `animation`, `grain`, `stillimage`, `fastdecode`, `zerolatency`. Encoders without an equivalent say so in the report. Forces a re-encode. |
| `--keyframe-interval SECONDS` | Seconds between keyframes. `--for` sets its own. Forces a re-encode. |
| `--fixed-keyframes` | Keyframes at the interval only, never at scene changes, for streaming platforms and segmenters. Needs `--keyframe-interval` or `--for`. Forces a re-encode. |
| `--keep-hdr` | Keep HDR sources HDR: the output takes their tags (PQ or HLG, BT.2020) and a ten-bit codec, `h265` unless `--codec` says otherwise (`h265` needs a hardware encoder; `av1` and `vp9` are software). Without it, HDR sources are tone-mapped to SDR. |
| `--chunks N\|auto` | Encode the output in `N` stretches at once, on separate cores, joined afterwards; `auto` (the default) decides from the encoder and the machine, `1` turns it off. See `encode.video.chunks` in [timeline.md](timeline.md). |
| `--max-bitrate RATE` | A ceiling on the video bitrate, such as `6M` or `800k`; a bare number is kb/s, not ffmpeg's bits per second. The quality stays constant until the ceiling bites; see [rate control](#rate-control). Forces a re-encode, and wins over a `--for` target's own cap. |
| `--audio-codec NAME` | Audio codec: `aac`, `opus`, `mp3`, `vorbis`, `ac3`, `flac`, `alac`, `pcm` or `pcm24`. Defaults to the container's usual one. |
| `--audio-bitrate RATE` | Audio bitrate, such as `128k`; 160 kb/s by default. Lossless and PCM codecs have none. |
| `--sample-rate HZ` | Audio sample rate, such as `48000` or `44.1k`. Defaults to the source's when every input shares one, otherwise 48 kHz. |
| `--channels 1\|2` | Mono or stereo. Defaults to the source's, at most two. |
| `--no-audio` | Write no audio track. |
| `--renderer auto\|cpu\|gpu` | Which renderer composites the frames, as for `render`. Only the steps that composite are affected: a stream that is copied is copied either way, and the overlays drawn onto a copied picture are always drawn on the CPU. A verb that builds a canvas, such as a `--for` target that changes the shape, composites every frame and does take it. |
| `--exact` | Cut on the exact frame; a [smart cut](#smart-cut) when the source allows. |
| `--show-timeline` | Print the timeline the verb built instead of rendering it. Asset paths in it are relative to the directory printed on stderr. |

### Rate control

The video is encoded at a constant quality, `--crf`, with an optional
ceiling, `--max-bitrate`. In ffmpeg's terms that is `-crf 23 -maxrate 6M
-bufsize 12M`, and it is the only mode:

- There is no fixed average bitrate or constant bitrate (ffmpeg's
  `-b:v 6M` on its own) and no two-pass encode. For a file of a given
  size, `--for TARGET --budget SIZE` lowers the ceiling until the file
  should fit, and warns when it does not.
- The ceiling is held over a buffer of two seconds at that rate, so any
  two seconds stay under it, but a short file can end up above its rate
  by as much as the buffer: capped at 100k, a 2 s clip measured 190 kb/s
  and a 20 s one 110 kb/s.
- VP9 takes the ceiling as the bitrate of libvpx's constrained quality
  mode, which holds the average rather than every two seconds.
- VideoToolbox and NVENC encode at a constant quality and do not apply
  the ceiling; the report says so. `--budget` switches VideoToolbox to
  its bitrate mode.

The sound is copied when the output asks for what the source already
has, and encoded when the codec, sample rate or channel count differs or
a bitrate is asked for. The picture is still copied beside it when
nothing else changes. A source with more than two channels is copied as
it is unless something else re-encodes the sound, since geneva writes at
most two.

### `geneva convert <input> -o FILE [--crop RECT] [--width W] [--height H] [--fit contain|cover|fill] [--fps FPS]`

Re-encodes a video, optionally to another container, codec, size or frame
rate. Giving one of `--width` and `--height` keeps the aspect ratio.
Dimensions are rounded to even numbers.

`--crop` keeps one rectangle of the picture: `X,Y,WxH` from the top-left
corner, or `WxH` alone for the middle; each value is a pixel count or a
percentage of the source (`240,0,1440x1080`, `56%x100%`). The output
takes the crop's size unless a size is given, in which case the crop is
fitted to it like a whole picture would be. The region goes straight
from the decoder to the encoder, so a crop costs no more than a resize.

`--fit` also decides how the picture meets a canvas that `--for` builds:
a landscape video on a 9:16 target keeps its whole picture over a
blurred, scaled-up copy of itself, the way the phone editors fill a
Reel, and `--fit cover` crops it to its center instead. The blurred copy
is a first layer named `fill` with no audio, visible with
`--show-timeline`; it decodes the source a second time, so the render
costs about twice what bars would. `--fill bars` asks for the background
color instead, which is what this did before 0.7.

```sh
geneva convert talk.mov -o talk.mp4
geneva convert talk.mp4 -o talk.webm --codec vp9 --crf 32
geneva convert talk.mp4 -o talk-720.mp4 --height 720 --fps 30
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fit cover
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fill blur
geneva convert talk.mp4 -o square.mp4 --crop 1080x1080
```

### `geneva resize <input> -o FILE [--crop RECT] --width W | --height H [--fit contain|cover|fill]`

`convert` with a required size. `contain` (the default) shows the whole
picture with bars when the shape differs; `cover` fills the frame and
crops; `fill` stretches.

### `geneva trim <input> -o FILE [--from TIME] [--to TIME | --duration TIME] [--crop RECT] [--speed FACTOR]`

Keeps a range of the input. Without `--crf`, `--preset` or `--exact` the
streams are copied and the cut moves back to the previous keyframe; the
report gives the time actually used. `--crop` (as on `convert`) keeps one
rectangle of the picture and re-encodes, since a copied stream cannot
carry a crop. `--speed 2` plays the range twice as fast (`0.5` at half
speed), video and audio alike, the pitch following; also on `convert`.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s
geneva trim talk.mp4 -o clip.mp4 --from 1:02:10 --duration 45s --exact
```

### `geneva concat <input>... -o FILE [--crossfade TIME | --fade TIME [--fade-color COLOR]]`

Joins inputs back to back. Sources with identical stream parameters are
joined by copying packets; anything else is rendered, with inputs of a
different shape fitted inside the first one's frame. Either transition
overlaps each pair over the given time and always renders.

`--crossfade` dissolves: both clips are on screen at once, and the sound
crosses at constant power so the level holds across the overlap.

`--fade` dips through a color instead, black unless `--fade-color` says
otherwise. One clip is visible at a time, and the sound reaches silence
at the midpoint and comes back.

```sh
geneva concat part1.mp4 part2.mp4 part3.mp4 -o all.mp4
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s
geneva concat a.mp4 b.mp4 -o ab.mp4 --fade 0.6s --fade-color white
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

### `geneva audio <input> -o FILE --extract [--speech] | --mute | --replace FILE | --mix FILE [--gain DB]`

| Operation | Output |
| --- | --- |
| `--extract` | Only the audio, to `.m4a` (AAC), `.ogg` (Opus), `.flac` or `.wav`. Copied without re-encoding when the codec already matches the container. `--speech` writes it the way speech recognizers want it, 16 kHz mono, so `-o talk.wav --extract --speech` is the file to hand a transcriber. |
| `--mute` | The video without its audio track. |
| `--replace FILE` | The video with another file's audio, cut to the video's length. |
| `--mix FILE` | The original audio and another file's mixed together; `--gain` adjusts the mixed-in file in decibels. |

```sh
geneva audio talk.mp4 -o talk.m4a --extract
geneva audio talk.mp4 -o talk.wav --extract --speech
geneva audio talk.mp4 -o dubbed.mp4 --replace voice.wav
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
```

### `geneva subtitles <input> -o FILE --add FILE... [--language CODE...] | --burn FILE [--position P] [--margin PX] [--style JSON] [--highlight COLOR] [--safe PERCENT] | --extract [--track N]`

| Operation | Output |
| --- | --- |
| `--add FILE` (repeatable) | The input with each subtitle file (`.srt` or `.vtt`) attached as a text stream; `--language` values pair with the files in order. The picture and sound are copied when nothing else changes. |
| `--burn FILE` | The input with the cues of one caption file drawn into the picture (re-encoded). The file is a `.srt`, a `.vtt`, or the `.json` a speech recogniser writes, whose word times allow `--highlight COLOR` to pick out the word being said; a file that times whole cues gets a `W453` instead of a silent difference. Words are grouped into cues at most two lines long. Each cue becomes a text clip; cues that overlap in time go on further layers. `--position` is `bottom` (default), `top` or `center`; `--margin` is the distance from the edge in pixels (by default the `--safe` inset, at least 5% of the height). `--style` is a JSON object of [text source](timeline.md#sources) fields merged over the default look (white, semi-bold, black outline, soft shadow, sized to the frame, wrapped at 90% of the width), for example `'{"font": "600 40px Inter", "shadow": "0 2px 6px #000a", "background": "#00000080", "padding": "8px"}'` (CSS shorthands or the object forms; see the [timeline reference](timeline.md#css-shorthands)). Tags such as `<i>` are removed. |
| `--extract` | The input's subtitle stream number `--track` (0 by default) written as `.srt` or `.vtt`, by the output's extension. Only text subtitles can be extracted. |

Before rendering, every burned-in cue is laid out with the same engine
that draws it and checked against the frame: a cue whose box leaves the
picture is a warning (`W403`), one that lies outside the title-safe area
(`--safe` percent in from each edge, 5 by default, 0 to turn it off) is a
note (`N404`). Both name the cue and the clip in the timeline, so a style
can be fixed before the render, or an agent can react to the JSON report.
Long cues fold onto more lines, and a word longer than a line breaks
inside it; the size never changes on its own. With `--fit`, a cue that
does not fit the title-safe area (or the frame, when `--safe 0`) is
shrunk in steps until it does, down to half its size, and each one
shrunk is reported (`N405`).

```sh
geneva subtitles talk.mp4 -o talk-subbed.mkv --add en.srt --add fr.srt --language en --language fr
geneva subtitles talk.mp4 -o talk-burned.mp4 --burn en.srt
geneva subtitles talk.mp4 -o talk-burned.mp4 --burn en.srt --position top --style '{"size": 32, "color": "#ffdd00"}'
geneva subtitles talk.mp4 -o talk-burned.mp4 --burn transcript.json --highlight "#ffd233"
geneva subtitles talk-subbed.mkv -o fr.vtt --extract --track 1
```

## Targets

`--for TARGET` answers "what can this file be" from where it is going,
the way a hosting service decides a rendition, but for one file and with
every choice printed. The table is data (`geneva targets` prints it, with
the source and date of each platform's numbers; `--format json` gives it
to programs), and the choices go into the explicit encode block, so
`--show-timeline` shows exactly what was picked and a hand-written
timeline can say the same without the flag. What a particular viewer
gets, a rendition chosen at play time from a ladder, is not a file's
property and stays with the hosted product.

| Target | Ceiling | Loudness | About |
| --- | --- | --- | --- |
| `phone` | 1920×1080 | | Playback on a phone, fullscreen (covers current screens at 3× pixel density). |
| `tablet` | 2560×1600 | | Playback on a tablet. |
| `desktop` | 2560×1440 | | Playback on a laptop or desktop screen. |
| `tv` | 3840×2160 | | Playback on a television. |
| `web` | 1920×1080 | | A page or player on the open web. |
| `youtube` | 3840×2160 | -14 LUFS | Upload to YouTube: high quality, keyframes every half second, AAC 384 kb/s, as its upload guide asks; the loudness it normalises playback to. |
| `instagram` | 1440×2560, 9:16 | -14 LUFS | Instagram Reels. The loudness is not published; -14 is what uploads are measured to be normalised to. |
| `tiktok` | 1080×1920, 9:16 | -14 LUFS | TikTok. As for Instagram. |
| `podcast` | 1920×1080 | -16 LUFS | Speech for a podcast feed, or a video of one: Apple Podcasts asks for -16 LUFS and -1 dBTP; Spotify normalises to -14 and accepts -16. The only target that also turns on `output.audio.hygiene`. |
| `x` | 1920×1200 | | A post on X: 2:20 and 512 MB limits. |
| `linkedin` | 4096×2304 | | A native LinkedIn post: 10 minutes and 5 GB limits. |
| `email` | 1280×720 | | An attachment: a 25 MB budget. |

The rules, in order:

1. Never upscale. The ceiling only ever shrinks a picture, to even
   dimensions.
2. Keep the aspect, unless the target is 9:16: a landscape source is then
   fitted onto a portrait canvas of its own width, so that nothing is
   lost, over a blurred, scaled-up copy of itself, the way phone editors
   fill a Reel. `--fill bars` puts the background color (black by
   default) above and below instead. `--fit cover` on `convert` and
   `resize` (or on the clip, in a timeline) crops to the center instead,
   keeping the source's full height; a blind crop keeps the middle third
   of a 16:9 picture, so use it for content that sits in the middle.
3. H.264 High everywhere, with the level the size and frame rate need
   (`3.1` for 720p, `4.0` or `4.2` for 1080p, `5.1` or `5.2` for 4K), so
   old decoders know what to expect. `--codec` overrides.
4. Quality by tier: CRF 23 at `good`, 20 at `best`, 26 at `eco` for most
   targets (two lower at 4K; YouTube's tiers are 18/20/23 because it
   re-encodes; email's are 23/26/28). `--crf` overrides.
5. A bitrate cap where the target has one, raised by half above 30 fps,
   and lowered to fit a size limit or `--budget` from the length: quality
   stays constant until the cap bites. That holds for x264; VideoToolbox
   has no capped-quality mode and, given a data rate limit in quality
   mode, writes files twice the size, so on it the cap is not applied
   and the report says so, while `--budget` switches it to bitrate mode
   at the budget's rate, which does hold the size.
6. Keyframes every 2 s and fast start, so the file plays over a network.
   Frame rate capped at 60.
7. Limits an encode cannot meet are warnings, never silent changes: a
   video longer than the platform allows (`W411`), a file still larger
   than the limit after encoding (`W412`), an output that is not an MP4
   (`W413`), a budget too small for the length (`W414`).
8. Where the destination normalises loudness, the mix is brought to its
   level first (`output.audio.loudness`, see [timeline.md](timeline.md#output)):
   -14 LUFS with true peaks under -1 dBTP, so the platform has nothing
   to correct; `podcast` asks for -16 and also turns on
   `output.audio.hygiene`, the high-pass and hum notches for speech. A
   source used as it is counts as fitting only when its audio already
   measures within one loudness unit of that with its peaks under the
   ceiling, and never for `podcast`, whose hygiene a copy cannot
   carry; otherwise it is re-encoded. The targets without a number
   leave the level alone.

The report carries one note (`N410`) with every choice and its reason:

```
note[N410]: target phone: 3840×2160 scaled to 1920×1080 (ceiling 1920×1080);
  CRF 23 (good); H.264 High level 4.0; capped at 10000 kb/s;
  keyframes every 2 s; fast start; AAC 128 kb/s 48 kHz stereo
```

A loudness target or hygiene add what the mix measured and
what was done to it to the render's notes:

```
note[N600]: hygiene: high-pass at 80 Hz; mains hum at 60 Hz (-40 dBFS) notched with 2 harmonics
note[N600]: loudness: measured -23.4 LUFS, +9.4 dB to reach -14 LUFS, true peak held under -1 dBTP
```

`validate --probe` measures each asset's audio before anything is
rendered and reports it (N310), with a warning for clipping (W311), a
DC offset (W312), mains hum (W313) and silence (W314); see
[errors.md](errors.md).

```sh
geneva convert master.mov -o phone.mp4 --for phone
geneva convert master.mov -o reel.mp4 --for instagram --quality best
geneva trim master.mov -o clip.mp4 --from 1m --duration 2m --for x
geneva convert talk.mp4 -o talk-email.mp4 --for email --budget 20MB
geneva targets --format json
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
| H.264 | 8-bit 4:2:0 | A hardware encoder when present and `encode.video.hardware` allows it; otherwise the system's x264 when its library is installed (see the README), otherwise the bundled OpenH264. The notes of a render say which. |
| H.265 | 8-bit 4:2:0 | Hardware encoders only. |
| VP9, AV1 | 8-bit 4:2:0 | Software. |
| ProRes | 10-bit 4:2:2; 4:4:4 for `4444` and `4444-xq` | Profiles `proxy`, `lt`, `standard`, `hq` (default), `4444`, `4444-xq`. |
| DNxHR | 8-bit 4:2:2; 10-bit for `dnxhr-hqx`; 10-bit 4:4:4 for `dnxhr-444` | Profiles `dnxhr-lb`, `dnxhr-sq`, `dnxhr-hq` (default), `dnxhr-hqx`, `dnxhr-444`. Needs at least 256×120. |
| PNG | 8-bit RGBA | Lossless; keeps transparency. |
| Motion JPEG | 8-bit 4:2:0, full range | `--crf` maps onto JPEG quality (0 best). |

Reading is wider than writing: sources may also be VP8, MPEG-2, MPEG-4
part 2, DNxHD, raw video or GIF, with E-AC-3 audio, in MPEG-TS or AVI as
well as the containers above.
