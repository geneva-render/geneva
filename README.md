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

## A worked example

Here's a lower third over ten seconds of footage from the space station.
You write a document, you run one command, and geneva tells you what it
did.

```json
"layers": [
  { "id": "footage", "clips": [ { "source": { "kind": "video", "asset": "iss" } } ] },

  { "id": "lower-third", "clips": [ {
      "source": { "kind": "composition", "composition": "card" },
      "start": "2s", "duration": "4s",
      "transform": {
        "anchor": { "x": "0%", "y": "100%" },
        "position": { "keyframes": [
          { "t": 0,      "v": { "x": -600, "y": 648 }, "ease": "ease-out" },
          { "t": "0.5s", "v": { "x": 56,   "y": 648 } } ] } },
      "opacity": { "keyframes": [ { "t": 0, "v": 0 }, { "t": "0.3s", "v": 1 },
                                  { "t": "3.7s", "v": 1 }, { "t": "4s", "v": 0 } ] } } ] }
]
```

```sh
geneva render examples/lower-third.json -o dragon.mp4
```

<img src="docs/demo.gif" alt="A lower third sliding in over footage of a Dragon capsule at the space station" width="480">

```
note[N600]: smart cut: 165 of 300 frames copied from the source, 135 encoded in 1 run around the cuts and overlays
note[N600]: H.264 runs encoded with the system's x264 (build 164) at CRF 18
wrote dragon.mp4 (300 frames, 10s of video, 3.5s elapsed)
```

Three things are going on there.

The card is its own little composition: a plate, a green bar, a name and a
subtitle, sized 560 by 96. You build it once and place it in the timeline
like any other clip, so a second one costs you four lines.

It slides in because the `position` has two keyframes half a second apart,
one off the left edge and one in place, with `ease-out` between them. Same
idea for the fade: four keyframes on `opacity`. Any number you can set, you
can animate.

And the last line is the part worth looking at twice. The card is only on
screen for four of the ten seconds. geneva worked out that the rest of the
video is untouched, so it copied 165 frames straight through as they were
and only re-encoded the 135 around the card. Ten seconds took three. On a
ninety-minute film with a watermark on the title card, that's the
difference between a coffee and an afternoon.

### Captions

<img src="docs/captions.png" alt="A caption over the footage with the current word highlighted, and an Arabic line below it" width="560">

```sh
geneva render examples/captions.json -o captioned.mp4
```

Give a text clip a list of words with start and end times. geneva draws the
line and picks out whichever word is current, which is the style every
short-form platform uses now.

The text is properly shaped, so the Arabic line underneath reads right to
left, and Thai would break in the right places. You get wrapping, an
outline, a shadow and a rounded box. If you know CSS you can write the
styles the CSS way: `"700 44px Liberation Sans"` for the font,
`"3px black"` for the outline.

### Vertical video

<img src="docs/social-reframe.png" alt="The landscape clip on a tall canvas over a blurred copy of itself, with captions" width="240">

```sh
geneva render examples/social-reframe.json -o reel.mp4
```

Three layers make a 16:9 clip fit a 9:16 frame without cropping anything
out. The clip fills the tall frame and gets blurred, the same clip sits
whole on top, and the captions go over both.

If you only want the effect and not the document, there's a flag for it:

```sh
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fill blur
```

### Everything a page needs, in one go

A video on a web page usually needs the same pile of files: two or three
sizes, a thumbnail, a sprite sheet for the scrub preview, and the audio on
its own at 16 kHz to feed whisper. List them in one document and geneva
reads the source once instead of six times.

```json
"outputs": {
  "720p":    { "kind": "video" },
  "480p":    { "kind": "video", "height": 480, "encode": { "video": { "crf": 23 } } },
  "360p":    { "kind": "video", "height": 360, "encode": { "video": { "crf": 25 } } },
  "poster":  { "kind": "poster", "width": 960 },
  "preview": { "kind": "sprites", "every": "2s", "columns": 5 },
  "speech":  { "kind": "audio", "path": "speech.wav", "audio": { "sample_rate": 16000, "channels": 1 } }
}
```

```sh
geneva render examples/renditions.json -o out/
```

geneva picks the thumbnail rather than grabbing frame one. It skips the
opening, then takes the first frame that isn't dark and has some movement
behind it, so you don't end up with a black frame or a blurry one. The
sprite sheet comes with the WebVTT file that players expect.

### Checking a design without rendering it

`geneva frame` writes a single frame as a PNG or JPEG. It's for when you're
fiddling with the position of a title and don't want to sit through a
render to see it.

```sh
geneva frame examples/lower-third.json --at 3.5s -o check.png
```

### Cuts that don't re-encode

The same trick from the worked example applies to plain cuts. A trim or a
join that leaves the picture alone copies the packets instead of
re-encoding, which takes about as long as reading the file. You don't have
to know when that's safe, and you don't have to remember `-c copy`.

```
$ geneva trim talk.mp4 -o cut.mp4 --from 0.5s --to 1.5s
note[N600]: the video stream is used as is, so it is copied without re-encoding
note[N600]: cut at 0.5s moved to the keyframe at 0.48s
wrote cut.mp4 (1s, streams copied without re-encoding, 0.0s elapsed)
```

When it can't copy, it says why rather than quietly re-encoding:

```
note[N600]: not copied without re-encoding: the audio of talk.mp4 is aac, which wav cannot hold
```

### Files that lie about their timing

Real files are full of traps. Variable frame rates. Edit lists. Audio that
starts after the video. 29.97 fps in a 600-tick timebase. Phone clips whose
average frame rate matches no frame in the file.

geneva is tested against fourteen files built to break exactly that, and
the tests check the output's timing against the input's. A phone clip you
trim stays a copy, and the audio doesn't drift.

### Colour

Standard-definition footage stays BT.601 and HD stays BT.709, and the tags
end up in the file you write. If the source has no tags, geneva guesses
from the size and tells you what it guessed.

```
$ geneva probe clip.mp4
  video: h264 720×576 @ 25 fps, yuv420p
    color: primaries untagged, transfer untagged, matrix untagged, range untagged
    assumed: matrix bt601, primaries bt601-625, transfer bt709, range limited (untagged material below HD resolution)
```

An HDR clip off a phone comes back to SDR through ITU-R BT.2446 method A,
which is the conversion the spec asks for, so you don't get the blown-out
highlights a naive curve gives you. `--keep-hdr` keeps it HDR instead.

### Running it from a script

Add `--format json` and stdout becomes one JSON document. Progress goes to
stderr, one object per line, so you can follow a long render and still read
the result at the end.

geneva checks a document before it decodes anything. Every problem comes
with a code, a pointer to the spot in your JSON, the bad value and a fix.
Misspell a field and you get an error, not silence.

```
$ geneva validate job.json
error[E301]: clip 0 of layer 0 lasts 6s but its source range is only 2s
  --> /layers/0/clips/0/duration = "6s"
   = help: shorten the duration or widen the in/out range
```

Renders are deterministic. Same document, same assets, same version, same
frames, so you can cache by input hash. [docs/agents.md](docs/agents.md)
has the rest for scripts and agents.

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
