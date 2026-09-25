<img src="docs/wordmark-any.png" alt="Geneva" width="240" height="77">

A command-line video editor. One binary, no dependencies: cut, join, convert and caption video, and draw HTML and CSS overlays on it without a browser, either with everyday commands or by handing it a JSON document that describes the whole edit.

It's built on ffmpeg's libraries but not its command line: it re-encodes only what your edit changes and copies the rest, and it checks the whole edit before rendering, so a mistake comes back as an error that points at it, not as a broken file and exit code 0.

https://github.com/user-attachments/assets/6d8566dd-7270-49bc-ac66-dab1585817a3

<sub>An AI agent made this on its first try, from one prompt, with nothing but geneva and whisper-cli. [The prompt is below.](#made-to-be-driven-by-agents)</sub>

## Install

```sh
# Linux, macOS
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.ps1 | iex
```

Or download an archive from [Releases](https://github.com/geneva-render/geneva/releases). Runs on Linux (x64, arm64), macOS on Apple silicon and Windows x64. Everything it needs is inside the binary.

## Try it

```sh
geneva trim match.mp4 -o goal.mp4 --from 41:10 --to 41:40   # no re-encode, done in a blink
geneva concat day1.mp4 day2.mp4 -o trip.mp4 --crossfade 1s
geneva convert talk.mov -o talk.mp4 --for web               # sensible codec, size and settings for the destination
geneva subtitles talk.mp4 -o talk-subbed.mp4 --burn talk.srt
geneva probe talk.mp4                                       # what's actually in the file
```

Stick `--show-timeline` on any of these and geneva prints the JSON document the command turns into, instead of running it. Full list of commands and flags: [docs/cli.md](docs/cli.md).

## Why I replaced ffmpeg's command line

Let me be clear about what's under the hood: FFmpeg's libraries. I didn't rewrite a single decoder, encoder or container format. They all come from libavcodec and libavformat, which are some of the best code in open source. What I replaced is ffmpeg's *command line*, the flags and filtergraphs we all copy from Stack Overflow, with an engine of my own. Not for nicer syntax, but because a command line that runs one pipeline can't know what your edit actually needs:

- **It works out the cheapest way to get the result.** Before touching a frame, geneva plans the whole job: which streams can be copied as they are, which frames really need re-encoding, which can go straight from decoder to encoder untouched. A frame-accurate cut re-encodes only the few frames between the cut and the next keyframe, and copies the rest (when your system has x264; see below). Changing only the audio leaves the picture alone. With ffmpeg, that's all on you, and one filter anywhere means everything gets re-encoded.
- **It catches mistakes before the render, not after.** The whole job is checked up front. Problems come back with a code, the exact place in the document and usually a hint:

  ```text
  error[E200]: unknown asset "crad"
    --> /layers/2/clips/0/source/asset = "crad"
     = help: did you mean "card"? assets are declared under "assets"
  ```

  ffmpeg is happy to hand you a broken file and exit 0, which matters most when the one running it is an agent that can't watch the result. In a small test I ran (20 editing tasks answered blind by one model, twice; I wrote the tasks, so it's a hint, not a benchmark), the ffmpeg answers produced 8 files that exited 0 and were quietly wrong: shifted colours, cuts tens of milliseconds off, sound drifting out of sync. The geneva answers got all 20 right in both runs, once I'd fixed the one bug the test turned up.
- **It has an actual compositor.** Layers, keyframes, masks, blend modes and transitions, on exact frame timing, blended in linear light with colour metadata preserved (and HDR tone-mapped when needed). Titles and graphics can be plain HTML and CSS, flexbox and `@keyframes` included, which geneva lays out itself. GPU when you have one, CPU when you don't.
- **It tells you what it did.** Every run ends with a few notes: the encoder it picked, what it copied, what it had to guess about your source. Add `--format json` and all of it is machine-readable.

**"Couldn't an agent just use Playwright and ffmpeg, or Remotion?"** It could, and people do: screenshot the HTML frame by frame in a headless browser, then have ffmpeg lay the shots over the footage. But everything apart from the overlay is still ffmpeg flags, with the pitfalls above. The browser's animations run on the wall clock, so its clock has to be faked for every frame. And it's a second full encode, with Chromium, Node and ffmpeg to install first. geneva does it in one pass with no browser. On the same 4-core machine with the same x264 settings, a 4-second name card over 9 seconds of 720p footage took 6 seconds against 14.7 for Playwright plus ffmpeg, and the Popeye video at the top took 113 seconds against 195. Remotion packages the same approach and says in its own docs not to use CSS `@keyframes` or transitions, since frames render out of order; geneva plays them as written, and the card's HTML file looks the same opened in a browser. What the browser does better: any CSS and any JavaScript, where geneva handles [a subset of CSS](docs/timeline.md#markup) and no JavaScript.

## One document, the whole edit

Every command above is shorthand for a document. Written by hand, one looks like this: nine seconds of space-station footage, captions from a word-timed transcript, and a name card that slides in at 2 seconds.

```json
{
  "geneva": "1.0",
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
        "source": { "kind": "captions", "asset": "words", "margin": "9%",
          "style": { "font": "500 34px/1.35 Liberation Sans", "color": "#b6c2cd",
                     "highlight": { "color": "#ffffff" }, "background": "#0a0f14cc",
                     "padding": "14px", "radius": "3px", "max_width": "66%" } } } ] },

    { "id": "lower-third", "clips": [ {
        "source": { "kind": "html", "asset": "card" }, "start": "2s", "duration": "4s" } ] }
  ]
}
```

The card is a plain HTML file, and opened in a browser it looks and moves the same:

```html
<style>
  @keyframes slide-in { from { translate: -100% } to { translate: 0 } }
  .card {
    animation: slide-in 0.5s ease-out;
    position: absolute; left: 4.4%; top: 7.5%; width: 33%;
    padding: 14px 24px;
    background: #0a0f14cc; border-left: 5px solid #c4362f;
  }
  /* ... */
</style>
<div class="card">
  <h1>Dragon CRS-17</h1>
  <p>BERTHING AT THE ISS &nbsp;&middot;&nbsp; NASA</p>
</div>
```

```sh
geneva render examples/lower-third.json -o dragon.mp4
```

<img src="docs/demo.webp" alt="A name card sliding in at the top left over footage of a Dragon capsule at the space station, with captions below" width="640" height="360">

The document says when things appear, the HTML says how they look. Percentages are relative to the frame, so the layout scales with the output. `commentary.json` is a word-timed transcript, the kind Whisper writes. Documents are plain JSON with a published schema, so they diff nicely, live happily in git and are easy for a program to write. The format is in [docs/timeline.md](docs/timeline.md); [examples/](examples/README.md) has the full card and more to copy from.

## Made to be driven by agents

Everything an agent needs is in the binary: `geneva guide` prints the manual, `geneva explain <code>` explains any error, and `--format json` works on every command.

The video at the top is an agent's first attempt with that and nothing else. It worked out the layout, wrote the lower third in HTML and CSS, pulled the headlines from a Whisper transcript of the clip, and rendered it, all from this prompt:

> Using only the built-in geneva guide and whisper-cli, take a 1m clip
> from public-domain popeye and overlay a realistic CNN style animated
> lower third for the whole clip (change the CNN logo to GNN, keep the
> style), with headlines derived from the clip itself. The source is 4:3,
> so output 16:9 1080p with the empty sides filled by a blurred copy of
> the video.

## H.264 and x264

geneva ships with OpenH264 for H.264. If x264 is installed on your system, geneva uses it instead, and you get noticeably smaller files. I don't bundle it because it's GPL and geneva is MIT.

```sh
sudo apt install libx264-164   # Debian 12, Ubuntu 24.04 (libx264-163 on 22.04)
brew install x264              # macOS
```

On Windows, drop `libx264-<build>.dll` next to `geneva.exe` (MSYS2's `mingw-w64-ucrt-x86_64-libx264` has one), or point `GENEVA_X264` at it.

## Docs

- [Commands and flags](docs/cli.md)
- [The document format](docs/timeline.md)
- [Examples](examples/README.md)
- [For scripts and agents](docs/agents.md)
- [Error codes](docs/errors.md)
- [Colour and HDR](docs/color.md)
- [How it's built](docs/architecture.md)
- [Changelog](CHANGELOG.md)

## How this was built

This was built almost entirely with Fable/Opus, which wrote the code,
tests and docs under my guidance. I am being upfront about that so you
can decide how much you trust the code.

## Where it's at

geneva is young, and for now it's just the engine: no GUI, no hosted service. What I build next depends on what people make with it, so if that's you, or you'd like it to be, [I'd love to hear what you're working on](https://genevarender.com/building). Issues and [discussions](https://github.com/geneva-render/geneva/discussions) are open.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, takes a while
cargo build --release
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the toolchain for each platform and how to run the tests.

## Licence

MIT. Bundled libraries and their licences are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
