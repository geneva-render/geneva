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

## Captions as a layer

`geneva subtitles --burn` is the whole job in one line, and the
[README](../README.md#captions) shows it. When captions are one part of
something larger they are a source like any other, and the same two kinds
of file go in — a transcript with word times, and a SubRip or WebVTT file
that times whole cues:

```json
"assets": {
  "iss":    { "src": "iss.mp4"    },
  "words":  { "src": "words.json" },
  "arabic": { "src": "ar.srt"     }
},

"layers": [
  { "id": "footage", "clips": [ { "source": { "kind": "video", "asset": "iss" } } ] },

  { "id": "captions", "clips": [ {
      "source": {
        "kind": "captions", "asset": "words", "margin": "15%",
        "style": {
          "font": "700 44px Liberation Sans", "color": "white",
          "highlight": { "color": "#ffd233" },
          "outline": "3px #000000cc", "max_width": "80%"
        } } } ] },

  { "id": "captions-ar", "clips": [ {
      "source": {
        "kind": "captions", "asset": "arabic",
        "style": { "font": "400 30px Liberation Sans", "color": "#cfd8e3",
                   "outline": "3px #000000cc", "max_width": "80%" } } } ] }
]
```

```sh
geneva render examples/captions.json -o captioned.mp4
```

<img src="../docs/captions.png" alt="A caption over the footage with the current word highlighted, and an Arabic line below it" width="560">

```
note[N453]: 2 cues read from "words.json"
  --> /layers/1/clips/0/source
note[N453]: 2 cues read from "ar.srt"
  --> /layers/2/clips/0/source
note[N600]: smart cut: 183 of 300 frames copied from the source, 117 encoded in 1 run around the cuts and overlays
wrote captioned.mp4 (300 frames, 10s of video, 2.3s elapsed)
```

One line in the document, one clip per cue on the timeline — which is why
183 of the 300 frames were still copied rather than composited. The cues
carry the timing, so there is nothing to write down.

The text is properly shaped, so the Arabic line reads right to left, and
Thai would break in the right places. You get wrapping, an outline, a
shadow and a rounded box, and the CSS spellings work here too:
`"700 44px Liberation Sans"` for the font, `"3px black"` for the outline.

### Where the cues sit

`position` is `bottom` (the default), `top` or `center`, and `margin` is
the distance from that edge — a length, or a percentage of the frame
height. A margin left out defaults to the title-safe inset (`safe`, 5% of
the height), so captions stay clear of the edges a television crops
without anyone choosing a number.

That is all that separates the two layers above. The English says
`"margin": "15%"`, which is 108px up from the bottom of a 720-tall frame;
the Arabic says nothing and takes the 36px default. Move either one by
changing its margin, or put it somewhere else entirely:

```json
"kind": "captions", "asset": "arabic", "position": "top", "margin": "8%"
```

If the file is WebVTT and places its own cues with `line:`, `position:`,
`align:` or `size:`, geneva follows it. `"follow_file": false` overrules
the file and places every cue the same way.

One thing to know: **caption layers do not know about each other.** Every
box is anchored at its own edge and grows away from it, so a cue that
wraps to a second line grows upward from a `bottom` margin. In the
example there is 44px between the two lines, and an Arabic cue long
enough to wrap twice would close that gap. Nothing warns about it yet —
give the layers room, or put them on opposite edges.

## What it refuses to guess

A caption file that geneva cannot make sense of is an error, not an empty
caption track. Times in a transcript are seconds, as whisper and WhisperX
write them; AssemblyAI and Deepgram write milliseconds, and those are
refused with the remedy rather than drawn as a caption that first appears
twenty minutes in:

```
error: reading the words in transcript.json: the times look like milliseconds,
not seconds: a word starts past 1000. Divide them by 1000; geneva reads
seconds, as whisper and whisperx write them
```

A file with no words in it anywhere is the same kind of error, and says
what it looked for. SubRip and WebVTT time whole cues rather than words,
so asking for a highlight on one is a warning rather than a silent
difference:

```
warning[W453]: en.srt has no word times, so nothing is picked out as it is said
  --> /layers/1
   = help: a highlight needs a word file; SubRip and WebVTT time whole cues
```

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
