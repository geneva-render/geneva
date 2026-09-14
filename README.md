<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/wordmark-dark.png">
  <img src="docs/wordmark.png" alt="Geneva" width="240">
</picture>

geneva turns a JSON file into a video.

It also handles the quick jobs from the command line. Trim a clip. Join two
files. Resize, burn in subtitles, pull the audio out. Each of those commands
writes the same JSON under the hood, and `--show-timeline` prints it, so you
can grab it and keep editing.

Under the commands there's a real compositor: layers, keyframes, text that
shapes properly, masks, blend modes. Colour is handled in linear light.

One binary. It uses FFmpeg's libraries to read and write files, so it opens
what ffmpeg opens.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s        # copies the streams, no re-encode
geneva convert talk.mp4 -o web.mp4 --for web      # one flag instead of fifteen
geneva render job.json -o out/                    # everything the document asks for
```

**Status: 0.4.** The format, the renderer, the commands and the encoders all
work. There's no GPU rendering yet. The format still changes between
versions, and will until 1.0. See [CHANGELOG.md](CHANGELOG.md).

MIT licensed. Linux and macOS. No services, no network access.

## What it does

Every example below is a file in [`examples/`](examples/). They all run as
they are: `examples/talk.mp4` is a four-second clip that geneva rendered
from shapes, so you don't have to supply your own footage. Each picture is
what the command next to it produced.

### Captions

<img src="docs/captions.png" alt="Captions with the current word highlighted, above Arabic, Hebrew and Thai text" width="520">

```sh
geneva frame examples/captions.json --at 0.8s -o captions.png
```

Give a text clip a list of words with start and end times. geneva draws the
line and highlights whichever word is current.

The text is shaped properly, so Arabic and Hebrew read right to left and
Thai breaks in the right places. You can add an outline, a shadow and a
rounded box. If you know CSS, write the styles the CSS way.

```json
{ "kind": "text",
  "words": [ { "text": "Captions",  "start": 0,      "end": "0.6s" },
             { "text": "highlight", "start": "0.6s", "end": "1.2s" } ],
  "font": "700 64px Liberation Sans",
  "highlight": { "color": "#ffb347" },
  "outline": "3px black",
  "background": "#00000099", "padding": 20, "radius": 14 }
```

### Vertical video

<img src="docs/social-reframe.png" alt="A landscape clip on a 9:16 canvas over a blurred copy of itself, with captions" width="260">

```sh
geneva frame examples/social-reframe.json --at 1s -o reframe.png
```

Three layers. The clip fills the tall frame and gets blurred, the same clip
sits whole on top of it, and the captions go over both.

If you just want the effect and not the document, there's a flag:

```sh
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fill blur
```

### Lower thirds you can reuse

<img src="docs/lower-third.png" alt="A lower third with a name and a title on a dark plate with an accent bar" width="420">

```sh
geneva frame examples/lower-third.json --at 1.2s -o lower-third.png
```

The plate, the accent bar, the dot that springs in, the name and the title
are one named composition. The timeline drops it in twice at different
positions and sizes. You design it once.

### Keyframes

<img src="docs/shapes.png" alt="Shapes with keyframed position, scale, rotation and opacity" width="360">

```sh
geneva frame examples/shapes.json --at 1s -o shapes.png
```

Put keyframes on position, scale, rotation, opacity, audio gain or a blur
radius. Ease them by name, with a cubic Bézier, or with a spring.

### One pass, many files

Most video jobs need the same pile of files: a few sizes, a thumbnail, a
sprite sheet for the player's scrub preview, and the audio at 16 kHz for
whisper. Ask for them together and geneva reads the source once.

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

```sh
geneva render examples/renditions.json -o out/
```

geneva picks the thumbnail for you. It skips the opening, then takes the
first frame that isn't dark and has some movement behind it, so you don't
get a black frame or a blurry one. The sprite sheet comes with the WebVTT
file players expect.

### Cuts that don't re-encode

If a trim or a join leaves the picture alone, geneva copies the packets
instead of re-encoding. That takes about as long as reading the file. You
don't have to know when it's safe, and you don't have to remember `-c copy`.

```
$ geneva trim talk.mp4 -o cut.mp4 --from 0.5s --to 1.5s
note[N600]: the video stream is used as is, so it is copied without re-encoding
note[N600]: cut at 0.5s moved to the keyframe at 0.48s
wrote cut.mp4 (1s, streams copied without re-encoding, 0.0s elapsed)
```

When it can't copy, it tells you why rather than quietly re-encoding:

```
note[N600]: not copied without re-encoding: the audio of talk.mp4 is aac, which wav cannot hold
```

Need the cut on an exact frame? `--exact` re-encodes only the frames between
your cut and the next keyframe, and copies the rest into the same track.

```
note[N600]: smart cut: 285 of 300 frames copied from the source, 15 encoded in 1 run around the cuts and overlays; audio copied as coded, each cut within half a packet
```

### Files that lie about their timing

Real files are full of traps. Variable frame rates. Edit lists. Audio that
starts after the video. 29.97 fps in a 600-tick timebase. Phone clips whose
average frame rate matches no frame in the file.

geneva is tested against fourteen files built to break it, and the tests
check that the output's timing matches the input's. A phone clip you trim
stays a copy, and the audio doesn't drift.

### Colour

Standard-definition footage stays BT.601 and HD stays BT.709. The tags end
up in the file you write. If the source has no tags, geneva guesses from the
size and tells you what it guessed.

```
$ geneva probe clip.mp4
  video: h264 720×576 @ 25 fps, yuv420p
    color: primaries untagged, transfer untagged, matrix untagged, range untagged
    assumed: matrix bt601, primaries bt601-625, transfer bt709, range limited (untagged material below HD resolution)
```

An HDR clip off a phone comes back to SDR through ITU-R BT.2446 method A,
which is the conversion the spec asks for. You don't get the blown-out
highlights a naive curve gives you. Use `--keep-hdr` to keep it HDR.

### Running it from a script

Add `--format json` and stdout becomes one JSON document. Progress goes to
stderr, one object per line, so you can follow a long render and still read
the result at the end.

geneva checks a timeline before it decodes anything. Every problem comes
with a code, a pointer to the spot in your JSON, the bad value and a fix.
Misspell a field and you get an error, not silence.

```
$ geneva validate job.json
error[E301]: clip 0 of layer 0 lasts 6s but its source range is only 2s
  --> /layers/0/clips/0/duration = "6s"
   = help: shorten the duration or widen the in/out range
```

Renders are deterministic. Same document, same assets, same version, same
frames. You can cache by input hash. [docs/agents.md](docs/agents.md) covers
the rest for scripts and agents.

## Commands

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s              # copied, no re-encode
geneva trim talk.mp4 -o clip.mp4 --from 12s --exact     # frame-accurate, smart cut
geneva concat part1.mp4 part2.mp4 -o all.mp4            # copied when the streams match
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s    # rendered
geneva resize talk.mp4 -o talk-720.mp4 --height 720
geneva convert talk.mp4 -o web.mp4 --for web            # copied if a browser can already play it
geneva convert talk.mp4 -o talk.mov --codec prores --profile hq
geneva convert talk.mp4 -o frames/%04d.png              # image sequence
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5
geneva audio talk.mp4 -o talk.wav --extract --speech    # 16 kHz mono, for whisper
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
geneva subtitles talk.mp4 -o burned.mp4 --burn en.srt --fit
geneva frame talk.mp4 -o thumb.jpg                      # first clear frame past the opening
geneva probe talk.mp4                                   # streams, colour tags, what was guessed
```

`--for` sets everything a destination needs at once: the size ceiling, the
codec, the level, the quality, the bitrate cap, the keyframe interval, fast
start and the audio settings. Run `geneva targets` to see the table, with a
source and a date for every platform's numbers.

Add `--show-timeline` to any command and it prints the JSON instead of
rendering. That's the quickest way to a document that already works. Run the
command, keep the JSON, edit it, render it.

## Installing

```sh
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

That grabs the right build for your machine and puts it in `/usr/local/bin`,
or `~/.local/bin` if the first isn't writable. Set `GENEVA_PREFIX` to
override. You can also download an archive from the
[releases page](https://github.com/geneva-render/geneva/releases) and run the
`install.sh` inside it. On macOS the installer clears the quarantine flag, so
you won't get a Gatekeeper dialog.

The archive also carries `check.sh`. Point it at one of your own files and
it runs the everyday commands on it and times each one, next to ffmpeg
doing the same job if you have ffmpeg installed.

Linux builds need glibc 2.35 or newer, so Ubuntu 22.04, Debian 12 or RHEL 9
and up. macOS builds need macOS 12 or newer on Apple silicon. Codecs,
containers and font shaping are all built in.

One thing to know about H.264. geneva bundles OpenH264, which makes bigger
files than x264 at the same quality. It doesn't bundle x264 itself, because
x264 is GPL, but it will use the copy on your system if you have one, and it
says which encoder it used. Hardware encoders come first when they're
available.

```sh
sudo apt install libx264-164     # Debian 12, Ubuntu 24.04 (libx264-163 on 22.04)
brew install x264                # macOS
```

## Documentation

| Page | What's in it |
| --- | --- |
| [docs/timeline.md](docs/timeline.md) | The format: every field, its default and its rules |
| [docs/cli.md](docs/cli.md) | Every command and flag, the `--for` targets, containers and codecs |
| [docs/agents.md](docs/agents.md) | The short version, for scripts and AI agents |
| [docs/errors.md](docs/errors.md) | Every error code and what to do about it |
| [docs/color.md](docs/color.md) | Tags, guessing, the working space, HDR |
| [docs/architecture.md](docs/architecture.md) | How the renderer, the copy planner and the encoders fit together |

## How this was built

Claude wrote all of it, running in Claude Code, directed and reviewed by one
person. That covers the Rust, the tests, these docs and the design notes
behind them. The commit trailers say which model wrote each commit.

It seems better to say that up front than to let you work it out. If you're
deciding whether to trust the code, you should know where it came from. And
if you're curious what this way of working actually produces, the repository
is the answer, bugs and fixes included.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, 10 to 20 minutes
cargo build --release
```

You'll need a C and C++ toolchain, cmake, meson, ninja, nasm, pkg-config and
clang. [CONTRIBUTING.md](CONTRIBUTING.md) lists the packages per platform and
explains how to run the tests. Third-party components and their licences are
in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## What's where

| Path | What it holds |
| --- | --- |
| `crates/geneva-timeline` | The format: types, parsing, validation, resolution, JSON Schema |
| `crates/geneva-render` | The compositor |
| `crates/geneva-color` | Colour tags, guessing, transfer functions, matrices, linear light |
| `crates/geneva-anim` | Keyframes and easing |
| `crates/geneva-media` | Probing, decoding, encoding, the copy planner, smart cut, mixing |
| `crates/geneva-golden` | Image comparison and the golden-frame tests |
| `crates/geneva-cli` | The `geneva` command |
| `schema/` | Published JSON Schema, one file per format version |
| `tests/golden/` | Golden scenes, their reference frames and fonts |
| `tests/media/` | Small test files, including the fourteen timing traps |

## Licence

MIT. See [LICENSE](LICENSE). The bundled third-party components and their
licences are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
