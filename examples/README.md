# Examples

Each file is a complete timeline. All of them pass `geneva validate`.

| File | Shows | Renders with `geneva frame`? |
| --- | --- | --- |
| `solid.json` | The smallest useful document: one solid-color layer. | Yes |
| `shapes.json` | Shapes, keyframed position, scale, rotation and opacity, spring easing. | Yes |
| `lower-third.json` | A reusable composition (plate, accent bar, springing dot) placed twice with different transforms. | Yes |
| `captions.json` | Word-by-word highlighted captions with outline and background box, plus Arabic, Hebrew and Thai text. Uses the system font "Liberation Sans". | Yes |
| `overlay.json` | Video clips with a crossfade, an image overlay, timed caption text, an audio bed. | Only with the media files it refers to, which are not included. |

Try:

```sh
geneva validate examples/overlay.json
geneva frame examples/shapes.json --at 1s -o shapes.png
geneva --format json validate examples/shapes.json
```
