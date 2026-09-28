# Changelog

All notable changes to Geneva are recorded here. The project follows
semantic versioning. The `geneva` field in every document names the format
version it was written for; within 1.x the format only gains optional
fields, so a document keeps meaning what it meant. Before 1.0, minor
versions changed the format freely.

## 1.1.0 (2026-09-28)

Timeline format 1.1: a `"1.0"` document is read as it is.

### Added

- **Captions from part of a file.** The `captions` source takes `in` and
  `out`, as a video clip does: cues outside the range are left out, one
  crossing an edge is cut, and `in` becomes the clip's start. Captions
  for three shots cut from one film are three captions clips on the same
  file with the shots' own `in`, `out` and `start`. Before, a word file
  or a subtitle file could only be used whole from its first second, and
  two agents building a social clip and a news package from the middle
  of a film both had to rewrite the file's times by hand.
- **Several posters in one document.** Each `poster` entry in `outputs`
  is written, at its own `at`. Before, only the last one by name was
  written and the others were skipped without a word. A second `sprites`
  entry is now E434, since a document writes one sheet.

### Changed

- **A pause ends a caption.** Words from a speech recogniser were
  grouped by segment and by length only, so a cue could hold the words
  on both sides of a six-second silence and show the second half
  seconds before it was said. A pause of a second or more between two
  words now starts a new cue.
- **`geneva frame --at` draws the frame a render shows at that time**,
  from the time it starts, as the render draws it. It drew the exact
  time asked for, which between two frames is a picture the video never
  holds.
- **An audio clip's fades longer than the clip are W306**, not W304,
  which kept only its other meaning: a `color` on a crossfade. Every
  code now has one meaning.

### Fixed

- **A `<div>` with paragraphs in it set them side by side.** An element
  that set no `display` was a flex row, taffy's default, rather than a
  block, CSS's, so `<div><p>A</p><p>B</p></div>` drew "AB" on one line
  and a heading after them ran off the edge instead of wrapping. An
  element with no `display` that holds block elements is now a block
  and they stack. One that holds only text and inline elements keeps
  the row, which is how a line of spans stays a line. Markup that sets
  `display` is laid out as before; every golden case and benchmark
  piece renders the same to the bit.
- **Subtitle streams ran past the end of the video.** A cue after the
  output's end was kept, so a 3 s video with a subtitle file offset
  into it reported itself as 20 s long. Cues are now cut at the end.
  A subtitle `title` was dropped in MP4 and MOV, which have no stream
  title; it is now written as the stream's handler name, which is what
  players there show.
- **An `outputs` entry with both `width` and `height` in another shape**
  stretched the whole picture into it without a word. It still does,
  with W406, and the reference says a different framing needs its own
  document.
- **Diagnostics pointed at the wrong place or said the same thing
  twice.** An error in an `@keyframes` inside markup pointed at a JSON
  field named after the CSS offset (`/source/from`); it now points at
  the source and names the rule and the offset in the message. A
  captions clip with a negative `start` gave one E300 per cue, with
  times nobody wrote; it gives one. `geneva explain W201` called a note
  a warning.

- **A word in bold inside a sentence broke the sentence.** In markup,
  `<p>Go for <b>launch</b> at nine</p>` was laid out as three boxes in a
  row: the paragraph stopped wrapping and ran off the edge of its box,
  the space after the bold word was lost, and nothing said so. Text with
  inline elements inside it (`b`, `strong`, `i`, `em`, `span`, `small`,
  `code`, `a` and the other inline tags) is now set as one run, as a
  browser sets it: it wraps as a whole, spaces are collapsed across the
  pieces, each piece keeps its own colour, weight, style, size, family
  and letter-spacing, a line is as tall as its tallest piece, and `<br>`
  breaks the line. An inline element that
  needs a box of its own (a background, a border, padding) still gets
  one, and an element that asks for `display: flex` still makes its text
  and elements flex items, as a browser does.
- **Full-screen motion graphics in markup render two to four times
  faster.** A benchmark against Remotion found geneva slower on pure
  markup pieces, and a profile found work that changed nothing in the
  picture. A panel waiting to wipe in, its `clip-path` still a line, was
  painted over the whole frame every frame and then masked away; it is
  now bounded to its polygon and skipped while the polygon covers
  nothing. Everything under the last box that covers the frame in an
  opaque colour is no longer painted. A group laid down exactly as
  painted (full opacity, no blur, blend or transform, clips that cover
  it) no longer gets a buffer of its own. A soft shape under a steady
  `filter: blur()` is blurred once and kept, where it was blurred again
  every frame. Inside a box, pixels well within its corners and its
  clip skip the rounded-rectangle test, `screen` and `multiply` no
  longer divide by alpha, and text is encoded through a table rather
  than a power per pixel. Two faults in the cache of group pictures went
  with it: a group whose boxes changed every frame was repainted over its
  whole reach every frame instead of being given up on, and a picture no
  longer shown was never let go, so it could keep a new one out of the
  budget. On a 4-vCPU Xeon VM (2.1 GHz, no GPU) a 16 s kinetic typography piece at 1080p
  takes 14.3 s where it took 59.3, and a 14 s intro with blurred shapes
  in `screen` 29.9 s where it took 74.3. The picture is the same to one
  level in 8 bits, except that a glyph running past the edge of a group
  that now has no buffer is drawn whole, as a browser draws it, rather
  than cut at the buffer's edge.
- **A keyframe's `letter-spacing` in `em` failed the render** with `E442,
  invalid length ".3em"`, although the same length works in a static
  rule and in a browser: keyframes were parsed without the element, so
  only pixels could be read. An `em` is now kept as written and resolved
  against the element's own font size when the animation is played on
  it. `normal` reads as `0`; `rem` and percentages are still refused.
  Found by a kinetic typography piece whose word collapses from `.6em`
  to `-.02em`.
- **An animation on markup's outermost element with only a `to` (or only
  a `from`) keyframe was dropped** with `W440, nothing interpolates`.
  CSS starts such a rule from the element's own value, so
  `@keyframes out { to { opacity: 0 } }` fades out in a browser; geneva
  played the outermost element's animation on the clip and found one
  keyframe with nothing to move between. A missing first or last keyframe
  now takes the clip's own value: no movement, no scaling, no turning,
  and the clip's opacity. The same holds for a clip's `animation` in the
  document. Found by the intro card of a benchmark package, whose
  `card-out` fade never ran.
- **Overlays over a video cost less when they are far apart.** With a
  card at the top of the frame and captions at the bottom, the two were
  drawn onto one transparent frame spanning both, two thirds of a 720p
  picture, and every block of it was visited and a small list allocated
  for each before the transparent ones were skipped. Clips whose boxes
  do not touch are now drawn and laid on separately, blocks are skipped
  without allocating, and word-timed captions are drawn again only when
  the lit word changes. The output is the same to the bit. On
  `examples/lower-third.json` (720p, 9 s, 4 cores) the render takes
  5.9 s where it took 6.2; ffmpeg, drawing the same card and an ASS
  version of the captions, takes 5.5.

## 1.0.0 (2026-09-25)

The first public release.

### Added

- **Windows.** A `x86_64-pc-windows-gnu` build, cross-compiled on Linux
  with MinGW-w64: `MEDIA_HOST=x86_64-w64-mingw32
  scripts/build-media-libs.sh` builds the media libraries for it (zlib
  included, which Windows lacks), and the release ships it as a zip with
  `install.ps1`, which installs for the current user and puts geneva on
  the PATH. Before anything is published the release workflow installs
  that zip on a Windows runner and runs `scripts/smoke.sh`, every
  everyday command once. `geneva.exe` needs nothing but Windows' own
  DLLs.
- **Media Foundation** for H.264 and H.265 on Windows, when a GPU
  encoder is behind it, and NVENC there as on Linux. Media Foundation is
  8-bit only; ten bits go to the next encoder. Without a GPU encoder
  H.265 fails as it does elsewhere, with the reason. Untested on real
  hardware so far: the Windows runner has no GPU.
- The system's x264 on Windows: `libx264-<build>.dll` next to
  `geneva.exe`, or the file `GENEVA_X264` names. A bare name is looked
  for next to the executable and in the system folder only, never in the
  current directory or along `PATH`.

- **`max_chars` on the `captions` source.** Cue grouping is a budget of
  `max_lines` lines of `max_chars` characters, and the character count
  was fixed at 42 with no way to reach it from a document, so `max_lines`
  was the only lever: the same 724 words grouped into the same 55 cues
  at `style.max_width` 700 and at 1900. It is a field now, defaulting to
  the 42 it always was. Under 8 is `E402` rather than a quiet clamp,
  since a line that short cannot hold a word.

  Grouping still counts characters rather than measuring, because it runs
  before a font is loaded, and settling cues without one is what keeps a
  document grouping the same way on every machine. `style.max_width` is
  therefore still yours to match: `max_width / (0.5 * font.size)` errs
  narrow, and the reference gives the measured figures.

- **The cue count says what grouped it.** `N453` read `55 cues read from
  "captions.json"`, which does not say what would change it. It now adds
  `grouped at up to 2 lines of 42 characters`, and says nothing extra for
  a `.srt` or `.vtt`, which carries its own cues and is not grouped.

- **Every field measured in pixels takes `"8px"` as well as `8`.** Only
  `padding` did, so a text style could say `"padding": "14px"` and then
  be refused `"radius": "3px"` one line below with `E103`, which is the
  first thing someone writing CSS-shaped values will try. The rest now
  read either spelling: a text `size`, `letter_spacing` and `radius`, an
  outline's `width`, a shadow's `x`, `y` and `blur`, a fill's `width`,
  `height`, `x` and `y`, a shape's and a mask's `radius`, a mask's
  `feather`, a blur effect's `radius`, and the frame and picture sizes
  under `output`, `compositions` and `outputs`. Keyframe values of these
  fields take either spelling too.

  A percentage is refused, with the reason, since none of these fields
  has anything for it to be a percentage of; the fields that do take
  one are lengths and already did. A frame or picture size must be a
  whole number of pixels. Nothing that parsed before parses differently,
  and a printed timeline still writes the number.

- **Rate and audio flags on every verb**: `--max-bitrate`,
  `--audio-codec`, `--audio-bitrate`, `--sample-rate` and `--channels`,
  for the fields the format already had and the command line did not
  reach. `geneva convert talk.mov -o talk.mkv --max-bitrate 6M
  --audio-codec opus --audio-bitrate 128k` is now one command rather
  than `--show-timeline`, an edit and a `render`. Rates take `6M`,
  `800k` or a bare number of kb/s; a number over 1 Gb/s is refused with
  the reason, since ffmpeg reads a bare number as bits per second. A
  flag wins over what a `--for` target writes.

  The sound is copied when what is asked matches the source (codec,
  rate, channels) and encoded otherwise, with the picture still copied
  beside it. There is still no fixed average bitrate, constant bitrate
  or two-pass encode: the video is constant quality under an optional
  ceiling, and the CLI reference now has a section saying so, with the
  ceiling's two-second buffer and what each encoder does with it.

### Changed

- **Timeline format 1.0.** The first public release starts the format
  clean: every document declares `"geneva": "1.0"`, which is the only
  version this binary reads, and `schema/geneva-timeline-1.0.schema.json`
  is the only schema file. The 0.x formats were only ever read by the
  author's own documents. A document declaring one of them is E110,
  whose help says what to do: set `"geneva"` to `"1.0"`, and nothing
  else needs changing except that `output.audio.denoise` is gone (see
  Removed). What 1.0 reads is what 0.5 did: everything from 0.1 to 0.4,
  plus `max_chars` on the captions source and `"8px"` in every field
  measured in pixels.

  From here the published schema is a promise: a 1.x version adds only
  optional fields, so a document written for an earlier 1.x is read as
  it is, and anything that would change what a written document means
  is a 2.0.

- **Video drawn small converts only what it shows.** On the CPU
  renderer a video drawn smaller than it is is made smaller while it is
  still Y'CbCr (libswscale), to its drawn size (rounded up to a
  sixteenth of its own while its scale is animated, so a zoom goes
  through a few sizes rather than one per frame), and the rest of the reduction is done in linear light as before. Where
  that would leave out less than a quarter of the pixels the frame is
  converted whole: the resize then costs more than it saves. 10 s at
  1080p, 4 cores: a 2x2 grid of 4K clips in 24 s where it took 49; 4K
  footage with a picture-in-picture over it in 26 s (47); 1440p footage
  in 17 s (23); a 4K clip zooming out from 0.9 to 0.3 of its size in
  27 s (30). The price is a fraction of a code value on average, since
  averaging encoded values dims fine bright detail a little; a clip with
  a mask is converted whole. An asset source opts in through `AssetSource::video_size` and
  `video_frame_shrunk`, so the goldens and the GPU renderer are
  unchanged.

### Removed

- **Speech denoising is gone**: `--denoise`, `output.audio.denoise`,
  `E424`, the `denoise` cargo feature and the DeepFilterNet dependency,
  along with the two files that existed only to serve it. Loudness and
  hygiene are untouched.

  It was in no released binary and could not be, because the licence of
  the model weights was never settled, so the only feature the manual
  documented in full was the only one nobody could run: `--denoise`
  returned `E424` telling the reader to build their own binary. It was
  also the one git dependency in the workspace and about three minutes
  of first-build time for everyone working on the engine.

  `output.audio.denoise` was in the published `0.4` schema, so a
  document that still sets it is now `E101`, an unknown field, rather
  than being accepted and ignored. No document in the wild can have
  relied on it working, since no released binary ever did it. A test
  pins the refusal.

  If it returns it will be a fresh decision, and the way to have it
  without the licence question is to leave the weights out of the binary
  and fetch them on first use, which upstream supports.

### Fixed

- A time with a unit spelled out, `"4 seconds"` or `"2 min"`, failed with
  `invalid number "4 second"`, which points at the number rather than
  the unit. It now names the unit and the form to write: `unknown unit
  "seconds" in "4 seconds"; write "4s"` (`"2 minutes"`: `"120s"`).
- **Two clips of one video decoded every frame twice over.** When a
  source's frames sit a little after the output's frame times (8 ms, in
  a 23.976 fps file), the second request for the same time, which the
  blur fill's backdrop and picture make every frame, took the frame as
  one it had passed, seeked back to the keyframe and decoded forward
  again. It now keeps the frame it already has. The pictures are the
  same to the bit; a 1080p blur fill with markup over it renders in 30 s
  where it took 64.
- The CPU compositor lays a wide blur's small layer back onto the frame
  a row at a time, working out each column's texels once rather than
  per pixel, and an unrotated enlarged picture reuses its columns on
  every row. Same pixels, less work.
- **Text on a machine with no fonts panicked** ("no default font
  found"), in a bare container as much as in a fresh Wine prefix. Such a
  machine now draws in Liberation Sans, built in for that case only
  (SIL OFL 1.1, `licenses/OFL-1.1-Liberation.txt`).
- Windows: verbs refused inputs on two drives ("inputs must be on the
  same drive"), a video on `D:` with a logo on `C:`, say. A verb now
  writes such paths whole and the resolver takes them from it, and from
  nothing else: a document someone wrote still keeps its paths under its
  asset root.
- Windows: verbs with inputs in different folders joined `/`-separated
  paths onto a `\\?\C:\` root from `canonicalize`, in which `/` is not a
  separator; the root is now written in the ordinary form. An image
  sequence left a zero-byte file named after its pattern, since the
  file was still open when it was removed. A Windows checkout no longer
  turns the schema files' line endings to CRLF (`.gitattributes`), which
  failed the schema test.
- `missing_libraries_are_named_with_their_packages` failed on macOS and
  Windows, and CI's schema check compared against the 0.4 file.
- The bundled OpenH264 ignores `--max-bitrate` (it runs at a pinned
  quantizer) and now says so in its note, as VideoToolbox and NVENC do.
  It is what encodes H.264 wherever x264 is not installed, Windows
  included.
- **A `text` source drew in the machine's font, not the one the document
  ships.** Font assets were registered only when a source named one by
  its asset id. Name the family the file declares, which the reference
  offers as an equal spelling, and nothing was registered, so the code
  that drops the machine's faces of a shipped family never ran and the
  machine's copy drew instead. Shipping only a Bold file and asking for
  that family at weight 400 gave the machine's Regular. Markup never had
  this: it registered every font asset already. A document that carries
  its fonts is now the same picture on every machine whichever spelling
  it uses, which is the whole point of carrying them.

  Nothing reported it, and nothing could: `W405` fires when the family is
  missing, and the family was there, just not the copy the document
  brought. Worth re-rendering anything that names a shipped family rather
  than an asset id, since its output may have been the machine's font all
  along.

- **A markup group far from the frame stopped being drawn.** A group's
  buffer was bounded to a rectangle two frames wide and two tall, in the
  element's own coordinates. An element translated past that edge, which
  is any news crawl a few seconds in, had its buffer collapse to nothing:
  no error, no warning, a blank strip in the middle of a render. The
  bound is now the area that rectangle had, nine frames' worth, with the
  shape left free, so an element far wider than the frame is drawn whole
  and stays drawn however far it travels. Measured on a crawl in a
  1280x120 frame: at 20s its group sits at x 6000 to 7280, well past the
  old edge, and its buffer is 30,720 pixels against a budget of
  1,382,400.

- **`W455` when a group really is too big for its buffer.** The bound
  still exists, and something cut off by it is no longer cut off in
  silence. This needed a way for the renderer to say anything at all:
  render-time remarks came from the planner, never from the painter, and
  a wrapper in the middle would have swallowed them.

- **A ceiling on VP9 did not encode.** libvpx refuses `maxrate` without
  a bitrate, so any document setting `max_bitrate_kbps` for VP9 failed
  to open the encoder. The ceiling is now the bitrate of libvpx's
  constrained quality mode: 853 kb/s for a cap of 800 on a clip that
  runs at 1788 uncapped.
- **Settings a copy cannot carry were dropped by copying.** A document
  or verb setting only `max_bitrate_kbps`, `bitrate_kbps` or
  `keyframe_interval` was copied as it stood (a 500 kb/s ceiling came
  out at 1496), and `audio --extract --speech` into `.m4a` copied the
  48 kHz stereo AAC instead of writing 16 kHz mono. Each now re-encodes
  what it changes. The CLI reference already said `--keyframe-interval`
  forced a re-encode.
- **NVENC's ignored ceiling is reported**, as VideoToolbox's already
  was: at a constant quantizer it does not apply one.

- **A `captions` clip's `start` placed its cues.** It was discarded: every
  cue landed at the time written in the caption file, whatever the clip
  said, so captions over a video that starts anywhere but zero came up
  early by exactly the clip's start. The documented rule (timing rule 6,
  every time is relative to the clip) now holds for cues as it does for
  keyframes and word times, and a test pins it. Anyone who compensated by
  shifting the times in the file should take that shift back out.

- **Markup style diagnostics name the property.** `W450` gave the value
  that was refused and the element, but not which declaration it came
  from: `<div>: "50%" is not a length` for a `border-radius: 50%`, with
  nothing to say it was the radius. It now reads `<div>: border-radius
  "50%" is not a length`. The property is added once where the message
  reaches the reader, so every one of them carries it.

- **`geneva guide` carries `architecture.md`.** `timeline.md` and
  `cli.md` both link to it and it was not embedded, so from the binary
  those were dead ends. It is the sixth page, and a test now fails if a
  carried page links to one that is not carried.

### Documentation

- A shipped family is the one drawn wherever it is named, by asset id or
  by the family in the file, in markup and in a `text` source alike.
- `transform.position` is one animated point, so its keyframes carry both
  axes together; a `keyframes` object under `x` is `E103`, which reports
  the length it expected rather than the point it wanted.
- The group buffer bound, what it allows, and that a composition has none.
- `examples/words.json` was described as whisper's output pasted in as
  it came. `iss.mp4` has no sound, so it cannot be: the words were
  written for the clip in whisper's format. The README and the examples
  page now say so.
- The README's lower third has captions of its own,
  `examples/commentary.json`: three sentences where it had seven words
  in two cues. The render ends 3 s after the card leaves, at 9 s, and
  its report and frame counts in the README are the ones it prints now.
  `scripts/demo-gif.sh` writes the WebP figures the pages use rather
  than GIFs nothing did.
- The reference no longer mentions denoising in three places it
  outlived the feature.
- `cli.md` is a reference now: tables per command, the output modes in
  one table. `architecture.md` is shorter and more technical.
  `timeline.md` ends with a short list of known limitations in place of
  "What it does not do", and corrects what it said about hardware
  encoders (used, not ignored), un-probed clip lengths, the renderers,
  colour spellings and two missing fields (`subtitles`,
  `transition_out`).

Found by pointing an agent at the 0.7.1 binary with `geneva guide` as its
only source and watching where it went wrong.

- `animation-fill-mode` was described once, in the clip section, in terms
  that read as though they covered markup too. A clip's animation holds
  its first and last value and has no `fill-mode` field; an animation on
  an element inside markup is ordinary CSS, defaults to `none`, and takes
  `animation-fill-mode` like a browser. The two are now told apart where
  the claim is made. This was the costliest gap: staged entrances were
  all visible at frame 0 and elements snapped back when their animation
  ended.
- Cue grouping measures in characters, not pixels: `max_lines` lines of 42
  characters, decided before any of `style` applies, so `style.max_width`
  changes no cue boundary. A cue grouped for 42 characters can draw on
  three lines in a narrow `max_width` with no warning.
- The markup subset has no static `transform`. A transform comes only from
  an `animation`; the table had no row for it, which read as an omission
  rather than a rule.
- Percentages are taken on widths, heights, margins, padding, gaps and
  insets, and nowhere else, so `border-radius: 50%` is `W450`. The list
  was there but not marked as exhaustive.
- The `assets` `kind` list was missing `captions` and `html`.
- `N453` needs `validate --probe`, since counting cues means reading the
  file.

- **`scripts/check-markup.sh` renders every markup golden case.** The
  loop named `markup-opening` and `markup-card`, so `markup-blend`,
  added for `mix-blend-mode`, was not checked on a second machine. It
  now takes every `markup-*` case under `tests/golden`, which is nine
  frames rather than seven, and a case added later needs no edit here.
  The script is not in the release tarball, so a checkout of `main` has
  the fix without waiting for a release.

## 0.7.1 (2026-09-22)

### Added

- **The manual is in the binary.** `geneva guide` prints the guide for
  programs and agents, `geneva guide <topic>` one of `timeline`, `cli`,
  `errors` and `color`, and `geneva guide --list` names them. The pages
  are the `docs/` files, embedded at compile time, so the manual and
  the binary are one artifact and cannot describe another version. A
  machine that installed the release tarball has the binary and not
  `docs/`, and nothing in `--help` used to say where the format was
  written down.

  `geneva explain E302` answers the other half: every diagnostic
  carries a stable code, so a caller that hit one has a command to run
  rather than a search to make. It reads the tables in `docs/errors.md`,
  takes a code in any case, prints every meaning where a code has more
  than one (`W304` has two), and exits 2 for a code that does not
  exist. `--list` prints them all. Under `--format json` both commands
  return one document, as every other command does.

  A test walks every `src` directory in the workspace for codes the
  engine can emit and fails if one is missing from `docs/errors.md`, so
  the two cannot drift apart.

## 0.7.0 (2026-09-21)

### Added

- **W404 names a clip whose placement is probably not what was meant.**
  A `fit` is only needed when the source and the frame are different
  shapes, and the defaults suit what each source is usually for: a video
  is shown whole, and a picture, a text box or a markup box is drawn at
  the size it has. Two cases make that surprising, and both are now
  said out loud with the sizes and the share: a picture bigger than the
  frame, drawn at its own size so the frame cuts its edges (a 4000x3000
  photo on a 1920x1080 frame shows a ninth of itself), and a video shown
  whole in under two thirds of the frame (16:9 on a 9:16 canvas covers
  32%). A clip carrying a `crop`, a `scale` or an `animation` is left
  alone, since each is already a decision about size. The check needs
  the source's own size, so it appears wherever the assets are read:
  `render`, `frame`, the verbs, and `validate --probe`.

### Changed

- **A `--for` target that builds a portrait canvas fills with a blurred
  copy of the picture.** A landscape video on a 9:16 canvas was
  letterboxed unless `--fill blur` asked for better, which is not what
  anyone posts. The blurred, scaled-up copy behind the whole picture is
  what phone editors do and what this does now; `--fill bars` asks for
  the background colour, and `--fit cover` still crops instead. The copy
  decodes the source a second time, so such a render costs about twice
  what bars would. A `--width`/`--height` resize is unchanged: sizes
  given by hand are a request for those sizes, not for a second pass
  over the source.

- **The claim that the two renderers agree to within one 8-bit code now
  says where it does not.** 0.6.0 measured that on Mesa's lavapipe,
  where it holds for all eleven golden cases. On an Apple M1 the blur
  case differs by up to three codes on 0.13% of its pixels, since a blur
  is six passes over a downscaled buffer and the two paths round
  differently along the way; the case's tolerance holds with room, and
  every other case is within one code there too. The CLI reference,
  `architecture.md` and `color.md` say so, and `color.md` now also
  records what the device composites in: `Rgba16Float`, against the CPU
  renderer's 32-bit float, with the blur layers 32-bit on both.

- **A `clip-path` polygon's coverage is 2.3 times quicker.** Two
  things: the pixels a span covers whole take the same share whatever
  their position, so only the pixel at each end of a span is worked out
  now rather than every one of the thousands between; and the rows went
  to the threads one at a time, which costs rayon more to hand out than
  the row costs to do, so they go in jobs of about 32k pixels like every
  other loop in the painter. Measured warm on the markup goldens: 2.24 ms
  a call before, 0.85 ms after. It is 1% to 3% of a markup frame either
  way, so this is not why a markup frame costs what it does.

- **A `clip-path` sliver starting on a pixel boundary is drawn again.**
  The first version of the change above lost a span that began exactly on
  a pixel's edge and ended inside that same pixel: it had no partial
  pixel at its start, and the branch for the one at its end declined to
  run. A polygon reaching past the frame is clamped to exactly 0.0
  there, so the frame's own edge is where it showed, at two pixels of
  one `markup-opening` frame. Inside the golden tolerance, which is why
  the suite stayed green and a run on a second machine is what printed
  it. The two ends of a span are now worked out independently, a
  differential test against the old arithmetic agrees on 400,000 random
  spans, and a unit test pins the case.

- **A GPU that Vulkan cannot see is explained, not just reported
  missing.** A container built for CUDA, which is what a rented GPU
  usually comes with, carries the NVIDIA driver's compute half and not
  its graphics half, so `nvidia-smi` works and Vulkan finds nothing. The
  driver there is a GLVND vendor library that needs the X11 client
  libraries to load and the GLVND dispatch libraries to start, and
  missing either produces the same unhelpful loader message. The probe
  now says which of them is absent and names the packages, and a render
  that lands on a software device when one was not asked for says so
  instead of being quietly slower than the CPU renderer. NVENC is
  unaffected, since it goes through the compute half.

- **A hardware encoder that will not open says so, on a machine that
  has the device.** Under the default `auto` policy geneva tries
  VideoToolbox or NVENC and falls back to software without a word. That
  is right on a machine with no such device and wrong on one that has
  it: the run is slower than the machine can go and nothing says why.
  The note now names the encoder, the driver's reason and what ran
  instead, and only where the device looks present, so a machine without
  a GPU stays quiet. Measured case: in a container built for CUDA,
  NVENC reports "no capable devices found" for any program, stock ffmpeg
  included, and geneva encoded with x264 without mentioning it.

- **The everyday verbs take `--renderer` too.** It was on `render`
  alone, so a `convert --for tiktok`, which builds a canvas and
  composites every frame, had no way to ask for the GPU. Measured on an
  RTX 4090, that job is about 1.6x faster on the device at 1080p. Only
  the steps that composite are affected: a copied stream is copied
  either way.

- **The blur's difference between the renderers is stated plainly.**
  The docs hedged that "a few pixels differ by up to three on some
  devices". It is 71 pixels, 0.12% of the blur case, and it is the
  blur's own rounding rather than any one device: an Apple M1 through
  Metal and an RTX 4090 through Vulkan report the same 71 pixels.

- **A font family the machine does not have is reported (W405).** A
  document naming `Inter` on a machine without it still renders, in
  whatever the shaper falls back to, so the only sign is that the
  picture differs elsewhere. The warning names the family, says the text
  will be drawn in something else, and suggests the fix: carry the file
  as an asset of kind `font` and name that asset id, which keeps the
  document the same everywhere. A family that one of the document's own
  font assets declares counts as present, so carrying the file is enough
  to silence it, which is what the goldens already do. Markup is covered
  too: a `font-family` in its CSS is checked the same way, which is where
  most families are named, since a card written for a browser asks for
  its font by name. `validate` reports a text source's family without
  opening any asset file, since the fonts are on the machine either way;
  a markup family needs the markup read, so that one wants `--probe`.

- **`mix-blend-mode` in markup.** A box with one is composited into a
  buffer of its own, as `opacity` and `filter: blur()` already did, and
  mixed with what is behind it rather than laid over it. The modes are
  the separable ones the compositor already has: `multiply`, `screen`,
  `overlay`, `darken`, `lighten`, `difference` and `soft-light`. The
  rest of the CSS list is W450 rather than approximated. Mixing happens
  in the sRGB-encoded space the painter works in, which is the space a
  browser blends in, so `multiply` of `#8080ff` over `#ff8800` gives
  `#804400` in both. Both renderers blend: the GPU copies the markup
  surface before it draws a blending group and reads the copy as the
  backdrop, so such a box stays on the device. The device mixes in
  16-bit floats, so the two agree to the golden tolerance rather than
  byte for byte, which a new golden case holds them to.

### Fixed

- **Markup written as a whole HTML document draws what a browser
  draws.** `html` and `body` name the clip's box rather than boxes of
  their own, but they were still built as boxes, so an absolutely
  positioned child of `<body>` resolved its `left`, `top` and `inset`
  against a `<body>` of no size and landed at 0,0. A design wrapped in
  a full-frame container, `position: absolute; inset: 0` with
  `overflow: hidden`, lost that container's size and had its whole
  subtree clipped away: the clip drew nothing at all, with no `W450`,
  no `E451` and a report that said overlays were drawn onto every
  frame. The same markup as a fragment, which is what every example in
  `examples/` is, drew correctly, which is why nothing caught it.
  `<html>` and `<body>` are now folded into the clip's box, carrying
  their `id`, `class` and `style` onto it, and `<head>` draws nothing
  as before.

- **An animated box at the top level of a document no longer moves the
  whole picture with it.** The clip plays the animation of the markup's
  outermost element, which moves the finished picture rather than that
  one box. Where the document's top level held several elements, the
  first of them was still treated as outermost, so its animation moved
  its siblings too: two drifting blobs and a card written as three
  top-level boxes drew the card rotating and sliding with the first blob,
  on both renderers. A document whose top level holds more than one
  element now has no outermost element, and every animation in it is
  played where it is written, each element composited as its own group.
  Documents with one top-level element, which is most of them, are
  unaffected. The scene in `check.sh` had a wrapper round its blobs to
  avoid this and is written without it now, which renders the same to the
  pixel.

- **Markup no longer borrows a face from whatever the machine has
  installed.** A document's fonts sit in the same database as the
  machine's, and an attribute no face of the named family had was matched
  across all of them, where an exact weight or a real italic in another
  family outranks the right family at the nearest weight or an upright
  face. `font-weight: 600` over a family shipped in regular and bold drew
  in FreeSans on a Linux box that has FreeSans installed, and in
  something else on a Mac that does not. Three things close it. A family
  a document ships now replaces the machine's copy: the installed faces
  of that family are dropped, so the document draws in the version it
  carries rather than in whichever version the machine has, and the
  claim made in 0.5.0 that a shipped family wins is now what the code
  does. A weight the family lacks is matched against its own faces by the
  CSS rule. And a family with no italic face is drawn upright rather than
  in another family's italic; there is still no synthetic oblique. The
  `markup-card` references and the `markup-opening` reference at 4.2s are
  regenerated: they had the substituted face in them. Found by running
  the golden cases on a second machine for the first time, where the GPU
  renderer agreed with the CPU renderer to within one code on every case
  but both disagreed with the references.
- **The renderer note no longer claims a composite that did not
  happen.** An output whose frames go straight from the decoder to the
  encoder reported `GPU ...: the frames were composited on the device`
  beside the note saying the frames were not composited at all. The
  note is now printed only when the compositor draws.

## 0.6.0 (2026-09-18)

### Added

- **The GPU renderer.** A new crate, `geneva-gpu`, renders on a device
  through `wgpu` (Vulkan on Linux, Metal on macOS, DirectX 12 on
  Windows; a software implementation such as Mesa's lavapipe when asked
  for). `render --renderer auto|cpu|gpu` chooses the renderer: `auto`
  is the GPU where the machine has a hardware one and the CPU
  otherwise, `gpu` takes any device and falls back to the CPU with a
  note when none opens; the report's notes name the device. The
  renderer draws every source (placement, crop, shape and luma masks,
  opacity, transitions, every blend mode, the fade's veil, nested
  compositions) through the same rules as the CPU renderer: text is
  painted on the CPU by a painter both renderers share and uploaded;
  a markup box's boxes are painted by the same painter and its groups
  (opacity, blur, rounded and polygon clips, transforms) are
  composited on the device in the painter's encoded space, the
  finished box decoded to linear light there, and the pictures of
  groups that do not change from frame to frame are kept on the device
  as the painter keeps them; pictures that do not change are kept on
  the device under a
  96 MB budget, an 8-bit 4:2:0 video frame goes up as its planes and is
  converted to linear light on the device by the same arithmetic as
  the CPU conversion (HDR, RGB and rotated frames go up as the picture
  the decoder converts), and a blurred clip is drawn onto a layer and
  blurred by the same three box blurs, in f32 as on the CPU. The frame
  is packed into the encoder's planes on the device too, for every
  layout the encoders take, SDR and HDR, and read back through a ring
  of staging buffers with the next frame already drawing. Every golden
  case agrees with the CPU renderer to within one 8-bit code. `frame`
  and the overlays over a copied picture stay on the CPU. A new golden
  case, `tests/golden/masks-and-fades`, pins masks (shape, feather,
  invert, luma) and transitions (crossfade, fade) down on both
  renderers; neither had a reference frame before, and neither had the
  blur effect, which `tests/golden/blur` now pins down (a wide blur on
  a cover-fit image, a small one on a moving shape, an animated one
  under a screen blend, a blurred masked shape at the frame's edge).
  `AssetSource` gains `video_planes`, which the media asset source
  answers for frames the decoder holds as 8-bit 4:2:0 SDR shown
  unrotated. The crate is
  behind the CLI's `gpu` feature, on by default; the minimum Rust
  version moves to 1.87.
- **E503** for a renderer whose device fails; the CPU renderer never
  reports it.

### Changed

- **Markup can name a font asset.** Every `font` asset of a document is
  registered before a markup box is drawn, so `font-family` may name
  one by its id or by the family the file carries, and a family that is
  both an asset and installed comes from the asset on every machine.
  Before, markup saw installed fonts only, so a document that shipped
  its fonts still rendered its markup with whatever the machine had.
  Two golden cases pin the markup painter down on the opening and the
  lower-third examples (`tests/golden/markup-opening`,
  `tests/golden/markup-card`), with the Liberation faces from the
  golden root.
- **The markup painter draws its rows on every core.** Painting a box,
  its shadows and its picture, masking a group by its `clip-path`,
  laying a group onto its parent and converting the finished box to
  linear light were one thread's work from start to finish, whatever
  the machine; they now share their rows across the thread pool the
  compositor already uses. Every pixel depends on its own inputs alone,
  so the picture is the same whatever order the rows finish in: the
  opening example's 291 frames are byte for byte what they were, and
  take 68 ms a frame instead of 149 on four cores, image export
  included. Text is still shaped and rasterized on one thread.
- **Frames are quantized to 8-bit sRGB by lookup.** Writing a PNG, a
  poster or an image sequence evaluated the sRGB curve three times a
  pixel; the codes now come from a table built once from the same
  curve, which gives the same code for every value, and the rows are
  converted in parallel. On an image sequence of the opening example
  that was 15% of the instructions.
- **The markup composite carries its sampling point along the row.** The
  inverse of a group's transform is affine, so it is held as two linear
  expressions in the destination pixel instead of being rebuilt from the
  rotation and the scale at every tap: the trigonometry and the two
  divisions happen once for the group and one add an axis is left in the
  loop. A group drawn at its own size or larger takes one tap a pixel and
  has its own loop; one drawn smaller still takes up to sixteen. The
  bilinear sampler indexes straight in where the 2x2 neighbourhood is
  inside the image, and a clip's coverage is worked out once as a
  rectangle where it is exactly one rather than per pixel. The opening
  example's markup goes from 182 ms a frame to 139 ms at 960x540, a fifth
  off, and every frame is byte for byte what it was. The plain
  compositor is unchanged.


## 0.5.0 (2026-09-17)

Timeline format 0.4. Documents saying `"geneva": "0.1"`, `"0.2"` or
`"0.3"` are read unchanged. 0.4 adds the optional top-level `keyframes`
map, the `captions` source kind, and `loudness`, `hygiene` and `denoise`
on output audio.

### Added

- **Markup redrawn from kept pictures.** An element inside markup that
  only moves is painted once instead of once a frame. A group's boxes
  are painted where they sit and its transform is applied when the
  group is composited, so a drifting element paints the same pixels
  every frame; those are now kept and the frame copies out the part it
  needs. The opening example's markup goes from 205 ms a frame to
  153 ms at 960x540, for about 98 MB more memory, and every frame is
  byte for byte what it was.
- **A copied picture with encoded sound.** An output whose mix is brought
  to a loudness, cleaned or denoised no longer drags the picture through
  the encoder with it. When nothing else asks for a re-encode, the video
  packets are copied and the treated mix is encoded beside them, the two
  written together so the file interleaves as a copy does. The report
  calls it `copy-picture` and gives `frames: 0`. `--for podcast` reaches
  it whenever the source's picture already fits the target: three minutes
  of 1080p30 take 4 s instead of 80 s, and the video stream is byte for
  byte the source's.

- **Loudness.** `output.audio.loudness` brings the mix to an integrated
  loudness (`target_lufs`, measured as ITU-R BS.1770-4 and EBU R128
  measure it: K-weighted, gated, over the whole output) with one gain,
  and a limiter holds the true peak under `true_peak_dbtp` (default
  -1). The mix is read once to measure and once to write, so every
  audio source is decoded twice. The render's notes say what was
  measured and applied, and where the limiter took some of the gain
  back, at what loudness the file was written. `--for youtube`,
  `instagram` and `tiktok` set -14 LUFS and -1 dBTP, which those
  platforms normalise to, and a source is copied as it is only when its
  audio already measures within one loudness unit of that; `geneva
  targets` shows the column. Out-of-range values are E423. The meter,
  true peak and limiter are a crate of their own, `geneva-audio`, with
  no dependencies.
- **Audio hygiene.** `output.audio.hygiene` runs a second-order
  high-pass at 80 Hz (rumble, handling noise, DC offset) and, where the
  mix hums at 50 or 60 Hz, notches 2 Hz wide on the fundamental and the
  harmonics that show. Hum is found by arithmetic (one Goertzel probe
  per candidate against its neighbours, no transform), the mix is read
  once to look for it, and the render's note says what was found. Fixed
  curves, meant for speech: on music the high-pass takes the bass.
- **Audio diagnostics.** `validate --probe` measures each asset's audio
  and reports it (N310: integrated loudness, true peak, noise floor,
  distance from the document's loudness target), and warns about
  clipping (W311), a DC offset (W312), mains hum (W313) and silence
  (W314); a stereo file carrying one signal twice is noted (N315). A
  mono file is measured as a player measures it, not as the decoder's
  3 dB-down upmix. `render` does not measure, since it reports what it
  applies.
- **Speech denoising, behind a build feature.** `output.audio.denoise`
  and `--denoise` on the verbs run the mix through DeepFilterNet
  (v0.5.6, MIT or Apache-2.0, the model embedded) before hygiene and
  loudness, at 48 kHz as a mid and a side channel so the stereo image
  is kept, with the model's lag taken off so the sound stays where it
  was. Speech only: it damages music and overlapping speakers. It is
  in the `denoise` cargo feature and not in the default or the released
  builds, which refuse the document with E424 at `audio/denoise` before
  rendering anything. A minute of 48 kHz stereo takes 2.9 s on one core
  in a release build, and the feature adds 25 MB to the stripped binary
  (31.6 to 56.7 MB).
- **`--for podcast`.** -16 LUFS and -1 dBTP, as Apple Podcasts asks
  for, with hygiene on. Hygiene is a change no copied audio track can
  carry, so the sound is always encoded for this target; the picture is
  copied where it already fits, which is what makes it cheap on a long
  recording.
- **Audio in the golden harness.** A golden case can carry
  `expected/audio.wav`, compared on numbers rather than bytes: the
  largest sample difference, the level over the whole track and over
  every 100 ms window, and the integrated loudness, each within a
  stated tolerance, plus the loudness and true peak the case states.
  Two cases cover a loudness target that is met exactly and one where
  the limiter holds the peaks.
- **Gradients in markup.** `background` takes `linear-gradient()` and
  `radial-gradient()` as well as a colour: an angle or a `to` side or
  corner, stops with optional positions, and a circle or ellipse `at` a
  position. `conic-gradient`, `repeating-` gradients,
  the radial size keywords and `url()` images are each named as
  undrawn (W450) rather than skipped; a radial is always sized to the
  farthest corner.
- **Shadow lists.** `text-shadow` and `box-shadow` in markup, a text
  source's `shadow`, and `text-shadow` in a keyframe all take several
  shadows, front to back as CSS lists them; the first is drawn on top.
  A keyframe from one shadow to two pads the shorter list with a
  transparent shadow of no size, as CSS does, so the newcomer fades in.
  `inset` box shadows are still not drawn.
- **Markup blends like a browser.** Inside an `html` source, gradients,
  translucent boxes, shadows, blur and text edges are blended on
  sRGB-encoded premultiplied values, as a browser blends them, and the
  finished box is converted to linear light once for the compositor. A
  30% teal over a dark ground is 83 on the green channel, as on a page,
  where linear-light blending gave 139. Everything outside the box
  (the clip's transform, opacity, blend mode, transitions) is unchanged.
- **`steps()` and `linear()` timing functions.** Both are read where a
  timing function goes: in a CSS `animation`, and as `{ "steps": [n,
  "jump-end"] }` and `{ "linear": [[input, output], ...] }` on a
  keyframe's `ease`. `linear()` spreads the inputs it is not given the
  way CSS does.
- **`clip-path: polygon()` in markup.** A box and its children are cut to
  the polygon, with anti-aliased edges and the nonzero fill rule; points
  may lie outside the box. It animates on an element inside the
  outermost one, point by point when the counts match. Other shapes are
  W450.
- **Animations on elements inside markup.** An `animation` on an element
  inside the outermost one is played by the renderer: the element is
  composited as a group with the transform, opacity and `filter: blur()`
  the animation gives it at each frame, several animations stack the way
  a browser stacks them, `animation-fill-mode` and the other longhands
  are read, and a keyframe can also set `color`, `text-shadow`,
  `letter-spacing`, `width`, `height`, `max-width`, `min-width` and
  `background-position`, the sizes laying the document out again at each
  frame. Such an animation was W450 before. The outermost element's
  animation is still the clip's, and a rule for it that sets anything
  past transform and opacity is E442.
- **`opacity` groups.** A box with an opacity below one is composited as
  one picture, so its overlapping children no longer show through each
  other. `filter: blur()` is drawn the same way.
- **Gradient text fill.** A text source's `fill` draws its glyphs with a
  `linear-gradient()` or `radial-gradient()` in place of `color`, as the
  string alone or as `{ "gradient", "width", "height", "x", "y" }`. The
  gradient sits on a tile that repeats across the text; `x` and `y`, the
  tile's start, are animatable, so a wide tile with a keyframed `x`
  sweeps the gradient across the text. In markup, `background-clip: text`
  does the same with an element's `background`, and `background-size` and
  `background-position` place the tile, on boxes as well as text.
  `-webkit-background-clip` and `-webkit-text-fill-color` are read too.
- **Animatable text colour and shadow.** A `text` source's `color`,
  and a `shadow`'s `color`, `x`, `y` and `blur`, take keyframes like any
  other animatable value; the `text-shadow` shorthand stays a constant.
  A highlight's colour animates independently or follows the base. The
  rendered image is sized for the shadow's furthest reach over the clip
  rather than its size at the frame, so a glow that swells does not move
  the glyphs under it. A text whose style moves is drawn fresh each frame
  instead of once per clip.
- **`text-shadow` in markup.** One shadow, drawn behind the glyphs
  through the same path a text clip's `shadow` already used. It does not
  affect layout: the engine pads the image it renders to make room, and
  painting takes that padding back off, so adding a glow leaves the words
  where they were. It is clipped at the edge of the clip's own box.

### Changed

- **Markup renders faster.** A group inside an `html` source (an element
  with an animation, a transform, opacity, a filter or a clip) was painted
  whole into its own buffer, however far it reached outside the frame,
  and then resampled whole. It is now painted and resampled only where
  its parent can show it, taken back through its transform and padded
  for its blur, and the buffer of a group that lands off the frame is
  skipped. A box with no border no longer tests its inner edge per
  pixel, a gradient no longer takes a modulo per pixel inside its own
  tile, and the finished box's conversion to linear light reads a table.
  The pixels are the same to within one level in 255. The README's
  opening (960x540, four full-frame radial gradients drifting under the
  text) went from 526 ms to 194 ms a frame on one core, from 744 ms to
  240 ms at its worst; layout is 1 ms of that, text shaping under 2 ms.

### Fixed

- **The published schema no longer changes under a released version.**
  The test that keeps `schema/` current wrote to a path with the format
  version spelled into it, so every change to the format types rewrote
  the file 0.4.0 had published as format 0.3 rather than publishing a
  new one. It now takes the name from `FORMAT_VERSION`, and
  `geneva-timeline-0.3.schema.json` is back to what 0.4.0 shipped.

- A blurred `box-shadow` was a distance-field ramp that stayed solid to
  the box's edge, so a bar thinner than its blur glowed far harder than
  in a browser. It is now the box convolved with the Gaussian CSS
  defines (half the blur as standard deviation), exact for a box whose
  corners are smaller than the blur. A blurred `text-shadow` was about
  a fifth wider than CSS asks; its box passes are sized from the same
  definition now.
- A `text-shadow` or `box-shadow` whose colour was written as `rgb()` or
  `rgba()` was dropped as a shadow list (W450), since the commas inside
  the colour were read as separators. Only a comma outside brackets
  separates shadows now.
- A box with `opacity`, a `filter` or an inner animation was painted
  after every positioned box in its stacking context, so a translucent
  haze under a positioned vignette came out on top of it. Such a box is
  painted in tree order with the positioned boxes, where CSS puts it.


## 0.4.3 (2026-09-14)

### Added

- **Overlays written as HTML and CSS.** A clip can take a source of kind
  `html`, with the markup in the document or in an asset of kind `html`,
  and draw it:

  ```json
  { "kind": "html", "asset": "card", "width": 560 }
  ```
  ```html
  <div class="card"><h1>Dragon CRS-17</h1><p>Berthing at the ISS</p></div>
  ```

  The box defaults to the size of the frame, so CSS places things in the
  picture the way it places them on a page and the clip needs no
  `transform` at all; `"auto"` on either side fits the content instead.
  That costs nothing: the painter reports the rectangle it actually
  marked and the compositor reads only that, which on the lower third
  example is the difference between +35% and +13% against a box drawn
  tight around the card. The remaining difference is a drop shadow the
  tight box was silently clipping.

  `box-sizing` is `content-box` by default, as CSS has it. taffy's own
  default is `border-box`, so a `width` with `padding` used to come out
  the wrong size.

  Motion comes with it. `@keyframes` in the markup's stylesheet are in
  scope for the clip that draws it, and an `animation` on the outermost
  element is what the clip plays, so a card that slides in a browser
  slides here. `examples/card.html` does exactly that, and renders byte for
  byte what the same animation written in the document rendered. The
  clip's own `animation` replaces the markup's, since only the document
  knows where the clip sits in time (W451 when both are set). An
  `animation` further in is W450: the markup is painted once and the clip
  moves that picture, which is what makes a box on screen for a minute
  cost one layout.

  A new crate, `geneva-html`, parses a strict HTML subset and a CSS subset
  (type, class and id selectors, the descendant and child combinators, the
  real cascade with specificity, source order, `!important` and
  inheritance) and lays it out with
  [taffy](https://github.com/DioxusLabs/taffy), so flexbox and block
  layout are an implementation of the spec, not an approximation. It draws
  `display`, `position`, the box model, the flex properties, borders and
  `border-radius`, `box-shadow`, `opacity` and the text properties;
  `em` and `rem` resolve against the element's own font size. The box is
  as wide as `width` and, without a `height`, as tall as its content, the
  way a card sizes itself to its text. Layout and paint do not depend on
  time, so a box is drawn once per clip whatever its length.

  A picture or a stylesheet the markup points at is a path, relative to
  the markup, the way it is on a page: `<img src="logo.png">` and
  `<link rel="stylesheet" href="house.css">` find the same files geneva
  does and a browser does. The rule every asset path follows holds here:
  no leading `/`, no `..`, no drive letter, no URL. That, along with a
  file that is not there, is E452 while the document is validated, so a
  missing picture is an error before anything is drawn rather than a hole
  in the frame. `<link>`, `<meta>`, `<base>` and `<title>` never become
  boxes, so they cannot take a slot in a flex row.

  It is not a browser, and it says which parts of one it is not: there is
  no inline layout (an element's text is one paragraph, a child element is
  a box), and `opacity` multiplies down the tree instead of grouping. Anything it
  cannot draw it names rather than ignoring: E451 for markup or a
  selector it cannot parse, with the line and column, and W450 for a
  property it does not draw and for an element with a renderer of its own
  (`<iframe>`, `<svg>`, `<canvas>`, `<video>`, `<object>`, `<embed>`),
  E452 for a file it points at. E450 covers an
  `html` source with both `html` and `asset`, or neither.

- **Motion written the way a stylesheet writes it.** A top-level
  `keyframes` map holds CSS `@keyframes` rules, an offset (`from`, `to`,
  `60%`) to a declaration block, and a clip's `animation` field plays
  them with the CSS shorthand:

  ```json
  "keyframes": { "slide-in": { "from": "translate: -100%", "to": "translate: 0" } },
  "animation": "slide-in 0.5s ease-out, fade-out 0.3s 3.7s"
  ```

  A block sets `transform` (`translate`, `translateX`, `translateY`,
  `scale`, `scaleX`, `scaleY`, `rotate`), the same three as separate
  properties, or `opacity`. The shorthand takes a duration, a delay, a
  timing function, an iteration count (including `infinite`) and a
  direction (`alternate` and the rest), in any order, plus geneva's own
  `spring(stiffness[, damping[, mass]])`. A rule is laid over what the
  clip already sets: translations add to `transform.position`, scales
  multiply, rotations add, and `opacity` replaces. Two animations may
  drive one property where their times do not collide, which is how a
  fade-in and a fade-out share `opacity`.

  A distance in `translate` can be a percentage, of the box being moved:
  the element carrying the animation when the rule comes from markup, and
  the clip's own box when the clip plays it, as CSS resolves one against
  the element. So `translateX(-100%)` slides a card in by exactly its own
  width and survives a change of output size. A clip whose size is only
  known once its file is open (a video, an image, a text run) has no box
  to take a share of, and says so (E442). `font-size` and
  `line-height` take percentages too, of the inherited size.

  It all lowers to the same keyframe tracks the long form produces, so
  nothing downstream changes and `--show-timeline` prints the document as
  written. A property set at
  one offset of a rule has nothing to interpolate with and is dropped,
  which is what lets `to { transform: none }` mean "back where it
  started" rather than "and reset the scale and rotation too". New codes
  E440–E444 and W440 cover a missing rule, a malformed shorthand or
  rule, a property driven twice, and a rule that interpolates nothing;
  W451 covers two things asking for one motion, and W203 notes a rule
  nothing plays.

- **A transcript is a source.** A clip can take a source of kind
  `captions` pointing at a `.srt`, a `.vtt`, or the `.json` a speech
  recogniser writes:

  ```json
  { "kind": "captions", "asset": "words",
    "style": { "font": "700 44px Liberation Sans",
               "highlight": { "color": "#ffd233" } } }
  ```

  One clip in the document becomes one clip per cue on the timeline, so
  the frames between cues are still copied rather than composited: on the
  ten-second example, 183 of 300. The cues carry their own times, so
  there is nothing to write down; `position` (`bottom`, `top`,
  `center`), `margin` and `safe` place them, a margin defaults to the
  title-safe inset, and a WebVTT file's own `line`, `position`, `align`
  and `size` are followed unless `follow_file` is false. Everything a
  caption looks like lives under `style`, which takes a text source's
  fields.

  Word files go in as they came out. whisper's
  `{"segments": [{"words": [...]}]}`, a bare `{"words": [...]}` and a
  bare list all read; `word` and `text` both name the word; `probability`,
  `seek`, `tokens`, WhisperX's `score` and every other key are ignored.
  Forgiving about keys, strict about structure: a file with no words in
  it anywhere is E453 rather than a caption track that draws nothing, and
  times that are plainly milliseconds, which AssemblyAI and Deepgram
  write, are E453 too, with the remedy in the message. Word times are
  what lets `style.highlight` pick out the word being said; SubRip and
  WebVTT time whole cues, so a `highlight` on one is W453 rather than a
  silent difference. N453 says how many cues were read and from where.
  This sits beside the `text` source's `words`, which is still the way to
  write a handful of words by hand.

  `geneva subtitles --burn` takes the same three file types, and gains
  `--highlight COLOR` for the word being said.

  Parsing moved out of `geneva-media` into `geneva-timeline`, where the
  resolver needs it; `geneva_media::subtitles` re-exports it, so reading
  a caption file no longer needs the media libraries at all.

- **Shorter spellings for the three things a document repeats most.** A
  point (`transform.position`, `transform.anchor`) can be a pair or a
  string: `[30, 36]`, `"30 36"`, `"0% 50%"`, or the CSS position keywords
  `"left"`, `"center"`, `"bottom right"` (either order, and a keyword
  pairs with a length as `"left 36"`). A keyframe can be its fields in
  order, `[t, v]` or `[t, v, ease]`. A timed word can be `["word",
  start, end]`, or `["word", start]` where the next word's start is this
  word's end; the last word needs its own (E102). All three are accepted
  next to the long forms and mean exactly the same thing, and a printed
  timeline (`--show-timeline`) still uses the long forms, which stay
  canonical. A document that uses them says `"geneva": "0.3"` like any
  other; it needs 0.4.3 or later to read. The card in
  `examples/lower-third.json` went from 176 lines to 40.
- Progress as JSON. A render that composites frames now reports itself on
  stderr in the format the command asked for: the line rewritten in place
  for a person, and with `--format json` one object per line
  (`{"event":"progress","frames":540,"total":1800,"seconds":8.0,
  "rate":67.5,"time":18.0,"duration":60.0,"remaining":18.67}`) at the
  first frame, about once a second, and at the last. `remaining` is the
  estimate at the rate so far, and is `null` where there is nothing to
  estimate from: the last line, and the first half second, which measures
  the startup as much as the work. Stdout stays the one report document,
  so a program can follow a long render and still read the result. Seventeen of the forty-one products surveyed parse ffmpeg's
  stderr with regular expressions for this. A run that copies its streams
  or writes audio alone renders no frames and says nothing.

- **A `fade` transition, and a crossfade that reaches the sound.** A
  clip's `transition` takes a second kind:

  ```json
  "transition": { "kind": "fade", "duration": "0.6s", "color": "white" }
  ```

  `crossfade` dissolves, both clips up at once. `fade` dips through a
  color, black unless the transition names one, with the leaving clip
  gone before the arriving one appears. `color` on a crossfade is W304,
  since nothing would show it. `geneva concat` gains `--fade TIME` and
  `--fade-color COLOR` beside `--crossfade`.

  The sound follows the picture now, which it did not before. A crossfade
  used to leave the audio alone: both clips played at full gain through
  the overlap and summed, measured at +3 dB on two tones, so a join was
  louder than either side of it and two pieces of dialogue ran together.
  Transitions now reach the mixer. A crossfade crosses at constant power,
  each gain the square root of its linear ramp, which holds the level
  across the overlap to within a few tenths of a decibel. A fade goes to
  silence at the midpoint and back. Explicit `fade_in` and `fade_out` on
  an audio clip stay linear, which is what a fade to silence wants.

  The dip color covers the whole frame, layers below included, and a fade
  above the first layer makes the composition composite rather than copy.
  Both are written down in docs/timeline.md next to the feature.

- **Transitions at the head and tail of a layer.** A transition on the
  first clip used to be W303, a warning whose help told you to write
  opacity keyframes by hand. It now opens the piece: over the whole
  duration a `fade` comes up out of its color and a `crossfade` up from
  whatever is behind. `transition_out` closes a layer the same way, so a
  piece fades up from black and out to black without anyone counting
  keyframes against a clip length that might change. Setting
  `transition_out` where another clip follows is E307, since that clip's
  `transition` already covers the join. W303 is retired.

- **`ease` on a transition**, taking what a keyframe's easing takes.
  Both ramps were linear; `ease-in-out` is the usual choice for a slow
  dissolve. A malformed curve is E308.

### Changes

- A multi-output render takes the direct path when the canvas is one video
  filling the frame: the decoded frames are scaled to each rendition and
  handed to the encoders without passing through the compositor, and only
  the frames a poster or a sprite tile actually takes are composited. On a
  four-core machine, three renditions plus a poster, a sprite sheet and
  speech audio from a 60-second 1080p file take 42.9s, against 53.1s for
  the six ffmpeg commands that do the same work and 43.3s for the single
  command with `split` filters. The renditions also come out cleaner,
  since the picture no longer makes a round trip through linear-light
  float and back.
- The person's progress line is rewritten on a timer (four times a
  second) rather than every thirtieth frame, so it moves at the same pace
  whatever the frame rate, and the chunked path reports like the others.
- A diagnostic about the document as a whole no longer prints an arrow
  line reading `--> (document)`. It pointed at nothing and doubled the
  length of every report that was only notes.

- **`z-index`.** The painter walked the tree in document order and drew
  what it found, so the only way to put a box in front was to move it
  down the markup. It now paints a stacking context the way CSS does:
  boxes with a `z-index` are painted whole, in the order their numbers
  give, with negative ones under everything in flow and the rest above
  the positioned boxes that have no number. A plain box opens no context,
  so a `z-index` deeper in can still rise above an uncle.

  It applies where CSS applies it, on a positioned box or a flex item,
  because a card that looks right here has to look right in a browser.
  Setting it anywhere else does nothing, as it does on a page, and that
  is W454 rather than silence: forgetting `position` is the usual way to
  get `z-index` wrong. `opacity` still does not open a stacking context,
  which is the same thing it already said about not grouping.

### Fixes

- **`body` and `html` selected nothing.** The markup is wrapped in a root
  element standing in for the page, but nothing could name it, so
  `body { display: flex; background: ... }` parsed, matched no element
  and was discarded without a word. Both names now reach that root. It
  draws its background and border when someone asks for them, and stays
  unpainted otherwise, which is what keeps the composited area down to
  the content. Its size is the box the clip draws into, so padding goes
  inside that box rather than pushing the root past it.

- **A rule that matched nothing said nothing.** W450 covers a property
  geneva does not draw and E451 a selector it cannot parse, but a
  selector that parsed and then matched no element fell between them: a
  misspelt class name produced an unstyled box and a report reading "ok:
  no problems found". That is W452 now, naming the selector and how many
  declarations it wasted. Only the markup's own `<style>` is checked; a
  stylesheet it links to is written for more than one file, so the rules
  this one leaves alone are not mistakes.

- `letter_spacing` was a multiple of the font size rather than pixels.
  cosmic-text adds the value to an advance it has already divided by the
  font's units per em, so what it wants is a share of the em; geneva
  passed pixels straight through. `letter-spacing: 1.6px` on 13px text
  came out at 20.8px, thirteen times too wide, in markup and in a `text`
  source alike. There was no test for it, so there is one now.

### Examples

- `lower-third.json` draws its card from `card.html` and animates it with
  `@keyframes`, so the example is markup, a stylesheet and twenty lines of
  JSON. Every other example that places or animates something is written
  with the shorthands above and renders byte for byte what it did before.
- `social-reframe.json`: a landscape clip reframed to 9:16 with a blurred
  backdrop and word-timed captions. `renditions.json`: three renditions, a
  poster, a sprite sheet and speech audio from one pass. `lower-third.json`
  gains the name and title it was drawing a plate for.
- The demo is a broadcast-style name card at the top left with captions
  under it: a dark plate with a red rule, the name in semibold and the
  strap in letter-spaced caps, and cues whose current word is white
  against a cool grey rather than yellow. `lower-third.json` reads the
  same `words.json` as the captions example.
- `words.json` is a whisper transcript pasted in as it came out, and
  `captions.json` puts it on the footage with a second caption layer read
  from `ar.srt`, so the example shows both kinds of caption file.
  `social-reframe.json` captions the tall frame from the same transcript,
  where the only thing that changes is the margin.
  `scripts/demo-gif.sh` renders the README's figures from those files.
- `examples/talk.mp4`, a four-second 1080p clip rendered by geneva from
  shapes and text, so the examples that need video run without supplying
  any. Those two documents no longer set `output.duration`; they take the
  length from the file, which `render` and `frame` probe for.

## 0.4.2 (2026-09-14)

Two verbs fewer. `publish` and `sprites` are gone after a day: `publish`
was `convert --for web` plus pictures into a directory, under a name
that means "upload", and its one useful rule belongs to `--for`; the
`sprites` kind stays in the document for the players that want a sheet.
`poster` is folded into `frame`.

### Changes

- `--for` copies a source that already fits its target. A source used
  as it is, H.264 4:2:0 with AAC in MP4 or MOV, within the target's
  size, frame-rate and bitrate ceilings, is stream-copied by `convert`,
  `trim`, `concat` and the other verbs instead of being re-encoded to
  the same thing, and the note says so. `--crf`, `--quality`, `--budget`
  or `--exact` re-encode regardless.
- `frame` takes a video file as well as a timeline: `geneva frame
  talk.mp4 -o thumb.jpg` writes the chosen frame (the first clear one
  after the opening) or the one at `--at`. Both inputs take `--width`
  and `--height`, and write JPEG for a `.jpg` output.

### Removed

- `publish`, `sprites` and `poster` verbs, and `manifest.json`. The
  JSON report carries the same rows with content types; redirect it to
  keep a manifest. A document with an `outputs` map and `render -o DIR`
  gives any bundle with exactly the files listed.

## 0.4.1 (2026-09-14)

### Added

- The report names the media type of every file it writes: `content_type`
  beside `output` for `render`, `frame`, the verbs and `subtitles
  --extract`, and on each `outputs[]` row of a multi-output render, with
  `width` and `height` for pictures and video. A sprite sheet's `.vtt`
  map is its own row (`kind` `sprites-map`), so the list is every file
  written. `publish` writes the same list as `manifest.json` in its
  directory, with paths relative to it and the video's size and length,
  for whatever uploads or serves the files: a bucket needs the content
  types set, and the manifest carries them.

## 0.4.0 (2026-09-14)

Timeline format 0.3. Documents saying `"geneva": "0.1"` or `"0.2"` are
read unchanged.

### Added

- **Multi-output documents.** A top-level `outputs` map lists every file
  one render writes from the composition: `video` renditions at several
  sizes, a `poster` still, a `sprites` sheet with its WebVTT map for seek
  previews, and the `audio` alone, each with its own size, encode and
  audio settings. `geneva render -o DIR` writes them all in one pass:
  the sources are decoded and the frames composited once, scaled per
  rendition and encoded in parallel threads; a canvas-size rendition
  with no encode asks of its own is stream-copied like a single-file
  render. When no entry is a video, only the frames the pictures need
  are composited. The report lists each file with its size and mode.
  New codes E430–E433 cover fields that do not belong to a kind, bad
  file names, two entries writing one file, and out-of-range values.
- `geneva poster`: one still, at a time or the first clear frame after
  the opening (not dark, past the first motion).
- `geneva sprites`: a thumbnail sheet and the `.vtt` file players read
  for seek previews.
- `geneva publish`: everything a web page needs for a video from one
  pass over the source: `video.mp4` fitted to the `web` target (or
  `--for`), `poster.jpg`, `sprites.jpg` and `sprites.vtt`, and with
  `--speech` the sound as 16 kHz mono `speech.wav`. A source that
  already fits the target is copied, not re-encoded, and the report
  says so.
- `geneva audio --extract --speech`: 16 kHz mono, what speech
  recognizers want.
- Pictures of the composition keep exact sizes; only video sizes are
  rounded to even.

### Changes

- The copy planner's refusal for a colour-tag mismatch between two SDR
  encodings now says how to get the copy (set `output.color` to the
  source's tags); the verbs already do.
- The JSON Schema moves to `schema/geneva-timeline-0.3.schema.json`.

## 0.3.5 (2026-09-13)

### Fixes

- Rotated clips. Phones store portrait video as a landscape stream with a
  rotation flag, which geneva ignored: a portrait clip came out sideways
  from every verb, and a copy dropped the flag. Now the probe reports the
  picture as displayed (with `rotation` and the stored size), the reader
  turns frames upright for rendering, the copy planner compares the
  displayed size and keeps the flag on the copied stream, and the direct
  and smart-cut paths hand rotated sources to the compositor. Found by a
  survey of how open-source products use ffmpeg: 21 of 41 handle this flag
  explicitly.
- A silent video (no audio stream) is now copied without audio rather than
  re-encoded; only joining a silent source with a sounding one refuses.

## 0.3.4 (2026-09-12)

### Changes

- HDR to SDR now follows ITU-R BT.2446 method A, the conversion
  specified for 1000-nit HDR to SDR, in place of the BT.2390 EETF. The
  EETF squeezed everything from 200 to 1000 nits into the top fifth of
  the SDR range, so on a sunlit iPhone clip the sand and the sky went to
  white while the mid-tones stayed bright. Method A keeps the texture of
  the highlights, puts reference white (203 nits) at 0.41 of SDR white
  in linear light, and lands 1000 nits on SDR white; mid-tones read a
  little darker than before, as the recommendation intends. PQ sources
  mastered above 1000 nits are first brought down to 1000 with the EETF.
  The `hdr-to-sdr` golden is regenerated.

## 0.3.3 (2026-09-12)

### Fixes

- An HDR source into an SDR output was copied as it was once 0.3.2 let
  phone recordings through the copy planner: `convert hlg.mov -o out.mp4`
  wrote the HEVC HLG stream unchanged instead of tone-mapping it. The
  planner now copies only when the source's color encoding is the
  output's, and says so in the notes otherwise ("hlg.mov is HDR (hlg
  bt2020, ...), and the output is SDR (...)"). With `--keep-hdr` the
  copy still happens.

## 0.3.2 (2026-09-12)

### Fixes

- Trims and joins of phone recordings re-encoded instead of copying.
  Such a file reports an average frame rate (frames over its duration)
  that no frame has, since its timestamps jitter, and the copy planner
  compared that figure to the rate the probe had settled on. The planner
  now reads the rate the same way. An iPhone HEVC clip trimmed with
  `--keep-hdr` is copied, as ffmpeg's `-c copy` does.
- When the streams could have been copied but the output cannot take
  them (a codec the container cannot hold, a source with no audio,
  sources that differ), the render's notes now say which.

## 0.3.1 (2026-09-12)

Fixes from the first Mac run of 0.3.0.

### Fixes

- `--keep-hdr` on a Mac failed at the first frame: the hardware encoder
  takes ten-bit pictures as P010, and the frames were laid out as the
  planes were. An HLG iPhone clip now goes through VideoToolbox as HDR
  HEVC.
- Chunked encoding's `auto` made every Mac encode slower, since the
  hardware encoder is its own bottleneck and two sessions contend for
  it; on four cores it also slowed x264 and DNxHD. `auto` now chunks
  only the two encoders measured to gain, VP9 and OpenH264, and never a
  hardware encoder. A number in `chunks` still forces it for any.
- `check.sh` ran every step against a missing input file, printing a
  column of failures; it now stops at once and says which file.

## 0.3.0 (2026-09-12)

HDR in and out, an A/V sync corpus that found eight timing bugs, chunked
encoding across cores, and two encode fields: `tune` and
`fixed_keyframes`. The timeline format stays 0.2; every new field is
optional.

### Added

- `encode.video.tune`, in x264's names (`film`, `animation`, `grain`,
  `stillimage`, `fastdecode`, `zerolatency`), and `--tune` on the verbs.
  x264 applies every one; VP9 takes `film`, AV1 `fastdecode`, NVENC and
  VideoToolbox `zerolatency`. An encoder with no equivalent ignores the
  tune and the report says so.
- `encode.video.fixed_keyframes`: keyframes at `keyframe_interval` only,
  never at scene changes, as streaming platforms and segmenters want. It
  needs the interval (E422). `--keyframe-interval SECONDS` and
  `--fixed-keyframes` on the verbs. x264, VP9, AV1, NVENC and
  VideoToolbox place keyframes so; OpenH264 cannot and the report says
  so.
- Both settings ask for a real encode: no stream copy or smart cut when
  they are set, as with `crf` and `preset`.
- An A/V sync corpus: `tests/media/sync`, twelve small files from one
  scene with a flash and a tone at known times, muxed with the traps
  real files carry (B-frame edit lists and negative offsets, variable
  frame rate, a 10 s start, an audio track starting after the video,
  NTSC rates, 44.1 kHz, Opus, MPEG-TS), and tests that the marks come
  out where they went in on every path: direct, compositor, stream copy,
  trims copied and exact.

- HDR sources are tone-mapped instead of clipped: PQ and HLG material
  goes through the BT.2390 curve from the source's peak (its mastering
  metadata, 1000 nits without) down to reference white, with hue held
  and highlights rolled off, and its BT.2020 primaries are converted
  into the working space. Still images tagged `pq` or `hlg` on their
  asset take the same path, and 16-bit images keep their depth. A
  golden case, `hdr-to-sdr`, holds gray steps at known nits and the
  BT.2020 primaries in both encodings. See [color.md](docs/color.md).
- Sources with other primaries than BT.709 (the BT.601 sets, BT.2020
  SDR) are converted into the working space; they used to be taken as
  BT.709.
- Chunked encoding: `encode.video.chunks` (and `--chunks`) encodes the
  output in several stretches at once, each on its own share of the
  cores, joined afterwards without re-encoding, with the audio mixed
  once. `auto`, the default, decides from the encoder's measured
  scaling and the machine, so it only kicks in where an encoder would
  leave cores idle; a number forces it, `1` turns it off. Boundaries
  fall on clip starts or the keyframe grid. The report says how many
  stretches ran and how long the encoding and the join took.
- HDR output: `output.color` may be `pq` or `hlg` with BT.2020 on a
  ten-bit codec (`h265` on a hardware encoder, `av1`, `vp9`, `prores`);
  E420 now says which codecs carry it. HDR sources keep their range in
  an HDR composition, graphics land at reference white, and PQ outputs
  carry static HDR10 metadata, the first source's or standard defaults.
  `--keep-hdr` on the verbs keeps HDR sources HDR.

### Fixes

- Time zero of a file is its first video frame, for the picture and the
  sound alike. Each stream used to be rebased on its own start, so an
  audio track that began later than the video (a delayed recording, an
  offset applied at muxing) lost its offset on every path.
- Stream copies of B-frame video from Matroska failed ("non
  monotonically increasing dts"): the container stores no decode
  timestamps, so they are now derived from presentation order.
- AAC priming and Opus pre-skip shifted decoded audio by 21 ms and
  6.5 ms: the decoders now know the packets' time base and move the
  first frame's time past the samples they drop.
- A stream copy dropped the audio packet under the cut, so the decoder
  lost its lead-in and the file its priming trim. Into MP4 and MOV the
  packet is kept and the container trims it.
- A variable-frame-rate source was converted at its average rate (which
  no frame has); the output now takes the source's base rate.
- Frames on a millisecond timestamp grid could be sampled one frame
  late; the sampler allows a quarter of a frame of slack.
- Seeking in MPEG-TS, which has no index, could land past the target,
  so a copied cut moved to a later keyframe and a decoded clip started
  late. A seek that lands late is repeated a few seconds earlier and
  read forward; copied segments start at the last keyframe at or
  before the cut.
- When a copied cut moved to a keyframe, the clips' own audio was still
  cut at the time asked for, a few milliseconds off the picture. It now
  moves with the cut.
- The test suite without the media feature (CI's second job) failed on
  a test that needs media; it is gated now.

## 0.2.0 (2026-09-11)

The timeline format reaches 0.2: `crop`, `effects` (a Gaussian blur),
`mask` and `speed` on clips, all optional, so every 0.1 document still
reads as it is. The compositor was profiled and its largest costs
fixed, and the audio mix is streamed.

### Added

- Timeline format 0.2. A 0.2 document is a 0.1 document with more
  optional clip fields, so `"geneva": "0.1"` documents are read as they
  are; the verbs write 0.2. The schema file is now
  `schema/geneva-timeline-0.2.schema.json`.
- `crop` on clips: a rectangle of the source (pixels or percentages of
  the source's own size) that becomes the clip's box before `fit` and the
  transform. A video shown as it is with a crop takes the direct path,
  the region scaled straight from the decoder, with bars when it does
  not fill the frame.
- `--crop RECT` on `convert`, `resize` and `trim`: `X,Y,WxH`, or `WxH`
  for the middle of the picture. The output takes the crop's size unless
  a size is given.
- `effects` on clips, a list applied to the placed picture before
  opacity and blending, with one kind so far: `blur`, a Gaussian blur
  whose `radius` (the standard deviation in output pixels, as CSS's
  `blur()`) is animatable. A wide blur is computed on a smaller layer
  and brought back bilinearly, so its cost does not grow with the
  radius.
- `--fill blur` on the verbs: a picture that does not cover its frame (a
  landscape video on a portrait canvas, from `--for tiktok` or an
  explicit size) is shown whole over a blurred, scaled-up copy of
  itself instead of bars, as the phone editors do. The copy is a first
  layer named `fill` with no audio; `--show-timeline` shows it.
- `mask` on clips: a rectangle (with corner radius) or ellipse cut from
  the clip's box, with an optional feathered edge, or the luma of an
  image asset stretched over the box; `invert` flips it. Masks are in
  the box's own coordinates, so they follow the clip's fit and
  transform.
- `speed` on clips and audio clips: a constant rate change (`2` twice
  as fast, `0.5` half speed). The clip lasts its source range divided
  by it, nested compositions run faster inside, and audio is resampled
  so its pitch follows, as on a varispeed deck; the clip's own
  keyframes stay in output time. `--speed FACTOR` on `trim` and
  `convert`.

### Performance

- The compositor was profiled on the check scene at 720p and 1080p and
  its four largest costs fixed. At 1080p on four cores: clearing the
  frame 2.9 → 0.65 ms (parallel), the check scene 4.3 → 1.6 ms, a
  frame-sized nested composition 38 → 2.7 ms (its buffer is drawn from
  directly and kept for the next frame instead of being allocated,
  cleared and copied every frame), a scaled or fractionally placed
  image 43 → 6.4 ms (spans are resampled with a constant step and no
  per-texel bounds checks), and the 8-bit 4:2:0 conversion 6.7 → 4.4
  ms (a dedicated loop over 2×2 blocks). A magnified or unit-scale image
  is now sampled once at the pixel center well inside the picture, so
  text and pictures at fractional positions come out sharper; minified
  images, rotated ones and edges keep the 2×2 supersample. The text
  golden references were regenerated for the sharper edges.
- Decoded 8-bit 4:2:0 frames go straight to linear light for the
  compositor, with bilinear chroma upsampling at the standard siting,
  instead of through a 16-bit 4:4:4 pass first. What remains of a
  composited 1080p frame's cost is the memory traffic of the 16-byte
  working pixel (each full-frame pass moves 33 MB) and the encoder.

- The audio mix is streamed: the mixer produces a second at a time,
  reading every voice forward on its own decoder and resampler, so a
  composited output of any length holds only a block of audio in
  memory. The mix is the same whatever the block size (a test checks
  it), and the same as before.
- A render reuses its frame buffers: finished pictures go back to the
  producer to be filled again, instead of a fresh buffer being
  allocated, zeroed and paged in for every frame, and the picture
  handed to a bundled encoder is written in place. A 1080p re-encode
  through the system's x264 takes a third fewer page faults; at a cheap
  preset, where the encoder does not hide the difference, the run is
  about 15% faster than before and ahead of ffmpeg on the same file,
  and at the medium preset it is at parity.

### Fixes

- The build without the media feature (checked by CI with clippy)
  compiled again: the examples and the tests that need media are gated
  on the feature.
- Keeping the source's audio sample rate (0.1.11) broke outputs whose
  codec or container cannot take it: Opus (48 kHz and its own rates
  only) and MXF (48 kHz only) failed on a 44.1 kHz source. The rate
  now falls back to 48 kHz where the codec or container needs it.
- A smart cut is taken only when at least a fifth of the frames can be
  copied; below that (subtitles over most of a clip) the plain encode
  was faster.
- On VideoToolbox, `--for` wrote files about twice the size of a plain
  encode: a data rate limit on top of quality mode changes how the
  hardware encoder works. The ceiling is no longer passed to it in
  quality mode (the report says so), and `--budget` now switches it to
  bitrate mode at the budget's rate, which holds the size. New encode
  field `bitrate_kbps`, which x264 ignores in favour of constant
  quality under the ceiling.

## 0.1.11

### Added

- Smart cut. With `--exact`, or with an overlay or burned-in subtitle
  shown for part of the time, an H.264 source is no longer re-encoded
  whole: its packets are copied wherever nothing changes and only the
  frames from a cut to the next keyframe, or under the overlay, are
  encoded into the same stream (system x264, CRF 18, parameter sets
  side by side with the source's). The report calls the mode `smart`
  and counts the frames copied and encoded; copied frames are
  bit-identical to the source. The clips' own audio is copied as coded
  when the container takes it (exact at the start of the file, within
  half a packet at a join), so nothing is decoded outside the encoded
  runs. The planner reads only the packets around each clip's range,
  not the whole file. A one-second overlay on a ten-second 1080p clip:
  3.0 s instead of 4.6 s; an exact trim inside a one-second GOP
  re-encodes at most a second.

### Changes

- A picture fitted onto a larger frame of one color, as a landscape
  video on the portrait canvas `--for instagram` or `--for tiktok`
  builds, takes the direct path: the frame is cleared to the background
  once and each decoded picture is scaled to its place, on several
  threads, instead of every frame passing through the compositor. A 5 s
  720p clip onto a 1080×1920 canvas takes 1.2 s where it took 5.7 s
  (ffmpeg: 1.4 s).
- `--fit cover` on `convert` and `resize` also decides how the picture
  meets the canvas `--for` builds: a landscape video on a 9:16 target is
  cropped to its center at the source's full height instead of getting
  bars. The crop takes the direct path as well (0.4 s for a 5 s 720p
  clip; ffmpeg 0.4 s), and the N410 note says which of the two was done.
- The verbs keep the source's audio sample rate and channel count when
  every input agrees, instead of writing 48 kHz stereo: a mono 44.1 kHz
  track extracted to WAV is the same size as ffmpeg's now.
- Releases carry the tag's section of this changelog as their notes; a
  `release-notes` workflow sets them on an existing release.
- `check.sh` resizes to 720p as well when the input is taller, and
  also exercises burned-in subtitles with `--fit`, CSS
  shorthands in a text source, `geneva targets`, `--for tiktok` and
  `--for email --budget`, and compares the burn-in and the portrait
  canvas with ffmpeg.

### Fixes

- `output.audio.channels: 1` was documented but every file was written
  as stereo; mono is now written as mono, downmixed from the stereo mix.

## 0.1.10

### Added

- `--for TARGET` on every verb and on `render`: encode for a destination
  (phone, tablet, desktop, tv, web, youtube, instagram, tiktok, x,
  linkedin, email). From one table and the source's facts it picks a
  size ceiling (never upscaling; a 9:16 canvas for Reels and TikTok),
  H.264 with the level the size needs, a quality tier (`--quality best`,
  `good`, `eco`), a bitrate cap, keyframes every 2 s, fast start and the
  audio bitrate, writes them into the explicit encode block, and reports
  every choice (N410). Platform limits become warnings (W411 to W414);
  `--budget SIZE` caps the bitrate to a file size. `geneva targets`
  prints the table with the source and date of each platform's numbers.
- Encode block fields `keyframe_interval`, `max_bitrate_kbps`, `level`
  and `fast_start`, wired through every encoder.
- CSS shorthand strings where an object also is: `shadow` as
  `"0 2px 8px #0008"`, `outline` as `"2px black"`, `padding` as `"8px"`,
  and the `font` shorthand `"italic 600 40px/1.2 Inter"`, whose parts fill
  in size, weight, italic and line height. Same structures underneath;
  the object form stays canonical. Still format 0.1.

### Changes

- MP4, MOV and M4A files carry their index at the front by default
  (`encode.fast_start`), for copied and encoded outputs alike.

## 0.1.9

### Added

- `geneva subtitles --burn FILE` draws the cues of an SRT or WebVTT file
  into the picture: `--position` (bottom, top, center), `--margin`, and
  `--style` as a JSON object of text fields over a default look sized to
  the frame. Overlapping cues go on further layers. Every cue is laid out
  before rendering; one that leaves the frame is warning W403, one
  outside the title-safe area (`--safe`, 5% by default) is note N404.
  `--fit` shrinks such cues in steps until they fit, down to half size,
  and reports each one (N405).
- A word longer than a text line now breaks inside the word instead of
  running past the edge, in every text source.

### Fixes

- Overlays on an untouched video (burned-in subtitles, a logo, a lower
  third) no longer send every frame through the compositor. The decoded
  frames go straight to the encoder; only the overlays are drawn, onto
  a transparent frame the size of their box, and laid over the decoded
  planes where they have coverage. Pixels outside stay byte-identical.
  Text that does not change with time is laid out once per clip. Burning
  three short cues into 70 s of SD video on four cores: 17.3 s to 8.3 s,
  against ffmpeg's 7.7 s with the same x264 settings.
- Image sequences (PNG) from an 8-bit YCbCr source take the direct path:
  the scaler applies the source's matrix and a table re-encodes its
  transfer curve, instead of the compositor's floating-point conversion.
  One second of 1080p to PNG went from 1.6 s to ffmpeg's speed.

## 0.1.8

### Changes

- Software H.264 uses the system's x264 when its library is installed
  (`libx264.so.NNN` from the distribution, Homebrew's dylib on macOS),
  ahead of the bundled OpenH264 and after any hardware encoder. Nothing of
  x264 is in the binary; it is loaded at run time through the parts of
  its interface that are the same across builds 155 to 175. The notes of
  a render say which encoder was used. `GENEVA_X264=off` turns the lookup
  off; a path names the library. On four cores, a 70 s SD resize: 5.7 s
  and 5.1 MB with x264 against 6.5 s and 4.7 MB for ffmpeg's libx264,
  where OpenH264 wrote 11 MB.

## 0.1.7

### Fixes

- `geneva audio --extract` re-encoded the audio of files at fractional
  frame rates (29.97, 59.94) instead of copying it: the verb's compiled
  timeline printed the output duration with six decimals, which no longer
  matched the clip's exact length. The extract now takes its length from
  the clip, and the copy planner accepts a duration within a millisecond
  of the clip's, so hand-written timelines get the same treatment.
- The VideoToolbox H.264/H.265 encoders ignored the quality setting and
  ran at their default bitrate, writing files up to four times larger
  than ffmpeg at the same `-q:v`; the quality is now passed the way
  ffmpeg stores it.

## 0.1.6

### Fixes

- The audio track was mixed and encoded after the last video frame, which
  added about a second per minute of AAC to every render; it is now
  encoded on its own thread while the frames flow, and the encoder thread
  interleaves the packets. A 70 s SD resize on four cores went from
  6.7 s to 4.8 s.
- `check.sh` shows whether a "copied" step really copied ("ok, copied" or
  "ok, RE-ENCODED") and the size of ffmpeg's output next to geneva's.

## 0.1.5

### Changes

- Last release with an Intel macOS archive. Apple stopped selling Intel Macs in 2023
  and macOS 26 is the last release for them; the build was untested and
  gated every release on the slowest runner. Intel Macs build from source.

### Fixes

- macOS archives were built against media libraries from before MXF and
  PNG support, because the build cache kept the media bindings compiled
  earlier; DNxHR, MXF and image sequences failed on the Mac with "not
  known" and "not available" errors. The cache is now keyed to the media
  libraries and the bindings are rebuilt whenever the libraries are.
- VP9 on Apple silicon and Intel Macs ran several times slower than
  ffmpeg: libvpx's configure does not recognize macOS 15 and fell back to
  a build without NEON or SSE. The target is now spelled out.
- Verbs keep the source's color encoding (see `docs/cli.md`), so resizing
  or converting standard-definition material no longer converts it to
  BT.709 through the compositor; it takes the direct path, tagged as the
  source was.
- A resize whose even output size is up to two pixels off the exact fit
  (854×480 at height 360 is 640.5 wide, written as 642) takes the direct
  path as well.

## 0.1.4

### Fixes

- On macOS, a release archive fetched with a browser carries the
  quarantine flag on everything it unpacks, and Gatekeeper refuses the
  unsigned binary in place even after `install.sh` has installed a clean
  copy. `install.sh` now clears the flag on the unpacked folder as well,
  and `check.sh` clears it on the binary it runs.

## 0.1.3

### Performance

- A resize, or a change of codec that needs another sample layout (ProRes
  and DNxHR take 10-bit 4:2:2), no longer passes through the compositor
  when the picture is otherwise untouched: frames are scaled and repacked
  straight from the decoder to the encoder in their coded encoding. On a
  four-core machine a 720p to 360p resize went from 3.4 s to 0.8 s for 10 s
  of video (ffmpeg: 0.8 s) and ProRes HQ from 7.1 s to 4.1 s (ffmpeg: about
  the same).
- Decoders use every core as well; the decoder context had the same
  threading gap as the encoders.

## 0.1.2

### Fixes

- Writing the audio of a long output no longer slows to a crawl after the
  last frame: samples were shifted through the pending buffer once per
  encoder frame, which is quadratic in the length of the audio. A seven
  minute join now finishes seconds after its last frame instead of minutes.
- ProRes, DNxHR, PNG and Motion JPEG encode on all cores; the encoder
  context did not allow frame or slice threading, so FFmpeg's own encoders
  ran on one thread.
- A video whose picture could be copied but whose audio codec the
  container refuses (AAC into MXF, for one) is rendered instead of failing
  at muxing.
- `render` prints a "mixing audio" line so a long job shows what it is
  doing after the last frame.

### Installing

- Release archives include `install.sh`, which puts `geneva` on the PATH
  (and clears the macOS quarantine flag), and `check.sh`, which runs the
  everyday commands on one machine and prints a timing table with ffmpeg
  alongside when it is installed.
- The release workflow signs and notarizes the macOS binaries when the
  Apple signing secrets are configured.

## 0.1.1

### Codecs and containers

- ProRes (422 Proxy, LT, 422, HQ, 4444, 4444 XQ) and DNxHR (LB, SQ, HQ,
  HQX, 444) video, chosen with `encode.video.profile` or `--profile`; the
  encoder now packs 10-bit 4:2:2 and 4:4:4 as well as 8-bit layouts.
- PNG and Motion JPEG video, and image sequences: an output path with a
  numbered pattern such as `frames/%04d.png` writes one file per frame.
- MP3, Vorbis, ALAC, AC-3 and 24-bit PCM audio; MP3 as an audio-only
  container.
- MXF, defaulting to DNxHR HQ with 24-bit PCM.

### Subtitles

- Subtitle assets (`.srt`, `.vtt`) and `subtitles` tracks in the timeline,
  written as text streams (3GPP timed text in MP4 and MOV, SubRip in MKV,
  WebVTT in WebM) with language and title.
- `geneva subtitles --add` attaches subtitle files to a video, `--extract`
  writes a subtitle stream out as SRT or WebVTT; `probe` lists subtitle
  streams.

### Fixes

- Opus and Vorbis sources decoded short: the sample-rate converter's
  output was sized from the first, shorter frame and every later frame was
  capped to it.
- A demuxer's transient "try again" was taken as the end of the stream.

### Diagnostics

- E421: a codec profile that does not belong to the chosen codec.

## 0.1.0

First release.

### Timeline format 0.1

- JSON documents with a versioned `geneva` field, validated against a
  published JSON Schema (`geneva schema`, `schema/`).
- Layers of clips with solid, shape, image, video, text and nested
  composition sources; audio tracks with gain and fades.
- Exact rational time; times as seconds, milliseconds, frames or timecode.
- Keyframed values with CSS easing names, cubic Béziers and springs.
- Transforms (anchor, position, scale, rotation), fit modes, opacity, blend
  modes computed in linear light, crossfade transitions.
- Color tags per asset and per output, with documented inference for
  untagged material.
- Diagnostics with stable codes, JSON pointers and suggested fixes.

### Commands

- `validate`, `frame`, `render`, `probe`, `schema`.
- `trim`, `concat`, `convert`, `resize`, `overlay`, `audio`, each building
  a timeline and rendering it through the same engine; `--show-timeline`
  prints that timeline.
- `--format json` on every command for programs and agents.

### Engine

- CPU reference renderer in premultiplied linear light with supersampled
  edges and bit-exact pixel-aligned images; row-parallel on a thread pool.
- Text layout and shaping through cosmic-text and swash, with font assets
  and system fallback.
- Media through statically linked, trimmed media libraries: H.264 (OpenH264
  in software; NVENC and VideoToolbox in hardware), H.265 (hardware), VP9,
  AV1; AAC, Opus, FLAC and PCM audio; MP4, MOV, MKV, WebM and audio-only
  M4A, Ogg, FLAC and WAV.
- Stream copy for trims and joins that leave the picture untouched, with
  keyframe-snapped cuts reported and `--exact` to re-encode instead; when
  a re-encode is needed anyway, decoded frames go straight to the encoder.
- Golden-frame test harness with perceptual comparison.

### Platforms

- Prebuilt binaries for Linux x86_64 and arm64 and macOS arm64 and x86_64.
  Windows builds from source without media support.
