<img src="docs/wordmark.svg" alt="Geneva" width="360">

Geneva renders video from a JSON document. Layers and nested compositions,
keyframed animation, text with real shaping and word-level timing, masks,
blend modes, colour-managed compositing in linear light, audio mixing. It
also does the everyday edits as one-line commands, and those compile to the
same documents, so a quick job can grow into a composition.

One binary, no services, no network access, MIT licensed. Linux and macOS.

<img src="docs/captions.png" alt="A frame of captions with the current word highlighted, over Arabic, Hebrew and Thai text" width="640">

```sh
geneva frame examples/captions.json --at 0.8s -o captions.png   # the frame above
```

**Status: 0.4.** The format, the renderer, the verbs and the encoders work
end to end. GPU rendering is not there. The format is versioned and will
keep changing until 1.0; see [CHANGELOG.md](CHANGELOG.md).

## What it is for

### Captions that move with the words

Word-by-word captions are the house style of every short-form platform, and
they are miserable to produce with a filter chain, because each word needs
its own draw call and its own time window. Here a caption is one text
source with timed words and a style for whichever word is current.

```json
{ "kind": "text",
  "words": [ { "text": "Every", "start": 0,      "end": "0.4s" },
             { "text": "word",  "start": "0.4s", "end": "0.8s" },
             { "text": "lands", "start": "0.8s", "end": "1.3s" } ],
  "font": "700 72px/1.25 Liberation Sans",
  "color": "white",
  "highlight": { "color": "#ffd233" },
  "outline": "4px #000000cc" }
```

Text is shaped, not just drawn: Arabic and Hebrew run right to left, Thai
breaks where Thai breaks, and the picture above is all three in one frame.
Lines wrap at a width you set, and a caption can carry an outline, a drop
shadow and a rounded background box. A font file can travel in the document
as an asset, so a render need not depend on what the machine has installed.

### Reframing for vertical

A landscape talk becomes a 9:16 post: the picture whole in the middle, a
blurred cover-fitted copy of itself behind to fill the bars, captions over
the top. The whole thing is three layers in one document
([`examples/social-reframe.json`](examples/social-reframe.json)), or a flag
if you want the plain version:

```sh
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fill blur
```

`--for` carries the destination's whole policy: size ceiling, codec, level,
quality, bitrate cap, keyframe interval, fast start and audio, from one
table that `geneva targets` prints with the source and date of each
platform's numbers. The choices land in the document, so `--show-timeline`
shows exactly what was picked.

### Everything a video needs, from one pass

Most products that handle video need the same set of files: a rendition or
three, a poster, a sprite sheet for the player's seek preview, and the
audio alone at 16 kHz mono for a speech model. An `outputs` map asks for
them together, and the frames are composited once
([`examples/renditions.json`](examples/renditions.json)).

```json
"outputs": {
  "1080p":   { "kind": "video" },
  "720p":    { "kind": "video", "height": 720, "encode": { "video": { "crf": 23 } } },
  "poster":  { "kind": "poster", "width": 1280 },
  "preview": { "kind": "sprites", "every": "5s", "columns": 10 },
  "speech":  { "kind": "audio", "path": "speech.wav", "audio": { "sample_rate": 16000, "channels": 1 } }
}
```

```sh
geneva render job.json -o out/
```

The poster is chosen rather than grabbed: the first frame past the opening
that is not dark and follows some motion, so a fade-in or a phone clip's
opening blur is skipped. The sprite sheet comes with the WebVTT file
players read. The report lists every file with its size and media type.

### Cuts that do not re-encode

A trim or a join that leaves the picture alone copies the coded packets,
which takes about as long as reading the file. You do not have to know when
that is safe, because the planner decides and says so; when it refuses, it
names the fact that stopped it rather than silently re-encoding.

```
$ geneva trim talk.mp4 -o cut.mp4 --from 0.5s --to 1.5s
note[N600]: the video stream is used as is, so it is copied without re-encoding
note[N600]: cut at 0.5s moved to the keyframe at 0.48s
wrote cut.mp4 (1s, streams copied without re-encoding, 0.0s elapsed)
```

When the cut has to land on an exact frame, `--exact` re-encodes only the
frames between the cut and the next keyframe and copies the rest into the
same track, stitched at packet level:

```
note[N600]: smart cut: 26 of 35 frames copied from the source, 9 encoded in 1 run around the cuts and overlays; audio copied as coded, each cut within half a packet
note[N600]: H.264 runs encoded with the system's x264 (build 164) at CRF 18
```

### Files whose timing is a trap

Real files lie about their timing: variable frame rates, edit lists,
negative composition offsets, audio that starts after the video, 29.97 in a
600-tick timebase, phone recordings whose average frame rate matches no
actual frame. Geneva is held to a corpus of fourteen such files that assert
the output's timing matches the input's, so a copy of a phone clip stays a
copy and audio does not drift.

Colour is the same story. Standard-definition sources stay BT.601 and
high-definition stays BT.709, tags travel into the output, and untagged
material is inferred from its size with the assumption printed:

```
$ geneva probe clip.mp4
  video: h264 720×576 @ 25 fps, yuv420p
    color: primaries untagged, transfer untagged, matrix untagged, range untagged
    assumed: matrix bt601, primaries bt601-625, transfer bt709, range limited (untagged material below HD resolution)
```

An HDR phone clip becomes SDR by ITU-R BT.2446 method A, the conversion the
recommendation specifies, instead of a curve that sends every highlight to
white. `--keep-hdr` keeps it HDR with the tags and a ten-bit codec.

### Built to be driven by a program

A timeline is validated before anything is decoded, and every problem
carries a code, a JSON pointer, the offending value and a fix. Unknown
fields are errors, so a typo is caught rather than ignored.

```
$ geneva validate job.json
error[E301]: clip 0 of layer 0 lasts 6s but its source range is only 2s
  --> /layers/0/clips/0/duration = "6s"
   = help: shorten the duration or widen the in/out range
```

With `--format json`, stdout is exactly one document and stderr is a stream
of progress objects, so a pipeline can follow a long render and still read
the result. Renders are deterministic, so output can be cached by input
hash. [docs/agents.md](docs/agents.md) is the page for code and agents.

## How this relates to ffmpeg

Geneva uses FFmpeg's libraries to demux, decode, encode and mux, the way
Shotcut, Handbrake, OBS and DaVinci Resolve do. The compositor, the
document format, the colour pipeline, the validator, the copy planner and
the smart cut are geneva's own.

What it deliberately does not do: capture from devices, stream to or from a
network, package HLS or DASH, normalise loudness, or draw waveforms. If you
need those, or one of ffmpeg's hundreds of filters, or a format off the
beaten path, use ffmpeg. It is a fine tool and it is not going anywhere.

## Everyday commands

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s              # copied, no re-encode
geneva trim talk.mp4 -o clip.mp4 --from 12s --exact     # smart cut, frame-accurate
geneva concat part1.mp4 part2.mp4 -o all.mp4            # copied when the streams match
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s    # rendered
geneva resize talk.mp4 -o talk-720.mp4 --height 720
geneva convert talk.mp4 -o web.mp4 --for web            # copied when a browser already plays it
geneva convert talk.mp4 -o talk.mov --codec prores --profile hq
geneva convert talk.mp4 -o frames/%04d.png              # image sequence
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5
geneva audio talk.mp4 -o talk.wav --extract --speech    # 16 kHz mono, for transcription
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
geneva subtitles talk.mp4 -o burned.mp4 --burn en.srt --fit
geneva frame talk.mp4 -o thumb.jpg                      # the first clear frame past the opening
geneva probe talk.mp4                                   # streams, colour tags, what was assumed
```

Any verb prints the document it built with `--show-timeline`. That is the
fastest way to a correct skeleton: run the verb, keep the JSON, edit it,
render it.

## Examples

<img src="docs/lower-third.png" alt="A lower third with a name and a title on a dark plate" width="480">

Every file in [`examples/`](examples/) is a complete timeline that passes
`geneva validate`.

| File | Shows |
| --- | --- |
| `captions.json` | Word-by-word highlighted captions, outline and box, three scripts |
| `social-reframe.json` | 16:9 into 9:16 with a blurred backdrop and captions |
| `renditions.json` | Renditions, poster, sprite sheet and speech audio from one pass |
| `lower-third.json` | A reusable composition placed twice, spring easing, text (above) |
| `shapes.json` | Keyframed position, scale, rotation and opacity |
| `overlay.json` | Video clips with a crossfade, an image overlay, an audio bed |
| `solid.json` | The smallest useful document |

## Installing

```sh
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

That fetches the latest release for your machine and installs it to
`/usr/local/bin`, or `~/.local/bin` when the first is not writable
(`GENEVA_PREFIX` overrides). You can instead take an archive from the
[releases page](https://github.com/geneva-render/geneva/releases) and run
the `install.sh` inside it. On macOS the installer clears the quarantine
flag a browser download carries, so the binary starts without a Gatekeeper
detour.

Each archive also carries `check.sh`, which runs the everyday commands on a
generated clip or on a file you pass, and prints what each step took next
to the time ffmpeg takes for the same step:

```sh
sh check.sh input.mp4
```

Linux binaries need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, RHEL 9).
macOS binaries need macOS 12 or newer on Apple silicon. Codecs, containers
and font shaping are built in.

Software H.264 is the one thing worth knowing about. Geneva bundles
OpenH264, which writes larger files than x264 at the same quality. It does
not bundle x264, which is GPL licensed, but it loads the system's copy when
one is present and the report says which encoder ran. Hardware encoders
(VideoToolbox, NVENC) come first when they are there.

```sh
sudo apt install libx264-164     # Debian 12, Ubuntu 24.04 (libx264-163 on 22.04)
brew install x264                # macOS
```

## Documentation

| Page | Contents |
| --- | --- |
| [docs/timeline.md](docs/timeline.md) | The format: every field, with defaults and rules |
| [docs/cli.md](docs/cli.md) | Every command and option, targets, containers and codecs |
| [docs/agents.md](docs/agents.md) | The short version for programs and AI agents |
| [docs/errors.md](docs/errors.md) | Every diagnostic code and what to do about it |
| [docs/color.md](docs/color.md) | Tags, inference, the working space, HDR |
| [docs/architecture.md](docs/architecture.md) | How the renderer, the copy planner and the encoders fit |

## How this was built

Every line of this repository was written by Claude, Anthropic's model,
running in Claude Code, from the direction, review and testing of one
person: the Rust, the tests, the documentation and the design notes behind
them. The commit trailers record which model wrote each commit.

It is worth saying plainly rather than leaving to be discovered. If you are
deciding whether to trust the code, you should know where it came from and
read it accordingly. And if you are curious what the method produces, the
repository is the evidence, including the mistakes the history records and
the fixes that followed.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, 10 to 20 minutes
cargo build --release
```

The script needs a C and C++ toolchain, cmake, meson, ninja, nasm,
pkg-config and clang. [CONTRIBUTING.md](CONTRIBUTING.md) has the
per-platform package lists and the test workflow. Third-party components
and their licences are in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/geneva-timeline` | The format: types, parsing, validation, resolution, JSON Schema |
| `crates/geneva-render` | The compositor and the renderer interface |
| `crates/geneva-color` | Colour tags, inference, transfer functions, matrices, linear light |
| `crates/geneva-anim` | Keyframes and easing; animation as a pure function of time |
| `crates/geneva-media` | Probing, decoding, encoding, the copy planner, smart cut, mixing |
| `crates/geneva-golden` | Perceptual image comparison and the golden-frame driver |
| `crates/geneva-cli` | The `geneva` command-line tool |
| `schema/` | Published JSON Schema, one file per format version |
| `tests/golden/` | Golden scenes, their reference frames and fonts |
| `tests/media/` | Small media files, including the timing corpus |

## Licence

Geneva is open source under the [MIT Licence](LICENSE). The bundled
third-party components and their licences are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
