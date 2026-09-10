# Geneva

Geneva is a composition and rendering engine for video, driven by a
declarative, versioned timeline format. It owns compositing, typography,
animation, color management and audio mixing, and delegates container
parsing, decoding and encoding to established media libraries.

**Status: pre-release (0.0.x).** The timeline format, validation, the CPU
reference renderer, text layout, media decoding and encoding, stream copy
and the everyday verbs work end to end. GPU rendering is not there yet.
Expect breaking changes to the format until the schema reaches 1.0.

## What works today

- `geneva validate timeline.json` — parses and validates a timeline, printing
  diagnostics that name the JSON path, the offending value, and a fix.
  `--probe` also opens the media files and checks their lengths.
- `geneva render timeline.json -o out.mp4` — renders the whole timeline:
  video clips, images, solids, shapes, nested compositions and text, with
  audio tracks mixed; H.264/H.265/VP9/AV1 video and AAC/Opus audio in
  MP4, MOV, MKV or WebM.
- `geneva frame timeline.json --at 1.5s -o frame.png` — renders one frame to
  PNG through the same renderer, for checking and iterating.
- `geneva probe clip.mp4` — shows streams, size, rate, duration and color
  tags, including which tags had to be assumed.
- `geneva schema` — prints the JSON Schema for the current timeline version.
- `geneva trim`, `concat`, `convert`, `resize`, `overlay` and `audio` —
  everyday tasks as one-line commands. Each builds a timeline and renders
  it through the same engine; `--show-timeline` prints that timeline so a
  quick job can grow into a full composition.

Geneva ships as a single binary with its media libraries built in; see
[Building from source](#building-from-source) if you build it yourself.
The [command-line reference](docs/cli.md) lists every command and option.

## Everyday tasks

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s            # copied, no re-encode
geneva concat part1.mp4 part2.mp4 -o all.mp4           # copied when the streams match
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s   # rendered
geneva resize talk.mp4 -o talk-720.mp4 --height 720
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5
geneva audio talk.mp4 -o talk.m4a --extract
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
```

Trims and joins that leave the picture untouched copy the source streams
and finish in the time it takes to read the files; cuts land on the
previous keyframe unless `--exact` asks for a re-encode. Anything that
changes the picture is rendered.

## Installing

Prebuilt binaries are published for each release on the
[releases page](https://github.com/geneva-render/geneva/releases) for Linux
(x86_64, arm64) and macOS (Apple silicon, Intel). Each archive contains the
`geneva` binary, this README, the license texts, and nothing else to install.

```sh
# Linux x86_64 (adjust the version and the target for your machine)
curl -fsSLO https://github.com/geneva-render/geneva/releases/download/v0.1.0/geneva-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf geneva-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 geneva-v0.1.0-x86_64-unknown-linux-gnu/geneva /usr/local/bin/geneva
geneva --help
```

Linux binaries need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, RHEL 9
and later). macOS binaries need macOS 12 or newer. There are no other
runtime requirements: codecs, container support and font shaping are built
in.

A first look on a fresh machine:

```sh
geneva probe input.mp4                          # what is in the file
geneva validate examples/lower-third.json       # check a timeline
geneva frame examples/lower-third.json --at 1s -o check.png
geneva render examples/lower-third.json -o out.mp4
```

## Quick look

```json
{
  "geneva": "0.1",
  "output": { "width": 1280, "height": 720, "fps": 30, "duration": "3s" },
  "assets": { "logo": { "src": "logo.png" } },
  "layers": [
    { "clips": [ { "source": { "kind": "solid", "color": "#1d2230" } } ] },
    {
      "clips": [
        {
          "source": { "kind": "image", "asset": "logo" },
          "start": "0.5s",
          "duration": "2s",
          "transform": {
            "position": { "x": "50%", "y": "50%" },
            "scale": { "keyframes": [
              { "t": 0, "v": 0.6, "ease": "ease-out" },
              { "t": "0.6s", "v": 1.0 }
            ] }
          },
          "opacity": { "keyframes": [ { "t": 0, "v": 0 }, { "t": "0.4s", "v": 1 } ] }
        }
      ]
    }
  ]
}
```

See [docs/timeline.md](docs/timeline.md) for the format reference,
[docs/cli.md](docs/cli.md) for the commands and
[docs/errors.md](docs/errors.md) for the diagnostic codes.

## Building from source

```sh
scripts/build-media-libs.sh   # builds the media libraries once, ~10-20 minutes
cargo build --release
```

The script needs a C/C++ toolchain, cmake, meson, ninja, nasm, pkg-config
and clang; see [CONTRIBUTING.md](CONTRIBUTING.md) for the per-platform
package lists and the test workflow. Third-party components and their
licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/geneva-timeline` | Timeline format: types, parsing, validation, resolution, JSON Schema |
| `crates/geneva-anim` | Keyframes and easing; animation as a pure function of time |
| `crates/geneva-color` | Color tags, inference, transfer functions, matrices, linear-light blending |
| `crates/geneva-render` | Renderer interface and the CPU reference renderer |
| `crates/geneva-golden` | Perceptual image comparison and the golden-frame test driver |
| `crates/geneva-media` | Probing, decoding, encoding and audio mixing over the bundled media libraries |
| `crates/geneva-cli` | The `geneva` command-line tool |
| `schema/` | Published JSON Schema files, one per timeline version |
| `docs/` | Format reference, command-line reference, diagnostics, color pipeline, architecture |
| `tests/golden/` | Golden scenes, their reference frames, and the fonts they use |
| `tests/media/` | Small media files used by tests |
| `scripts/` | The media library build and the benchmark script |
| `licenses/` | License texts of the bundled third-party components |

## License

Geneva is open source under the [MIT License](LICENSE). The bundled
third-party components and their licenses are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
