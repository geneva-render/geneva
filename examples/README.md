# Examples

Each file is a complete timeline that passes `geneva validate`.

| File | Shows | Renders on its own? |
| --- | --- | --- |
| `captions.json` | Word-by-word highlighted captions with an outline and a background box, in Latin, Arabic, Hebrew and Thai. Uses the system font "Liberation Sans". | Yes |
| `social-reframe.json` | A landscape talk reframed to 9:16: the picture whole over a blurred, cover-fitted copy of itself, with timed captions over the top. | Needs `talk.mp4` |
| `renditions.json` | An `outputs` map: three renditions, a poster, a sprite sheet and 16 kHz mono speech audio, from one pass. | Needs `talk.mp4` |
| `lower-third.json` | A reusable composition (plate, accent bar, springing dot, name and title) placed twice with different transforms. | Yes |
| `shapes.json` | Shapes with keyframed position, scale, rotation and opacity, and spring easing. | Yes |
| `overlay.json` | Video clips with a crossfade, an image overlay, timed caption text and an audio bed. | Needs its media |
| `solid.json` | The smallest useful document: one solid-colour layer. | Yes |

Try:

```sh
geneva validate examples/renditions.json
geneva frame examples/captions.json --at 0.8s -o captions.png
geneva frame examples/lower-third.json --at 1.2s -o lower-third.png
geneva --format json validate examples/shapes.json
```

The three that need media take any file you point them at: put it next to
the document, or pass `--assets DIR`. They set `output.duration` so they
validate without the file being present; `geneva render` probes the media
and an open-ended clip takes its length from the file.
