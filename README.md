<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/wordmark-dark.png">
  <img src="docs/wordmark.png" alt="Geneva" width="240">
</picture>

Geneva is a video tool with two halves that share one engine. On the
outside, everyday commands: trim, concat, convert, resize, overlay, audio,
subtitles, frame, probe. Underneath, a versioned JSON document with a
compositor behind it: layers and nested compositions, keyframed animation,
text with real shaping, masks, blend modes, colour-managed compositing in
linear light, audio mixing. Every command compiles to one of those
documents and will print it, so a one-line job can grow into a composition
without changing tools.

It uses FFmpeg's libraries to demux, decode, encode and mux. What it adds
is a set of things built in that usually cost you a filter chain, a report
that says what it did and why, and defaults that are right on the details
that go quietly wrong.

One binary, no services, no network access, MIT licensed. Linux and macOS.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s        # copies the streams, no re-encode
geneva convert talk.mp4 -o web.mp4 --for web      # a destination, not fifteen flags
geneva render job.json -o out/                    # every output in the document, one pass
```

**Status: 0.4.** The format, the renderer, the verbs and the encoders work
end to end. GPU rendering is not there. The format is versioned and will
keep changing until 1.0; see [CHANGELOG.md](CHANGELOG.md).

## Built in, not assembled

**Text that a designer would recognise.** Shaped, not stamped: Arabic and
Hebrew run right to left, Thai breaks where Thai breaks. Wrapping,
alignment, outline, drop shadow, a rounded background box, and word-level
timing with a style for whichever word is current. Where CSS has a
spelling, the document takes it: `font` as `"700 72px/1.25 Inter"`,
`outline` as `"2px black"`, `shadow` as the `text-shadow` shorthand
`"0 2px 8px #0008"`. A font file can travel in the document as an asset, so
a render need not depend on what is installed.

**Animation as a function of time.** Keyframes on position, scale,
rotation, opacity, gain and effect parameters, with easing by name, cubic
Bézier or a spring. Nothing is a filter expression evaluated per frame; the
value at time *t* is defined by the document.

**Compositing that respects colour.** Layers blend in linear light with the
usual modes. Standard-definition sources stay BT.601 and high-definition
stays BT.709, tags travel into the output, and untagged material is
inferred from its size with the assumption printed. HDR becomes SDR by
ITU-R BT.2446 method A, the conversion the recommendation specifies, rather
than a curve that sends every highlight to white.

**A copy planner.** Trims, joins and format changes that leave the picture
alone copy the coded packets instead of re-encoding. You do not have to
know when that is safe: the planner decides, and when it refuses it names
the fact that stopped it rather than falling back in silence.

**Smart cut.** When a cut has to land on an exact frame, only the frames
between the cut and the next keyframe are re-encoded; the rest are copied
into the same track, stitched at packet level.

**Many outputs from one document.** Renditions at several sizes, a poster
still, a sprite sheet with the WebVTT map players read, the audio alone at
16 kHz for a speech model: declared together, produced in one pass.

**Timing that survives real files.** Variable frame rates, edit lists,
negative composition offsets, audio that starts late, 29.97 in a 600-tick
timebase, phone clips whose average frame rate matches no actual frame. A
corpus of fourteen such files asserts that output timing matches input
timing.

**A machine-readable surface.** `--format json` puts one document on stdout
and a stream of progress objects on stderr. Timelines are validated before
anything is decoded, with a code, a JSON pointer, the offending value and a
fix for every problem. Unknown fields are errors, so a typo is caught
rather than ignored. Renders are deterministic, so output can be cached by
input hash.

## Examples

Every file in [`examples/`](examples/) is a complete timeline, and
`examples/talk.mp4` is a four-second clip geneva made from shapes, so the
ones that need video run as they are. Each picture below is the output of
the command beside it, scaled down for this page.

### Captions with word timing

<img src="docs/captions.png" alt="Captions with the current word highlighted, above Arabic, Hebrew and Thai text" width="520">

```sh
geneva frame examples/captions.json --at 0.8s -o captions.png
```

One text source carries the words and their times, a base style and a
highlight style. Four scripts in one frame, an outline and a background box
on the caption line.

```json
{ "kind": "text",
  "words": [ { "text": "Captions",  "start": 0,      "end": "0.6s" },
             { "text": "highlight", "start": "0.6s", "end": "1.2s" } ],
  "font": "700 64px Liberation Sans",
  "highlight": { "color": "#ffb347" },
  "outline": "3px black",
  "background": "#00000099", "padding": 20, "radius": 14 }
```

### Reframing to vertical

<img src="docs/social-reframe.png" alt="A landscape clip on a 9:16 canvas over a blurred copy of itself, with captions" width="260">

```sh
geneva frame examples/social-reframe.json --at 1s -o reframe.png
```

Three layers: the picture cover-fitted and blurred to fill the frame, the
picture whole on top, captions over both. The same effect without the
document is `geneva convert talk.mp4 -o reel.mp4 --for tiktok --fill blur`.

### A lower third, built once and placed twice

<img src="docs/lower-third.png" alt="A lower third with a name and a title on a dark plate with an accent bar" width="420">

```sh
geneva frame examples/lower-third.json --at 1.2s -o lower-third.png
```

The plate, accent bar, springing dot, name and title are a named
composition. The timeline places it twice with different transforms, so the
second costs one clip rather than a copy of the design.

### Keyframes and easing

<img src="docs/shapes.png" alt="Shapes with keyframed position, scale, rotation and opacity" width="360">

```sh
geneva frame examples/shapes.json --at 1s -o shapes.png
```

Position, scale, rotation and opacity on keyframes, with eased and spring
interpolation.

### Renditions, poster, sprites and speech audio

```sh
geneva render examples/renditions.json -o out/
```

```json
"outputs": {
  "1080p":   { "kind": "video" },
  "720p":    { "kind": "video", "height": 720, "encode": { "video": { "crf": 23 } } },
  "480p":    { "kind": "video", "height": 480, "encode": { "video": { "crf": 25 } } },
  "poster":  { "kind": "poster", "width": 1280 },
  "preview": { "kind": "sprites", "every": "5s", "columns": 10 },
  "speech":  { "kind": "audio", "path": "speech.wav", "audio": { "sample_rate": 16000, "channels": 1 } }
}
```

The poster is chosen rather than grabbed: the first frame past the opening
that is not dark and follows some motion, so a fade-in or a phone clip's
opening blur is skipped. The report lists every file with its size and
media type.

## Speed

Measured on one machine, so read them as ratios rather than absolutes: a
four-core Intel Xeon at 2.1 GHz, a 60-second 1080p H.264 file, release
build, software encoding through the same system x264 (build 164) on both
sides, matched presets and quality. Each step ran twice and the second time
is reported.

| Job | Geneva | ffmpeg |
| --- | --- | --- |
| Trim 10s out | 0.1s | 5.8s for the obvious command, 0.1s with `-c copy` |
| Frame-accurate trim, cut mid-GOP | 0.6s | 4.8s |
| Resize to 720p | 13.0s | 13.0s |
| 3 renditions + poster + sprites + speech audio | 42.9s | 53.1s as six commands, 43.3s as one |

On cuts and joins geneva reaches the result an expert command would, without
being told, and is many times faster than the command most people write. A
frame-accurate trim is about eight times faster because only 15 of 300
frames are re-encoded and the rest are copied. A single transcode is a
wash: both scale in YUV and hand the frames to the same encoder.

The last row is the one worth explaining. Six commands decode the file six
times; geneva decodes once and feeds the encoders in parallel, which is
where the 20% comes from. The single ffmpeg command with `split` filters
also decodes once, and lands in the same place. What geneva gives you there
is one document instead of a filter graph, plus the sprite sheet's WebVTT
map and a report listing every file, rather than less CPU time.

The archives carry `check.sh`, which runs this kind of comparison on your
machine and your file and prints the table.

## What ffmpeg does better

Everything not listed above, and a good deal of it matters: hundreds of
filters, capture from devices, streaming in and out, HLS and DASH, loudness
normalisation, waveforms, hardware encoders on every platform, formats off
the beaten path, and twenty-five years of hardening against broken files.
Geneva reads and writes files and does the jobs above. For the rest, or
when you already know the flags, use ffmpeg.

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

`--for` carries a destination's whole policy: size ceiling, codec, level,
quality, bitrate cap, keyframe interval, fast start and audio, from one
table that `geneva targets` prints with the source and date of each
platform's numbers. Any verb prints the document it built with
`--show-timeline`, which is the fastest way to a correct skeleton: run the
verb, keep the JSON, edit it, render it.

A run says what it did:

```
$ geneva trim talk.mp4 -o cut.mp4 --from 0.5s --to 1.5s
note[N600]: the video stream is used as is, so it is copied without re-encoding
note[N600]: cut at 0.5s moved to the keyframe at 0.48s
wrote cut.mp4 (1s, streams copied without re-encoding, 0.0s elapsed)
```

```
$ geneva validate job.json
error[E301]: clip 0 of layer 0 lasts 6s but its source range is only 2s
  --> /layers/0/clips/0/duration = "6s"
   = help: shorten the duration or widen the in/out range
```

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
