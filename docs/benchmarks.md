# Benchmarks

How long geneva takes against the two ways people put HTML and CSS
graphics on video today: a headless browser screenshotting each frame for
ffmpeg to lay over the footage (Playwright + ffmpeg), and
[Remotion](https://www.remotion.dev). Seven jobs, from a vertical social
clip to a four-minute subtitle burn to pure motion graphics with no
footage at all.

**In short:** wherever there is footage, geneva is 2 to 6 times quicker
than both and uses 2 to 5 times less CPU than Remotion. On full-screen
motion graphics with no footage, Remotion is quicker.

Contents: [the jobs](#the-jobs), [results](#results),
[what it says](#what-it-says), [how it was run](#how-it-was-run),
[the smaller test](#the-smaller-test), [caveats](#caveats),
[running it yourself](#running-it-yourself).

## The jobs

Footage is [Tears of Steel](https://mango.blender.org) (CC-BY 3.0,
(C) Blender Foundation), cut from the 1080p release once per job. Captions
are the film's own English subtitles; where a job lights the word being
said, each cue's time is shared out over its words by length, which is
what a transcript without word timings gives.

| Job | Output | Length | What is drawn | Graphics on screen |
| --- | --- | --- | --- | --- |
| [Social clip](#social-clip) | 1080x1920, 24 fps | 60 s | the film blurred to fill the frame under the film fitted, word-lit captions | every frame |
| [Broadcast package](#broadcast-package) | 1920x800, 24 fps | 3 min | intro card, three wipes, four lower thirds, the film's subtitles | about a quarter of the time |
| [Static captions](#static-captions) | 1920x800, 24 fps | 4 min | outlined subtitles, 51 cues | 37% of the time |
| [Dynamic captions](#dynamic-captions) | 1920x800, 24 fps | 90 s | word-lit captions, three name cards, a LIVE badge with a pulse and a sheen | every frame (the badge) |
| [Opening](#opening) | 960x540, 30 fps | 13 s | [`examples/opening.html`](../examples/opening.html), 68 keyframe rules, then footage fading in | the first 9.7 s |
| [Intro](#intro) | 1920x1080, 30 fps | 14 s | blurred colour blobs in `screen`, rotating outlines, sweeping bars, a title rising letter by letter out of a blur; lifts away into footage | the first 10 s, full frame |
| [Kinetic typography](#kinetic-typography) | 1920x1080, 30 fps | 16 s | four phrases on panels wiping in through `clip-path`, each word with its own motion, gradient letters spinning in | every frame, full frame |

Each picture below shows the same moments from all three tools, left to
right: **Playwright + ffmpeg, Remotion, geneva**. They were compared
before anything was timed, so each tool was timed drawing the same video.

### Social clip

![The social clip at one moment from each tool: a blurred copy of the film filling a vertical frame, the film fitted in the middle, a caption with one word in yellow](benchmarks/social.jpg)

### Broadcast package

![Three moments of the broadcast package from each tool: the intro card fading out, a lower third, a frame of the film](benchmarks/broadcast.jpg)

### Static captions

![Two outlined subtitles from each tool, over the film](benchmarks/captions.jpg)

### Dynamic captions

![Three moments of the dynamic captions job from each tool: a name card top left, the LIVE badge top right, a caption with one word in cyan](benchmarks/dynamic.jpg)

### Opening

![Five moments of the opening from each tool, from the title to the footage](benchmarks/opening.jpg)

### Intro

![Four moments of the intro from each tool: the title rising, the title with its rule and chips, the card lifting away, the footage behind it](benchmarks/intro.jpg)

### Kinetic typography

![Seven moments of the kinetic typography piece from each tool, one row per phrase and wipe](benchmarks/kinetic.jpg)

## Results

Wall time and the CPU time the whole machine spent, median of three
rounds. Every tool encodes x264 at CRF 23, preset `medium`, 4:2:0.

| Job | geneva | Playwright + ffmpeg | Remotion |
| --- | --- | --- | --- |
| Social clip, 60 s | **64.9 s** (220 s CPU) | 137.7 s (317 s) | 280.2 s (1057 s) |
| Broadcast package, 3 min | **56.7 s** (180 s) | 242.6 s (583 s) | 343.2 s (1199 s) |
| Static captions, 4 min | **100.3 s** (337 s) | 307.8 s (823 s) | 469.5 s (1665 s) |
| Dynamic captions, 90 s | **61.4 s** (222 s) | 189.0 s (390 s) | 175.5 s (621 s) |
| Opening, 13 s | **15.6 s** (41 s) | 64.0 s (86 s) | 20.6 s (53 s) |
| Intro, 14 s | 76.2 s (198 s) | 147.9 s (236 s) | **61.8 s** (172 s) |
| Kinetic typography, 16 s | 64.1 s (170 s) | 65.5 s (98 s) | **34.6 s** (68 s) |

The geneva column is geneva with its defaults. With the encoder pinned to
the other tools' settings, which also turns its frame copying off, the
broadcast package takes 109.2 s and the static captions 146.3 s; the
other jobs are within two seconds of the figures above.

How much quicker geneva is, on wall time:

| Job | than Playwright + ffmpeg | than Remotion |
| --- | --- | --- |
| Social clip | 2.1x | 4.3x |
| Broadcast package | 4.3x | 6.1x |
| Static captions | 3.1x | 4.7x |
| Dynamic captions | 3.1x | 2.9x |
| Opening | 4.1x | 1.3x |
| Intro | 1.9x | 0.8x (Remotion 1.2x quicker) |
| Kinetic typography | 1.0x | 0.5x (Remotion 1.9x quicker) |

Every run is in [`benchmarks/real-world/results`](../benchmarks/real-world/results/);
the spread between rounds was a few percent.

## What it says

- **With footage, geneva is 2 to 6 times quicker.** The browser routes
  take a screenshot of every frame that has graphics on it and encode
  separately; geneva decodes, draws and encodes in one pass, compositing only
  where the graphics are, and with its defaults copies the
  stretches the graphics leave alone. That copying is why the longest,
  sparsest jobs show the widest gaps: the broadcast package takes 56.7 s
  with it and 109.2 s without.
- **Without footage, Remotion is quicker.** On the kinetic typography
  piece, 190-pixel type over full-frame `clip-path` panels on every frame,
  Chromium's painter does the work with less than half of geneva's CPU.
  The intro is closer (61.8 s against 76.2 s). The opening, also pure
  markup but smaller and simpler per frame, still goes geneva's way. These
  runs used geneva's CPU renderer; its GPU renderer was not measured.
- **Playwright + ffmpeg sits between the two.** It captures one frame at
  a time in one tab, so it is slowest on motion graphics, where Remotion's
  four tabs pay off, and quicker than Remotion on footage, where Remotion
  screenshots every frame whatever is on it.

## How it was run

- **geneva** 1.0.0 with the fixes on `main` at the time of the run,
  release build, the system's x264 (build 164), CPU renderer.
- **Playwright + ffmpeg**: Playwright 1.56.1 with Chromium's headless
  shell (build 1194) screenshots each frame that has graphics on it, with
  every CSS animation paused and set to the frame's time. Frames with
  nothing on them reuse one blank picture, so they cost no screenshot,
  which is the kindest setup for this route. ffmpeg 6.1.1 then lays the
  pictures over the footage and encodes; for the social clip it also
  builds the blurred fill.
- **Remotion** 4.0.526 on the same Chromium, set up the way that was
  quickest in an earlier test: bundled once, JPEG frames, four tabs
  (`--concurrency 4`). Its documentation says not to drive animation with
  CSS keyframes, so the broadcast, social and caption graphics were
  ported to `interpolate()` on the frame number, using the same curves
  as the HTML files. The three motion pieces (opening, intro, kinetic)
  have dozens of keyframe rules each, so they were mounted unchanged and
  every CSS animation was paused and set to the frame's time before each
  frame was taken, which gives the same picture whatever order frames are
  rendered in.
- **The machine**: one shared 4-core Linux container, CPU only, nothing
  else running. Three rounds, the tools taking turns within each round.
  CPU is the machine's busy time over each run, read from `/proc/stat`,
  because the shell's own timer misses processes Chromium starts.

## The smaller test

The name card over nine seconds of 720p footage in the
[README](../README.md), timed the same way:

| Job | geneva | Playwright + ffmpeg | Remotion, bundled once |
| --- | --- | --- | --- |
| Name card and word captions | **4.6 s** (15 s CPU) | 18.5 s (32 s) | 18.0 s (54 s) |
| Name card alone | **3.2 s** (9 s) | 11.0 s (22 s) | 18.8 s (56 s) |

A browser tool's fixed start-up cost is part of that. Timed on one frame,
it is 0.1 s for geneva, 0.6 s for Playwright and 2.1 s for Remotion, so
the per-frame costs on the captioned job are about 17 ms, 66 ms and 59 ms,
and the ratios are not a start-up effect.

## Caveats

- **One machine.** The seconds are this 4-core machine's; the ratios are
  the finding. A dedicated machine, and a GPU, would move all three
  columns.
- **Remotion is often spread over many machines.** Its usual deployment
  renders across cloud functions, which cuts wall time and adds work; the
  CPU figures are the part that does not change.
- **The ports are this harness's.** The Remotion compositions and the
  Playwright pages were written for this test and checked by eye against
  each other, not by a pixel metric. A hand port of the three motion
  pieces could be quicker or slower than setting CSS animations frame by
  frame.
- **Colour tags differ.** Remotion's JPEG path writes full-range video
  tagged BT.470BG, Playwright's ffmpeg command leaves the colour space
  untagged, geneva writes BT.709. That changes how a player shows the
  file, not how long it took to make.

## Running it yourself

Everything is in [`benchmarks/real-world`](../benchmarks/real-world/):
the geneva documents, the graphics as HTML, the Playwright pages, the
Remotion projects and the scripts. The film is not in the repository;
`fetch.sh` downloads it.

```sh
cd benchmarks/real-world
./fetch.sh                                   # Tears of Steel and its subtitles into src/
python3 derive-captions.py                   # optional: rebuilds the committed caption files
set1/prepare.sh && set2/prepare.sh           # cuts the film, installs Remotion and Playwright
(cd set1/pw && npx playwright install chromium) && (cd set2/pw && npx playwright install chromium)
GENEVA=geneva set1/bench.sh 3                # social, broadcast, opening; three rounds
GENEVA=geneva set2/bench.sh 3                # intro, kinetic, captions, dynamic
```

It needs Linux (for `/proc/stat`), Node 22, ffmpeg with `libx264`, and
`bc`. Set `CHROME` to a Chromium binary to have Remotion use it rather
than the one it downloads. Each run adds a line to `results.txt` in its
set: tool, job, round, wall seconds and CPU seconds. `out/` keeps the
last video from each tool.
