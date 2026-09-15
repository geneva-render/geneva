# Examples

Each file is a complete timeline. `iss.mp4` is ten seconds of footage from
the International Space Station, so the examples that need video run as
they are without you supplying any.

| File | What it shows | Run it |
| --- | --- | --- |
| `lower-third.json` | A name card written as HTML and CSS (`card.html`), sliding in on its own `@keyframes` over real footage. The document says only when the card is on screen. Only the seconds under it get re-encoded. | `geneva render examples/lower-third.json -o dragon.mp4` |
| `card.html` | The card on its own: where it sits, how it looks and how it arrives, with no pixel coordinates in it. Open it in a browser; it looks and moves the same there. | |
| `captions.json` | Word-by-word captions with the current word picked out, and a second line in Arabic to show the text is properly shaped. | `geneva render examples/captions.json -o captioned.mp4` |
| `social-reframe.json` | A 16:9 clip fitted to a 9:16 frame over a blurred copy of itself, with captions. | `geneva render examples/social-reframe.json -o reel.mp4` |
| `renditions.json` | Three sizes, a thumbnail, a sprite sheet and 16 kHz speech audio, from one pass over the source. | `geneva render examples/renditions.json -o out/` |
| `shapes.json` | Keyframed position, scale, rotation and opacity, with eased and spring interpolation. | `geneva frame examples/shapes.json --at 1s -o out.png` |
| `overlay.json` | Crossfades between clips, an image on top, timed captions, an audio bed. Needs media of your own. | `geneva render examples/overlay.json --assets DIR -o out.mp4` |
| `solid.json` | The smallest document that does anything. | `geneva frame examples/solid.json -o out.png` |

`render` and `frame` open the media to find out how long it is, so a clip
with no `duration` takes its length from the file. Plain `geneva validate`
doesn't open anything, so add `--probe` when a document leaves lengths to
the media.

```sh
geneva validate examples/renditions.json --probe
geneva frame examples/lower-third.json --at 3.5s -o check.png
```

## Where the footage came from

`iss.mp4` is a ten-second excerpt of "Earth Views from the International
Space Station", from NASA's public image and video library. NASA material
is generally not subject to copyright. The excerpt was trimmed and
re-encoded for size; the original is at
<https://images.nasa.gov/details/Earth%20Views%20from%20the%20International%20Space%20Station>.
