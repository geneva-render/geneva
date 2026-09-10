# Examples

Each file is a complete timeline. All of them pass `geneva validate`.

| File | Shows | Renders with `geneva frame`? |
| --- | --- | --- |
| `solid.json` | The smallest useful document: one solid-color layer. | Yes |
| `shapes.json` | Shapes, keyframed position, scale, rotation and opacity, spring easing. | Yes |
| `lower-third.json` | A reusable composition (plate, accent bar, springing dot) placed twice with different transforms. | Yes |
| `overlay.json` | Video clips with a crossfade, an image overlay, timed caption text, an audio bed. | Not yet: it refers to media files that are not included, and video and text sources need the decoder and text layout that are still to come. |

Try:

```sh
geneva validate examples/overlay.json
geneva frame examples/shapes.json --at 1s -o shapes.png
geneva --format json validate examples/shapes.json
```
