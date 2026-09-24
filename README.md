<img src="docs/wordmark-any.png" alt="Geneva" width="240" height="77">

geneva is a programmatic video editor, built as an alternative to the ffmpeg command line. It's designed to work well with AI agents, too.

Simple jobs take one line. More complex ones go in as JSON + optional assets, so you can read, diff, validate and check into git.

geneva validates the job before decoding anything, so bad options fail
immediately instead of an hour into the render. It also reports what it
is doing as it runs.

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s        # copies the streams, no re-encode
geneva subtitles talk.mp4 -o subbed.mp4 --burn transcript.json --highlight "#ffd233"
geneva render job.json -o out/                    # everything the document asks for
```

Under the hood is a compositor: layers, keyframes, shaped text, masks and
blend modes. Overlays use HTML and CSS, including flexbox, the box model
and `@keyframes`, without a browser. Colour is handled in linear light,
and video goes through libavformat and libavcodec.

## How it works

Here is a name card and captions over ten seconds of space-station
footage. The card is just an HTML file. Open it in a browser and it looks and
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

The captions come from `commentary.json`, in the JSON format Whisper
writes. The clip has no sound, so the words were written for it rather
than recognised. The JSON ties the footage, card and captions together:

```json
{
  "geneva": "0.5",
  "output": { "width": 1280, "height": 720, "fps": 30 },

  "assets": {
    "iss":   { "src": "iss.mp4" },
    "card":  { "src": "card.html" },
    "words": { "src": "commentary.json" }
  },

  "layers": [
    { "id": "footage", "clips": [ {
        "source": { "kind": "video", "asset": "iss" }, "duration": "9s" } ] },

    { "id": "captions", "clips": [ {
        "source": {
          "kind": "captions", "asset": "words", "margin": "9%",
          "style": {
            "font": "500 34px/1.35 Liberation Sans",
            "color": "#b6c2cd",
            "highlight": { "color": "#ffffff" },
            "background": "#0a0f14cc",
            "padding": "14px",
            "radius": "3px",
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

<img src="docs/demo.webp" alt="A name card sliding in at the top left over footage of a Dragon capsule at the space station, with captions below" width="640" height="360">

```text
note[N453]: 3 cues read from "commentary.json", grouped at up to 2 lines of 42 characters
note[N600]: the video is used as it is, so frames are handed to the encoder as decoded, with the layers above drawn onto the frames that show them
note[N600]: H.264 encoded with the system's x264 (build 164)
note[N600]: overlays were drawn onto 222 of 270 frames; the others went from the decoder to the encoder untouched
wrote dragon.mp4 (270 frames, 9s of video, 4.6s elapsed)
```

The document controls when things appear. The HTML controls how they
look. geneva handles the CSS layout itself, without a browser.
Percentages are relative to the frame, so the layout scales with the
output.

Overlays are drawn onto 222 of the 270 frames. The other 48, where
nothing is on screen, go from the decoder to the encoder without being
composited.

## Good transitions and nice sprites, via cli

LLMs can be shown an example, and write custom good-looking intros and
transitions as [HTML+CSS markup](examples/opening.html). geneva can
render them natively over the footage using a simple
[JSON file](examples/opening.json), in a single pass.

https://github.com/user-attachments/assets/6e6456f3-2bb4-4b00-917f-8e793622815d

**An agent's first attempt**

An agent with the `geneva` binary and `whisper-cli`, and no checkout,
made this from the prompt below on its first try:

> Using only the built-in geneva guide and whisper-cli, take a 1m clip
> from public-domain popeye and overlay a realistic CNN style animated
> lower third for the whole clip (change the CNN logo to GNN, keep the
> style), with headlines derived from the clip itself. The source is 4:3,
> so output 16:9 1080p with the empty sides filled by a blurred copy of
> the video.

https://github.com/user-attachments/assets/6d8566dd-7270-49bc-ac66-dab1585817a3

The whole 62 s clip, re-encoded to fit GitHub's 10 MB upload limit.

**Compared with ffmpeg and Remotion**

| | ffmpeg | Remotion | geneva |
| --- | --- | --- | --- |
| Layout | none: `drawtext` at pixel positions | a browser | HTML and CSS, a [subset](docs/timeline.md#known-limitations) |
| Animation | an expression per filter option | `interpolate()` per frame; [not `@keyframes`](https://www.remotion.dev/docs/troubleshooting/css-animations) | `@keyframes` as written |
| Runs on | one binary | Node, headless Chrome, then ffmpeg | one binary |
| Mistakes | often a wrong file with exit status 0 | type errors when bundling; the rest in the output | checked before rendering, with a code and the field |
| Unchanged frames | re-encoded once any filter runs | re-encoded | copied where the source allows |
| Licence | LGPL or GPL | paid for companies of 4 or more | MIT |

## Installing

On Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.ps1 | iex
```

The archives are on the [releases
page](https://github.com/geneva-render/geneva/releases) if you would
rather pick one yourself. Linux needs glibc 2.35+, macOS 12+ on Apple
silicon, Windows 10 or 11 on x64. Codecs, containers and font shaping
are built in.

One thing to know about H.264: geneva bundles OpenH264, which produces
larger files than x264 at the same quality. It does not bundle x264
because it is GPL, but it will use your system copy if available and
always reports which encoder it used.

```sh
sudo apt install libx264-164     # Debian 12, Ubuntu 24.04 (libx264-163 on 22.04)
brew install x264                # macOS
```

On Windows it looks for `libx264-<build>.dll` next to `geneva.exe` (the
one in MSYS2's `mingw-w64-ucrt-x86_64-libx264` package works on its own),
or takes the file that `GENEVA_X264` names.

## What else it does

| | What it does | Where |
| --- | --- | --- |
| **Cut and join without re-encoding** | Copies the streams instead of decoding and encoding them again | [examples](examples/README.md#cuts-that-dont-re-encode) |
| **Several outputs in one pass** | Multiple renditions, a poster, and speech audio from one read | [examples](examples/README.md#one-read-many-files) |
| **Vertical video reframing** | Turns 16:9 into 9:16 over a blurred copy instead of cropping | [examples](examples/README.md#vertical-video) |
| **Destination presets** | `--for instagram`, `--for web`, `--for phone` set size, codec, quality, keyframes, audio and, where the platform normalises it, loudness, and warn when limits are exceeded | [docs/cli.md](docs/cli.md#targets) |
| **Transitions** | Dissolve or dip through a colour at joins or clip boundaries, with picture and sound kept together | [docs/timeline.md](docs/timeline.md#transitions) |
| **Colour** | Preserves BT.601/BT.709, reports guesses for untagged material, and tone-maps HDR with BT.2446 | [docs/color.md](docs/color.md) |
| **Scripts and agents** | `--format json`, field-level diagnostics, deterministic output | [docs/agents.md](docs/agents.md) |

## Commands

Every command and flag is in [docs/cli.md](docs/cli.md).

```sh
geneva resize talk.mp4 -o talk-720.mp4 --height 720
geneva trim talk.mp4 -o clip.mp4 --from 12s --exact     # frame-accurate, smart cut
geneva concat part1.mp4 part2.mp4 -o all.mp4            # copied when the streams match
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s    # dissolve, picture and sound
geneva concat a.mp4 b.mp4 -o ab.mp4 --fade 0.6s         # dip through black
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5
geneva convert talk.mp4 -o web.mp4 --for web            # copied if a browser can already play it
geneva convert talk.mov -o talk.mp4 --crf 20 --preset slow --fps 30 --height 1080
geneva convert talk.mov -o talk.mkv --max-bitrate 6M --audio-codec opus --audio-bitrate 128k --sample-rate 48k
geneva convert talk.mp4 -o talk.mov --codec prores --profile hq
geneva convert talk.mp4 -o frames/%04d.png              # image sequence
geneva audio talk.mp4 -o talk.wav --extract --speech    # 16 kHz mono, for whisper etc.
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
geneva subtitles talk.mp4 -o burned.mp4 --burn en.srt --fit
geneva frame talk.mp4 -o thumb.jpg                      # first clear frame past the opening
geneva probe talk.mp4                                   # streams, colour tags, what was guessed
```

`geneva targets` prints the preset table. Add `--show-timeline` to any
verb to print its JSON instead of rendering it. It is the quickest way
to get a working document.

## Documentation

| Page | What's in it |
| --- | --- |
| [examples/README.md](examples/README.md) | Examples with their document, command and output |
| [docs/timeline.md](docs/timeline.md) | The format, fields, defaults and rules |
| [docs/cli.md](docs/cli.md) | Commands, flags, targets, containers and codecs |
| [docs/agents.md](docs/agents.md) | The short version for scripts and AI agents |
| [docs/errors.md](docs/errors.md) | Error codes and what to do about them |
| [docs/color.md](docs/color.md) | Colour tags, guessing, working space and HDR |
| [docs/architecture.md](docs/architecture.md) | Renderer, copy planner and encoders |
| [CHANGELOG.md](CHANGELOG.md) | Version changes and unresolved format decisions |

The first five of those pages are carried in the binary too, so a
machine that has `geneva` and no checkout can read them: `geneva guide`
prints the agent page, `geneva guide --list` names the rest, and
`geneva explain E302` says what one diagnostic code means.

## How this was built

This was built almost entirely with Fable/Opus, which wrote the code,
tests and docs under my guidance. I am being upfront about that so you
can decide how much you trust the code.

## Where this is going

geneva is pre-1.0. The timeline format still moves between minor
versions, and the engine is the whole of it: there is no hosted service
and no editor.

Two directions are being weighed. One is running renders for people who
would rather not run the infrastructure. The other is describing an edit
and getting it, which is nearer than it sounds: given `geneva guide` and
no checkout, an agent built a lower third over a talk, with the
headlines written from the talk's own transcript and word-timed captions
under them, from a description in English.

Which of those gets built depends on what people are making. If that
includes you, [tell me what](https://genevarender.com/building), or open
a [discussion](https://github.com/geneva-render/geneva/discussions) if
you would rather not leave an address.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, 10 to 20 minutes
cargo build --release
```

You will need a C/C++ toolchain, cmake, meson, ninja, nasm, pkg-config
and clang. [CONTRIBUTING.md](CONTRIBUTING.md) lists the packages for each
platform and explains how to run the tests.
[docs/architecture.md](docs/architecture.md) maps the crates.

## Licence

MIT.

See [LICENSE](LICENSE).

Bundled third-party components and their licences are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
