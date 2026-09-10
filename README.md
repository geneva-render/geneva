# Geneva

Geneva is a composition and rendering engine for video, driven by a
declarative, versioned timeline format. It owns compositing, typography,
animation, color management and audio mixing, and delegates container
parsing, decoding and encoding to established media libraries.

**Status: pre-release (0.0.x).** The timeline format, validation, the CPU
reference renderer, text layout, media decoding and encoding work end to
end. GPU rendering, the everyday task verbs and hardware encoders are not
there yet. Expect breaking changes to the format until the schema reaches
1.0.

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

Media support needs the libav libraries (FFmpeg 6 or 7) installed as shared
libraries with development headers; see [Building](#building).

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

See [docs/timeline.md](docs/timeline.md) for the format reference and
[docs/errors.md](docs/errors.md) for the diagnostic codes.

## Building

Geneva is a Rust workspace. It links against the libav libraries for media
I/O, so install them first:

```sh
# Debian/Ubuntu
sudo apt install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev clang
# macOS
brew install ffmpeg
```

Then, with a stable Rust toolchain:

```sh
cargo build --release
cargo test
./target/release/geneva --help
```

To build without media support (validation, PNG frames of non-video
timelines, schema): `cargo build --no-default-features`.

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/geneva-timeline` | Timeline format: types, parsing, validation, resolution, JSON Schema |
| `crates/geneva-anim` | Keyframes and easing; animation as a pure function of time |
| `crates/geneva-color` | Color tags, inference, transfer functions, matrices, linear-light blending |
| `crates/geneva-render` | Renderer interface and the CPU reference renderer |
| `crates/geneva-golden` | Perceptual image comparison and the golden-frame test driver |
| `crates/geneva-media` | Probing, decoding, encoding and audio mixing over the libav libraries |
| `crates/geneva-cli` | The `geneva` command-line tool |
| `schema/` | Published JSON Schema files, one per timeline version |
| `docs/` | Format reference, diagnostics, color pipeline, architecture |
| `tests/golden/` | Golden scenes, their reference frames, and the fonts they use |
| `tests/media/` | Small media files used by tests |

## License

Geneva is source-available under the [Geneva License 1.0](LICENSE.md). It is
free for individuals, non-profits, education, evaluation, and companies of up
to three people. Larger companies need a commercial license; see
[COMMERCIAL.md](COMMERCIAL.md).
