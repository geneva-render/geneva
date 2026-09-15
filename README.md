<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/wordmark-dark.png">
  <img src="docs/wordmark.png" alt="Geneva" width="240">
</picture>

geneva edits and processes video programmatically. One-line commands cover
the everyday jobs; a JSON document covers anything more. Both run the same
engine, and the document is a file you can read, diff and check into a
repository.

It reads the job before it decodes anything, so a mistake is an error that
names the field rather than a bad file an hour later, and it reports what
it did on the way through.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s        # copies the streams, no re-encode
geneva subtitles talk.mp4 -o subbed.mp4 --burn transcript.json --highlight "#ffd233"
geneva render job.json -o out/                    # everything the document asks for
```

Underneath is a compositor: layers, keyframes, shaped text, masks, blend
modes. Overlays can be written as HTML and CSS, including flexbox, the box
model and `@keyframes`, and geneva lays them out without a browser. Colour
is handled in linear light.

One binary. It uses FFmpeg's libraries to read and write files, so it
opens what ffmpeg opens.

**Status: 0.4.** The format, the renderer, the commands and the encoders
all work. There's no GPU rendering yet, and the format still changes
between versions until 1.0. See [CHANGELOG.md](CHANGELOG.md).

MIT licensed. Linux and macOS. No services, no network access.

## How it works

A name card and captions over ten seconds of footage from the space
station. The card is an HTML file. Open it in a browser and it looks and
moves the same:

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
    top: 7.5%;
    width: 33%;

    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px 24px;
    box-sizing: border-box;

    background: #0a0f14cc;
    border-left: 5px solid #c4362f;
    box-shadow: 0 4px 18px #00000059;
  }
  .card h1 {
    margin: 0;
    font: 600 29px Liberation Sans;
    color: #f2f5f7;
  }
  .card p {
    margin: 0;
    font: 500 13px Liberation Sans;
    letter-spacing: 1.4px;
    color: #94a6b6;
  }
</style>

<div class="card">
  <h1>Dragon CRS-17</h1>
  <p>BERTHING AT THE ISS &nbsp;&middot;&nbsp; NASA</p>
</div>
```

The captions come from `words.json`, a whisper transcript pasted in as it
came out. The document says when the card is on screen and what the
captions look like, and nothing else:

```json
{
  "geneva": "0.3",
  "output": { "width": 1280, "height": 720, "fps": 30 },

  "assets": {
    "iss":   { "src": "iss.mp4" },
    "card":  { "src": "card.html" },
    "words": { "src": "words.json" }
  },

  "layers": [
    { "id": "footage", "clips": [ { "source": { "kind": "video", "asset": "iss" } } ] },

    { "id": "captions", "clips": [ {
        "source": {
          "kind": "captions", "asset": "words", "margin": "9%",
          "style": {
            "font": "500 34px/1.35 Liberation Sans",
            "color": "#b6c2cd",
            "highlight": { "color": "#ffffff" },
            "background": "#0a0f14cc",
            "padding": "14px",
            "radius": 3,
            "max_width": "66%"
          } } } ] },

    { "id": "lower-third", "clips": [ {
        "source": { "kind": "html", "asset": "card" },
        "start": "2s", "duration": "4s" } ] }
  ]
}
```

```sh
geneva render examples/lower-third.json -o dragon.mp4
```

<img src="docs/demo.gif" alt="A name card sliding in at the top left over footage of a Dragon capsule at the space station, with captions below" width="640" height="360">

```
note[N453]: 2 cues read from "words.json"
note[N600]: smart cut: 135 of 300 frames copied from the source, 165 encoded in 1 run around the cuts and overlays
note[N600]: H.264 runs encoded with the system's x264 (build 164) at CRF 18
wrote dragon.mp4 (300 frames, 10s of video, 4.2s elapsed)
```

The JSON says when the card appears and what the captions look like.
Everything else is the card's business, and everything in the card is CSS:
`display: flex` is flexbox, `padding` and `border-left` are the box model,
`position: absolute` with `left` and `top` places it the way it would on a
page. geneva cascades the styles and lays them out with
[taffy](https://github.com/DioxusLabs/taffy), so the layout implements the
spec rather than guessing at it.

There are no pixel coordinates. `left: 4.4%` and `width: 33%` are shares of
the frame, and `translate: -100%` is the card's own width, so the slide
starts off its own edge whatever size the output is. The motion is in the
same file, as `@keyframes` and an `animation`, taking what CSS takes plus
`spring(170, 26)`. It is not a browser, and anything it cannot draw is a
warning that names it rather than a silent difference.
[examples/README.md](examples/README.md#writing-a-card) says what the
subset covers and where it stops.

The captions are one clip in the document and one clip per cue on the
timeline. The transcript carries the timing, so nothing here says when a
word lands; `margin` and the style fields say where the cues sit and what
they look like.

The last line of the report is worth reading twice. Nothing is on screen
for the first second or the last four, so geneva copied 135 of the 300
frames as they were and re-encoded only the 165 it had to. On a
ninety-minute film with a watermark on the title card, that is the
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
itself because x264 is GPL. It will use the copy on your system if there is
one, and it always says which encoder it used.

```sh
sudo apt install libx264-164     # Debian 12, Ubuntu 24.04 (libx264-163 on 22.04)
brew install x264                # macOS
```

## What else it does

Four of these are runnable examples, with their document, command and
output. The rest point at the reference.

| | What it does | Where |
| --- | --- | --- |
| **Captions from a transcript** | whisper's JSON in, each word picked out as it is said. One command burns them into a file; as a source they are one layer of a larger document, placed and styled | [examples](examples/README.md#captions) |
| **Cuts that copy** | trims and joins that never touch the picture, so they take about as long as reading the file | [examples](examples/README.md#cuts-that-dont-re-encode) |
| **One read, many files** | renditions, a poster, a sprite sheet and speech audio written in a single pass over the source | [examples](examples/README.md#one-read-many-files) |
| **Vertical video** | 16:9 into 9:16 over a blurred copy of itself | [examples](examples/README.md#vertical-video) |
| **Delivery presets** | `--for tiktok`, `--for web`, `--for email` set size, codec, quality, keyframes and audio for a destination, and warn when the result runs past its duration or file-size limit. Every number says where it came from and when it was last checked | [docs/cli.md](docs/cli.md#targets) |
| **Transitions** | dissolve or dip through a colour, at a join or at the head and tail of a piece. One field moves the picture and the sound together, rather than a video filter and an audio filter that have to be kept in step | [docs/timeline.md](docs/timeline.md#transitions) |
| **Colour** | BT.601 and BT.709 kept, untagged material guessed out loud, HDR tone-mapped by BT.2446 | [docs/color.md](docs/color.md) |
| **Scripts and agents** | `--format json`, coded diagnostics that point at the field, deterministic output | [docs/agents.md](docs/agents.md) |

## Commands

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s              # copied, no re-encode
geneva trim talk.mp4 -o clip.mp4 --from 12s --exact     # frame-accurate, smart cut
geneva concat part1.mp4 part2.mp4 -o all.mp4            # copied when the streams match
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s    # dissolve, picture and sound
geneva concat a.mp4 b.mp4 -o ab.mp4 --fade 0.6s         # dip through black
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

`geneva targets` prints the preset table. `--show-timeline` on any command
prints the JSON instead of rendering, which is the quickest way to a
document that already works.

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

Claude wrote all of it, the Rust and the tests and the docs, running in
Claude Code, directed and reviewed by one person. The commit trailers say
which model wrote each commit.

Better to say so up front than let you work it out. If you are deciding
whether to trust the code, you should know where it came from. If you are
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
