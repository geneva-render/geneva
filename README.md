<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/wordmark-dark.png">
  <img src="docs/wordmark.png" alt="Geneva" width="240">
</picture>

geneva edits and processes video programmatically — one-line commands for
the everyday jobs, a JSON document for anything more. Same engine either
way: the commands compile to the document, and `--show-timeline` prints
it, so you can grab it and keep editing.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s        # copies the streams, no re-encode
geneva subtitles talk.mp4 -o subbed.mp4 --burn transcript.json --highlight "#ffd233"
geneva render job.json -o out/                    # everything the document asks for
```

Underneath is a real compositor: layers, keyframes, text that shapes
properly, masks, blend modes. Overlays can be written as HTML and CSS —
flexbox, the box model, `@keyframes` — and geneva lays them out itself,
with no browser anywhere. Colour is handled in linear light.

One binary. It uses FFmpeg's libraries to read and write files, so it
opens what ffmpeg opens.

**Status: 0.4.** The format, the renderer, the commands and the encoders
all work. There's no GPU rendering yet, and the format still changes
between versions until 1.0. See [CHANGELOG.md](CHANGELOG.md).

MIT licensed. Linux and macOS. No services, no network access.

## A worked example

Here's a lower third over ten seconds of footage from the space station.
The card is an HTML file — open it in a browser and it looks and moves the
same:

```html
<!-- A lower third. Open this file in a browser: it looks and moves the same. -->
<style>
  @keyframes slide-in { from { translate: -100% } to   { translate: 0 } }
  @keyframes fade-in  { from { opacity: 0 }      to   { opacity: 1 } }
  @keyframes fade-out { from { opacity: 1 }      to   { opacity: 0 } }

  .card {
    animation: slide-in 0.5s ease-out, fade-in 0.3s, fade-out 0.3s 3.7s;

    position: absolute;
    left: 4.4%;
    bottom: 10%;
    width: 44%;

    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
    padding: 16px 28px;
    box-sizing: border-box;

    background: #0b1016d9;
    border-radius: 8px;
    border-left: 5px solid #4ade80;
    box-shadow: 0 6px 24px #00000066;
  }
  .card h1 { margin: 0; font: 700 32px Liberation Sans; color: #ffffff }
  .card p  { margin: 0; font: 400 20px Liberation Sans; color: #9fb0bf }
</style>

<div class="card">
  <h1>Dragon CRS-17</h1>
  <p>Berthing at the ISS &nbsp;&middot;&nbsp; NASA</p>
</div>
```

and the whole document that puts it on the footage:

```json
{
  "geneva": "0.3",
  "output": { "width": 1280, "height": 720, "fps": 30 },

  "assets": {
    "iss":  { "src": "iss.mp4" },
    "card": { "src": "card.html" }
  },

  "layers": [
    { "id": "footage", "clips": [ { "source": { "kind": "video", "asset": "iss" } } ] },

    { "id": "lower-third", "clips": [ {
        "source": { "kind": "html", "asset": "card" },
        "start": "2s", "duration": "4s" } ] }
  ]
}
```

```sh
geneva render examples/lower-third.json -o dragon.mp4
```

<img src="docs/demo.gif" alt="A lower third sliding in over footage of a Dragon capsule at the space station" width="640">

```
note[N600]: smart cut: 165 of 300 frames copied from the source, 135 encoded in 1 run around the cuts and overlays
note[N600]: H.264 runs encoded with the system's x264 (build 164) at CRF 18
wrote dragon.mp4 (300 frames, 10s of video, 3.5s elapsed)
```

The JSON says only *when*. Everything else is the card's business, and
everything in the card is real CSS: `display: flex` is flexbox, `padding`
and `border-left` are the box model, `position: absolute` with `left` and
`bottom` places it the way it would on a page. geneva cascades the styles
and lays them out with [taffy](https://github.com/DioxusLabs/taffy), so
the layout implements the spec rather than guessing at it.

There isn't a pixel coordinate anywhere: `left: 4.4%` and `width: 44%` are
shares of the frame, and `translate: -100%` is the card's own width, so
the slide starts exactly off its own edge. The motion is in the same file
— `@keyframes` and an `animation`, taking what CSS takes, plus
`spring(170, 26)`, which CSS hasn't got. It is not a browser, and anything
it can't draw is a warning that names it rather than a silent difference.

The last line of the report is the part worth looking at twice. The card
is on screen for four of the ten seconds, so geneva copied the other 165
frames straight through and re-encoded only the 135 around it. On a
ninety-minute film with a watermark on the title card, that's the
difference between a coffee and an afternoon.

## Installing

```sh
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

That grabs the right build for your machine and puts it in `/usr/local/bin`,
or `~/.local/bin` if the first isn't writable (`GENEVA_PREFIX` overrides).
The [releases page](https://github.com/geneva-render/geneva/releases) has
the archives, each with the same `install.sh` inside, plus a `check.sh`
that times the everyday commands on a file of yours against ffmpeg doing
the same job. Linux needs glibc 2.35 or newer; macOS needs 12 or newer on
Apple silicon. Codecs, containers and font shaping are built in.

The one thing worth knowing is H.264. geneva bundles OpenH264, which makes
bigger files than x264 at the same quality, and doesn't bundle x264
itself because x264 is GPL — but it will use the copy on your system, and
it always says which encoder it used.

```sh
sudo apt install libx264-164     # Debian 12, Ubuntu 24.04 (libx264-163 on 22.04)
brew install x264                # macOS
```

## What else it does

The same document format covers captions from a transcript, vertical
reframing, and every file a web page needs from one pass over the source.
Each of these is a runnable example with its document, its command and its
output:

| | |
| --- | --- |
| **Captions from a transcript** — whisper's JSON in, each word picked out as it is said | [examples](examples/README.md#captions) |
| **Captions as a layer** — one part of a larger document, placed and styled | [examples](examples/README.md#captions-as-a-layer) |
| **Vertical video** — 16:9 into 9:16 over a blurred copy of itself | [examples](examples/README.md#vertical-video) |
| **Everything a page needs** — renditions, poster, sprite sheet and speech audio in one pass | [examples](examples/README.md#everything-a-page-needs-in-one-pass) |
| **Cuts that copy** — trims and joins that never touch the picture | [examples](examples/README.md#cuts-that-dont-re-encode) |
| **Overlays as HTML and CSS** — what geneva draws and what it won't | [examples](examples/README.md#writing-a-card) |
| **Colour** — BT.601 and BT.709 kept, untagged material guessed out loud, HDR tone-mapped by BT.2446 | [docs/color.md](docs/color.md) |
| **Scripts and agents** — `--format json`, coded diagnostics that point at the field, deterministic output | [docs/agents.md](docs/agents.md) |

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

`--for` sets everything a destination needs at once — size, codec, level,
quality, bitrate cap, keyframes, fast start, audio. `geneva targets` shows
the table, with a source and a date for every platform's numbers.

`--show-timeline` on any command prints the JSON instead of rendering,
which is the quickest way to a document that already works.

## Documentation

| Page | What's in it |
| --- | --- |
| [examples/README.md](examples/README.md) | Every example file, worked through with its document, its command and its output |
| [docs/timeline.md](docs/timeline.md) | The format: every field, its default and its rules |
| [docs/cli.md](docs/cli.md) | Every command and flag, the `--for` targets, containers and codecs |
| [docs/agents.md](docs/agents.md) | The short version, for scripts and AI agents |
| [docs/errors.md](docs/errors.md) | Every error code and what to do about it |
| [docs/color.md](docs/color.md) | Tags, guessing, the working space, HDR |
| [docs/architecture.md](docs/architecture.md) | How the renderer, the copy planner and the encoders fit together |

## How this was built

Claude wrote all of it — the Rust, the tests, the docs — running in Claude
Code, directed and reviewed by one person. The commit trailers say which
model wrote each commit.

Better to say so up front than let you work it out. If you're deciding
whether to trust the code, you should know where it came from; if you're
curious what this way of working produces, the repository is the answer,
bugs and fixes included.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, 10 to 20 minutes
cargo build --release
```

You'll need a C and C++ toolchain, cmake, meson, ninja, nasm, pkg-config and
clang. [CONTRIBUTING.md](CONTRIBUTING.md) lists the packages per platform and
explains how to run the tests;
[docs/architecture.md](docs/architecture.md) maps the crates.

## Licence

MIT. See [LICENSE](LICENSE). The bundled third-party components and their
licences are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
