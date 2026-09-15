# Examples

Each file is a complete timeline. `iss.mp4` is ten seconds of footage from
the International Space Station, so the examples that need video run as
they are without you supplying any.

| File | What it shows | Run it |
| --- | --- | --- |
| `lower-third.json` | A name card written as HTML and CSS (`card.html`), sliding in on its own `@keyframes` over real footage. The document says only when the card is on screen. Only the seconds under it get re-encoded. | `geneva render examples/lower-third.json -o dragon.mp4` |
| `card.html` | The card on its own: where it sits, how it looks and how it arrives, with no pixel coordinates in it. Open it in a browser; it looks and moves the same there. | |
| `words.json` | A whisper transcript, pasted in as it came out: a word and the moment it was said. Nothing in it was edited for geneva. | `geneva subtitles examples/iss.mp4 --burn examples/words.json --highlight "#ffd233" -o captioned.mp4` |
| `captions.json` | The same transcript as a layer, with the current word picked out, and a second caption layer from a SubRip file (`ar.srt`) to show the text is properly shaped. | `geneva render examples/captions.json -o captioned.mp4` |
| `social-reframe.json` | A 16:9 clip fitted to a 9:16 frame over a blurred copy of itself, with the same `words.json` captions under the picture band. | `geneva render examples/social-reframe.json -o reel.mp4` |
| `renditions.json` | Three sizes, a thumbnail, a sprite sheet and 16 kHz speech audio, from one pass over the source. | `geneva render examples/renditions.json -o out/` |
| `shapes.json` | Keyframed position, scale, rotation and opacity, with eased and spring interpolation. | `geneva frame examples/shapes.json --at 1s -o out.png` |
| `overlay.json` | Crossfades between clips, an image on top, timed captions, an audio bed. Needs media of your own. | `geneva render examples/overlay.json --assets DIR -o out.mp4` |
| `solid.json` | The smallest document that does anything. | `geneva frame examples/solid.json -o out.png` |

`render` and `frame` open the media to find out how long it is, so a clip
with no `duration` takes its length from the file. Plain `geneva validate`
doesn't open anything, so add `--probe` when a document leaves lengths to
the media, or when it draws markup: `--probe` is what reads an `html`
asset, the stylesheets it links to and the pictures it points at, so a
missing one is an error rather than a gap in the picture.

## Writing a card

A file like `card.html` is ordinary markup. Paths in it are relative to
itself, as they are on a page, so `<img src="logo.png">` and
`<link rel="stylesheet" href="house.css">` find the files next to it —
and open the file in a browser and you see what geneva will draw. The
one rule is the one every asset path follows: nothing outside the asset
root, so no leading `/`, no `..` and no URLs.

What it is not is a browser. An element's text is one paragraph and a
child element is a box, there is no `float` or `z-index`, and anything
geneva cannot draw it names rather than skipping quietly. The property
list is in [../docs/timeline.md](../docs/timeline.md#markup).

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
