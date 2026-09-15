# Examples

Every file here is a complete timeline. `iss.mp4` is ten seconds of NASA
footage, so these run as they are.

| File | What it shows | Run it |
| --- | --- | --- |
| `lower-third.json` | A name card written as HTML and CSS (`card.html`), sliding in on its own `@keyframes`. | `geneva render examples/lower-third.json -o dragon.mp4` |
| `card.html` | The card alone. Open it in a browser; it looks and moves the same. | |
| `words.json` | A whisper transcript, pasted in as it came out. | `geneva subtitles examples/iss.mp4 --burn examples/words.json --highlight "#ffd233" -o out.mp4` |
| `captions.json` | The same transcript as a layer, plus a second layer from a SubRip file. | `geneva render examples/captions.json -o captioned.mp4` |
| `social-reframe.json` | 16:9 fitted to 9:16 over a blurred copy of itself, captioned. | `geneva render examples/social-reframe.json -o reel.mp4` |
| `renditions.json` | Three sizes, a thumbnail, a sprite sheet and speech audio in one pass. | `geneva render examples/renditions.json -o out/` |
| `shapes.json` | Keyframed position, scale, rotation and opacity; eased and spring. | `geneva frame examples/shapes.json --at 1s -o out.png` |
| `overlay.json` | Crossfades, an image on top, timed captions, an audio bed. Needs your own media. | `geneva render examples/overlay.json --assets DIR -o out.mp4` |
| `solid.json` | The smallest document that does anything. | `geneva frame examples/solid.json -o out.png` |

`render` and `frame` open the media, so a clip with no `duration` takes
its length from the file. `validate` opens nothing unless you add
`--probe`, which is also what reads markup, its stylesheets and its
pictures.

## Captions

Speech recognisers write a word and the moment it was said.
`examples/words.json` is `whisper --output_format json`, pasted in as it
came:

```json
{ "id": 0, "seek": 0, "start": 1.0, "end": 2.6, "text": " Dragon is captured",
  "words": [
    { "word": " Dragon",   "start": 1.0, "end": 1.5, "probability": 0.981 },
    { "word": " is",       "start": 1.5, "end": 1.8, "probability": 0.914 },
    { "word": " captured", "start": 1.8, "end": 2.6, "probability": 0.972 }
  ] }
```

```sh
geneva subtitles examples/iss.mp4 --burn examples/words.json \
  --highlight "#ffd233" \
  --style '{ "font": "700 44px Liberation Sans", "outline": "3px #000000cc", "max_width": "80%" }' \
  -o captioned.mp4
```

<img src="../docs/captions.gif" alt="Captions over the footage, each word picked out in yellow as it is said" width="640">

geneva groups the words into cues the way a caption editor would — at
most two lines, about forty characters, nothing on screen for less than a
beat — and picks out whichever word is current.

whisper and its variants go in unedited; keys it does not need are
ignored. A `.srt` or `.vtt` works too, minus the highlight, which needs
word times. A file with no words in it, or one timed in milliseconds, is
an error rather than an empty caption track.

## Captions as a layer

The same two kinds of file are a source like any other, so captions can be
one layer of something larger:

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

One line in the document is one clip per cue on the timeline, which is why
183 of the 300 frames were still copied rather than composited. The text
is properly shaped, so the Arabic reads right to left and Thai would break
in the right places.

### Where the cues sit

`position` is `bottom` (default), `top` or `center`; `margin` is the
distance from that edge. A margin left out defaults to the title-safe
inset (`safe`, 5% of the height), which is the only difference between the
two layers above — the English says `"margin": "15%"`, the Arabic takes
the 36px default.

```json
"kind": "captions", "asset": "arabic", "position": "top", "margin": "8%"
```

WebVTT cues that place themselves with `line:`, `position:`, `align:` or
`size:` are followed; `"follow_file": false` overrules them.

Caption layers do not know about each other. Each box is anchored at its
own edge and grows away from it, so a cue that wraps grows upward from a
`bottom` margin. There is 44px between the two layers above, and an
Arabic cue long enough to wrap twice would close it. Nothing warns about
that yet.

## Vertical video

<img src="../docs/social-reframe.png" alt="The landscape clip on a tall canvas over a blurred copy of itself, with captions" width="240">

```sh
geneva render examples/social-reframe.json -o reel.mp4
```

Three layers fit a 16:9 clip into a 9:16 frame without cropping anything
out — the same file twice, then the captions over both:

```json
"output": { "width": 1080, "height": 1920, "fps": 30 },
"assets": { "iss": { "src": "iss.mp4" }, "words": { "src": "words.json" } },

"layers": [
  { "id": "backdrop", "clips": [ {
      "source": { "kind": "video", "asset": "iss", "audio": false },
      "fit": "cover",
      "effects": [ { "kind": "blur", "radius": 45 } ] } ] },

  { "id": "picture", "clips": [ {
      "source": { "kind": "video", "asset": "iss" },
      "fit": "contain" } ] },

  { "id": "captions", "clips": [ {
      "source": {
        "kind": "captions", "asset": "words", "margin": "28%",
        "style": { "font": "700 72px/1.25 Liberation Sans", "color": "white",
                   "highlight": { "color": "#ffd233" },
                   "outline": "4px #000000cc", "max_width": "80%" } } } ] }
]
```

The captions are the same `words.json`; only the margin changed, to sit
them under the picture band. For the effect without a document:

```sh
geneva convert talk.mp4 -o reel.mp4 --for tiktok --fill blur
```

## One read, many files

Everything a web page needs: two or three sizes, a thumbnail, a sprite
sheet for the scrub preview, and the audio at 16 kHz to feed whisper.
Listed in one document, the source is read once instead of six times.
This is `outputs`, not a `--for` preset — the presets pick the settings
for one destination, this writes many files from one pass.

```json
"outputs": {
  "720p":    { "kind": "video" },
  "480p":    { "kind": "video", "height": 480, "encode": { "video": { "crf": 23 } } },
  "360p":    { "kind": "video", "height": 360, "encode": { "video": { "crf": 25 } } },
  "poster":  { "kind": "poster", "width": 960 },
  "preview": { "kind": "sprites", "every": "2s", "columns": 5 },
  "speech":  { "kind": "audio", "path": "speech.wav", "audio": { "sample_rate": 16000, "channels": 1 } }
}
```

```sh
geneva render examples/renditions.json -o out/
```

The thumbnail is chosen, not grabbed: past the opening, first frame that
is neither dark nor still. The sprite sheet comes with the WebVTT file
players expect.

## Cuts that don't re-encode

A trim or a join that leaves the picture alone copies the packets, which
takes about as long as reading the file. You don't have to know when
that's safe, or remember `-c copy`.

```
$ geneva trim talk.mp4 -o cut.mp4 --from 0.5s --to 1.5s
note[N600]: the video stream is used as is, so it is copied without re-encoding
note[N600]: cut at 0.5s moved to the keyframe at 0.48s
wrote cut.mp4 (1s, streams copied without re-encoding, 0.0s elapsed)
```

When it can't copy it says why rather than quietly re-encoding.

## Writing a card

`card.html` is ordinary markup. Paths in it are relative to itself, as on
a page, so `<img src="logo.png">` finds the file next to it — and opening
it in a browser shows what geneva will draw. Nothing may leave the asset
root: no leading `/`, no `..`, no URLs.

It is not a browser. An element's text is one paragraph and a child
element is a box; there is no `float` or `z-index`; anything geneva
cannot draw it names rather than skipping. The property list is in
[../docs/timeline.md](../docs/timeline.md#markup).

## Where the footage came from

`iss.mp4` is a ten-second excerpt of "Earth Views from the International
Space Station", from NASA's public library. NASA material is generally not
subject to copyright. The excerpt was trimmed and re-encoded for size; the
original is at
<https://images.nasa.gov/details/Earth%20Views%20from%20the%20International%20Space%20Station>.
