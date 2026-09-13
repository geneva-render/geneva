# Geneva

Geneva is a composition and rendering engine for video, driven by a
declarative, versioned timeline format. It owns compositing, typography,
animation, color management and audio mixing, and delegates container
parsing, decoding and encoding to established media libraries.

**Status: 0.2.** The timeline format (now with crop, effects, masks and
speed on clips), validation, the CPU reference renderer, text layout,
media decoding and encoding, stream copy, smart cuts and the everyday
verbs work end to end. GPU rendering is not there yet. Expect breaking
changes to the format until the schema reaches 1.0; see
[CHANGELOG.md](CHANGELOG.md).

## What works today

- `geneva validate timeline.json` — parses and validates a timeline, printing
  diagnostics that name the JSON path, the offending value, and a fix.
  `--probe` also opens the media files and checks their lengths.
- `geneva render timeline.json -o out.mp4` — renders the whole timeline:
  video clips, images, solids, shapes, nested compositions and text, with
  audio tracks mixed; H.264, H.265, VP9, AV1, ProRes, DNxHR, PNG and
  Motion JPEG video, AAC, Opus, MP3, Vorbis, FLAC, ALAC, AC-3 and PCM
  audio, subtitle streams from SRT or WebVTT files, in MP4, MOV, MKV, WebM,
  MXF, audio-only files or image sequences.
- `geneva frame timeline.json --at 1.5s -o frame.png` — renders one frame to
  PNG through the same renderer, for checking and iterating.
- `geneva probe clip.mp4` — shows streams, size, rate, duration and color
  tags, including which tags had to be assumed.
- `geneva schema` — prints the JSON Schema for the current timeline version.
- `geneva trim`, `concat`, `convert`, `resize`, `overlay`, `audio` and
  `subtitles` —
  everyday tasks as one-line commands. Each builds a timeline and renders
  it through the same engine; `--show-timeline` prints that timeline so a
  quick job can grow into a full composition.

Geneva ships as a single binary with its media libraries built in; see
[Building from source](#building-from-source) if you build it yourself.
The [command-line reference](docs/cli.md) lists every command and option;
[docs/agents.md](docs/agents.md) is the short version for programs and AI
agents driving `geneva`.

## Everyday tasks

```sh
geneva trim talk.mp4 -o intro.mp4 --to 30s            # copied, no re-encode
geneva concat part1.mp4 part2.mp4 -o all.mp4           # copied when the streams match
geneva concat a.mp4 b.mp4 -o ab.mp4 --crossfade 0.5s   # rendered
geneva resize talk.mp4 -o talk-720.mp4 --height 720
geneva overlay talk.mp4 logo.png -o branded.mp4 --at bottom-right --scale 0.5
geneva audio talk.mp4 -o talk.m4a --extract
geneva audio talk.mp4 -o scored.mp4 --mix music.mp3 --gain -12
geneva convert talk.mp4 -o talk.mov --codec prores --profile hq      # 10-bit 4:2:2
geneva convert talk.mp4 -o frames/%04d.png                           # image sequence
geneva subtitles talk.mp4 -o talk-subbed.mkv --add en.srt --language en
geneva subtitles talk.mp4 -o talk-burned.mp4 --burn en.srt
geneva convert master.mov -o reel.mp4 --for instagram     # size, codec, quality, caps from a table
```

Trims and joins that leave the picture untouched copy the source streams
and finish in the time it takes to read the files; cuts land on the
previous keyframe unless `--exact` asks for a re-encode. Anything that
changes the picture is rendered.

## Installing

Prebuilt binaries are published for each release on the
[releases page](https://github.com/geneva-render/geneva/releases) for Linux
(x86_64, arm64) and macOS (Apple silicon). Each archive contains the
`geneva` binary, an installer, a check script, this README and the license
texts; there is nothing else to install.

```sh
# Pick the archive for your machine (adjust the version and the target)
curl -fsSLO https://github.com/geneva-render/geneva/releases/download/v0.3.5/geneva-v0.3.5-aarch64-apple-darwin.tar.gz
tar xzf geneva-v0.3.5-aarch64-apple-darwin.tar.gz
sh geneva-v0.3.5-aarch64-apple-darwin/install.sh
geneva --help
```

`install.sh` copies the binary to `/usr/local/bin` when that is writable
and to `~/.local/bin` otherwise (`GENEVA_PREFIX` overrides the choice), and
on macOS clears the quarantine flag a browser download carries, so the
binary starts without a Gatekeeper detour. Running the script straight
from the repository downloads the latest release first:

```sh
curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
```

To see that everything works on your machine, run the check script from
the archive. It renders a short clip, runs the everyday commands on it (or
on a file you pass) and prints a table with the time each step took, next
to the time ffmpeg takes for the same step when ffmpeg is installed:

```sh
sh geneva-v0.3.5-aarch64-apple-darwin/check.sh            # built-in test clip
sh geneva-v0.3.5-aarch64-apple-darwin/check.sh input.mp4  # your own file
```

Linux binaries need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, RHEL 9
and later). macOS binaries need macOS 12 or newer on Apple silicon; Intel
Macs build from source. There are no other runtime requirements: codecs,
container support and font shaping are built in.

One optional extra: software H.264 is encoded by the bundled OpenH264,
which writes larger files than x264 at the same quality. geneva does not
include x264 (it is GPL-licensed), but uses the system's copy when one is
installed, and says so in the notes of each render. Install it before or
after geneva, from your distribution or Homebrew:

```sh
sudo apt install libx264-164      # Debian 12, Ubuntu 24.04 (libx264-163 on Ubuntu 22.04)
sudo dnf install x264-libs        # Fedora (RPM Fusion)
brew install x264                 # macOS, for renders that ask for a software encoder
```

Hardware encoders (VideoToolbox on macOS, NVENC on Linux) still come
first when present. `GENEVA_X264=off` turns the lookup off;
`GENEVA_X264=/path/to/libx264.so` names a library file.

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
  "geneva": "0.2",
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
| `docs/` | Format reference, command-line reference, agent guide, diagnostics, color pipeline, architecture |
| `tests/golden/` | Golden scenes, their reference frames, and the fonts they use |
| `tests/media/` | Small media files used by tests |
| `scripts/` | The media library build and the benchmark script |
| `licenses/` | License texts of the bundled third-party components |

## License

Geneva is open source under the [MIT License](LICENSE). The bundled
third-party components and their licenses are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
