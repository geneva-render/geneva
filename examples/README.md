# Examples

Each file is a complete timeline. `talk.mp4` is a four-second 1080p clip,
made with geneva from `solid.json`-style shapes, so the examples that need
video run as they are.

| File | Shows | Run it |
| --- | --- | --- |
| `captions.json` | Word-by-word highlighted captions with an outline and a background box, in Latin, Arabic, Hebrew and Thai. | `geneva frame examples/captions.json --at 0.8s -o out.png` |
| `social-reframe.json` | A landscape clip reframed to 9:16: the picture whole over a blurred, cover-fitted copy of itself, with timed captions. | `geneva render examples/social-reframe.json -o reel.mp4` |
| `renditions.json` | An `outputs` map: three renditions, a poster, a sprite sheet and 16 kHz mono speech audio, from one pass. | `geneva render examples/renditions.json -o out/` |
| `lower-third.json` | A reusable composition (plate, accent bar, springing dot, name, title) placed twice with different transforms. | `geneva frame examples/lower-third.json --at 1.2s -o out.png` |
| `shapes.json` | Keyframed position, scale, rotation and opacity, with eased and spring interpolation. | `geneva frame examples/shapes.json --at 1s -o out.png` |
| `overlay.json` | Video clips with a crossfade, an image overlay, timed caption text and an audio bed. Needs media of your own. | `geneva render examples/overlay.json --assets DIR -o out.mp4` |
| `solid.json` | The smallest useful document: one solid-colour layer. | `geneva frame examples/solid.json -o out.png` |

`render` and `frame` open the media to learn its length, so a clip with no
`duration` takes it from the file. Plain `geneva validate` does not open
anything; add `--probe` when a document leaves lengths to the media.

```sh
geneva validate examples/renditions.json --probe
geneva --format json validate examples/shapes.json
```
