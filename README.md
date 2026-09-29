<img src="docs/wordmark-any.png" alt="Geneva" width="240" height="77">

**A command-line video editor. Edits are JSON documents; titles and graphics are HTML and CSS, drawn without a browser.**

One binary for Linux (x64, arm64), macOS (Apple silicon) and Windows (x64). The codecs come from FFmpeg's libraries; the planner, compositor and layout engine are written in Rust. MIT licensed.

https://github.com/user-attachments/assets/e6da7c2a-62eb-4852-8523-c9ea0ed192cf

<sub>Made by an AI agent on its first try, from one prompt, with whisper-cli and geneva. [The prompt.](#for-scripts-and-agents) Footage: Popeye, public domain.</sub>

## Install

```sh
# Linux, macOS
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.ps1 | iex
```

Or download an archive from [Releases](https://github.com/geneva-render/geneva/releases).

Optional: x264. H.264 is encoded with the bundled OpenH264 unless x264 is on the system; x264 gives smaller files and enables the smart cut. Not bundled because it is GPL.

```sh
sudo apt install libx264-164   # Debian 12, Ubuntu 24.04; libx264-163 on 22.04
brew install x264              # macOS
```

Windows: `libx264-<build>.dll` next to `geneva.exe` (from MSYS2's `mingw-w64-ucrt-x86_64-libx264`), or its path in `GENEVA_X264`.

## Everyday commands

```sh
geneva trim match.mp4 -o goal.mp4 --from 41:10 --to 41:40
geneva concat day1.mp4 day2.mp4 -o trip.mp4 --crossfade 1s
geneva convert talk.mov -o talk.mp4 --for web
geneva subtitles talk.mp4 -o talk-subbed.mp4 --burn talk.srt
geneva probe talk.mp4
```

Each command is turned into a JSON document and rendered. `--show-timeline` prints it. All commands: [docs/cli.md](docs/cli.md).

## A document

A name card over ten seconds of footage. The card is an HTML file; it plays the same in a browser.

`card.html`

```html
<style>
  @keyframes slide-in { from { translate: -100% } to { translate: 0 } }
  @keyframes fade-out { from { opacity: 1 } to { opacity: 0 } }
  .card {
    animation: slide-in 0.5s ease-out, fade-out 0.3s 3.7s;
    position: absolute; left: 4.4%; top: 7.5%; width: 33%;
    display: flex; flex-direction: column; gap: 6px; padding: 14px 24px;
    background: #0a0f14cc; border-left: 5px solid #c4362f;
  }
  .card h1 { margin: 0; font: 600 29px Liberation Sans; color: #f2f5f7 }
  .card p  { margin: 0; font: 500 13px Liberation Sans; color: #94a6b6 }
</style>
<div class="card">
  <h1>Dragon CRS-17</h1>
  <p>BERTHING AT THE ISS &middot; NASA</p>
</div>
```

`edit.json`

```json
{
  "geneva": "1.1",
  "output": { "width": 1280, "height": 720, "fps": 30 },
  "assets": { "iss": { "src": "iss.mp4" }, "card": { "src": "card.html" } },
  "layers": [
    { "id": "footage", "clips": [ { "source": { "kind": "video", "asset": "iss" } } ] },
    { "id": "card", "clips": [ { "source": { "kind": "html", "asset": "card" },
                                 "start": "2s", "duration": "4s" } ] }
  ]
}
```

```text
$ geneva render edit.json -o dragon.mp4
note[N600]: smart cut: 165 of 300 frames copied from the source, 135 encoded in 1 run around the cuts and overlays
note[N600]: H.264 runs encoded with the system's x264 (build 164) at CRF 18
wrote dragon.mp4 (300 frames, 10s of video)
```

<img src="docs/demo.webp" alt="A name card sliding in at the top left over footage of a Dragon capsule at the space station, with captions below" width="640" height="360">

<sub>The full example, [examples/lower-third.json](examples/lower-third.json), also adds word-by-word captions from a Whisper-format transcript. Footage: NASA, public domain.</sub>

Percentages in the card are relative to the frame, so the same card works at any output size.

Besides markup, a document can use layers, keyframes, masks, blend modes and transitions, blended in linear light. HDR sources are tone-mapped when the output is SDR. Each frame is computed from its timestamp, so it comes out the same on every render. The GPU is used when there is one, the CPU otherwise.

Frames with nothing drawn on them are copied from the source when it is H.264 and x264 is installed, and otherwise go from decoder to encoder without passing through the compositor. Errors are reported before rendering, with a code and the path in the document: `error[E200]: unknown asset "crad"`.

## Motion graphics

Both of these are one HTML file and a short JSON document. No footage in the first; the second cuts to footage at the end.

https://github.com/user-attachments/assets/d41ebeac-7a27-4a09-a158-819bc6e01bcf

<sub>1920x1080, 30 fps, 14 s. About 40 s to render on 4 vCPUs with no GPU. Fonts: Inter, JetBrains Mono (SIL OFL), shipped with the document as font assets. Source: [race.html](https://genevarender.com/src/race/race.html), [race.json](https://genevarender.com/src/race/race.json).</sub>

https://github.com/user-attachments/assets/52bdd3ca-f930-423b-8b88-9e67e3307d54

<sub>1920x1080, 30 fps, 13 s. About 70 s to render on 4 vCPUs with no GPU. Fonts: Inter, JetBrains Mono (SIL OFL), shipped with the document as font assets. Source: [opening.html](https://genevarender.com/src/opening/opening.html), [opening.json](https://genevarender.com/src/opening/opening.json). Footage: "Shibuya Crossing, Tokyo, Japan" by Basile Morin, [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), 960x540 scaled up to fill the frame; this video is under the same licence.</sub>

## What the markup supports

Flexbox and block layout, absolute positioning, gradients, `box-shadow`, `filter: blur()`, `clip-path: polygon()`, `mix-blend-mode`, `background-clip: text`, web fonts shipped as assets, and CSS `@keyframes` played as written.

Not supported: CSS grid, floats, transitions, static `transform` (transforms come from animations), and JavaScript. An unsupported declaration is reported by name and skipped. The full list is in [docs/timeline.md](docs/timeline.md#markup).

## Speed

Seven jobs, rendered by geneva, by Playwright screenshotting each frame for ffmpeg, and by Remotion. Time to render, median of three rounds. One machine: 4 vCPUs of an Intel Xeon at 2.1 GHz, 16 GB, no GPU. All three encode x264 CRF 23, preset medium.

| Job | geneva | Playwright + ffmpeg | Remotion |
| --- | ---: | ---: | ---: |
| Social clip, 60 s, 1080x1920 | 60.8 s | 140.6 s | 270.3 s |
| Broadcast package, 3 min | 52.2 s | 244.2 s | 324.3 s |
| Static captions, 4 min | 95.6 s | 300.4 s | 444.7 s |
| Dynamic captions, 90 s | 58.1 s | 190.9 s | 164.7 s |
| Opening, 13 s | 10.0 s | 64.2 s | 19.1 s |
| Intro, 14 s | 30.4 s | 145.0 s | 63.4 s |
| Kinetic typography, 16 s | 14.2 s | 56.2 s | 30.2 s |

geneva with its defaults, which copy frames the edit leaves alone. With frame copying off and the encoder pinned to the others' settings, the broadcast package takes 99.4 s and the static captions 140.7 s. Method, caveats and scripts: [docs/benchmarks.md](docs/benchmarks.md).

## For scripts and agents

`geneva guide` prints the manual, `geneva explain <code>` explains any diagnostic, and `--format json` works on every command. The document format has a [published JSON schema](schema). See [docs/agents.md](docs/agents.md).

The video at the top was made by an agent with those and nothing else. It worked out the layout, wrote the lower third in HTML and CSS, took the headlines from a Whisper transcript of the clip, and rendered it on the first try. The prompt:

> Using only the built-in geneva guide and whisper-cli, take a 1m clip
> from public-domain popeye and overlay a realistic CNN style animated
> lower third for the whole clip (change the CNN logo to GNN, keep the
> style), with headlines derived from the clip itself. The source is 4:3,
> so output 16:9 1080p with the empty sides filled by a blurred copy of
> the video.

## Status

- Version 1.1. The document format is `"1.1"`; 1.x adds only optional fields, so a `"1.0"` document is read as it is.
- No GUI and no hosted service.
- Under consideration: splitting a long render into chunks that run in parallel on serverless functions, then joining them into one file.
- If you are using it or want to, [tell me what you are making](https://genevarender.com/building). Issues and [discussions](https://github.com/geneva-render/geneva/discussions) are open.

## Docs

- [Commands and flags](docs/cli.md)
- [The document format](docs/timeline.md)
- [Examples](examples/README.md)
- [For scripts and agents](docs/agents.md)
- [Error codes](docs/errors.md)
- [Colour and HDR](docs/color.md)
- [Benchmarks](docs/benchmarks.md)
- [Architecture](docs/architecture.md)
- [Changelog](CHANGELOG.md)

## How this was built

The code, tests and docs were written almost entirely by AI models (Fable and Opus) under my guidance. This is stated so you can decide how much to trust the code.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, takes a while
cargo build --release
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the toolchain for each platform and how to run the tests.

## Licence

MIT. Bundled libraries and their licences: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
