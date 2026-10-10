#!/usr/bin/env python3
"""Writes the caption benchmark's timelines.

Each job is one timeline: a source video as the bottom layer and one
`html` clip per caption, the markup inline, as a caption generator
writes them. The captions are two lines of about thirteen words, on
screen 2.5 s out of every 2.7 s, from a fixed list of sentences, so
every run of a job draws the same thing.

    make-jobs.py OUT_DIR --video src.mp4 --size 1920x800 --fps 24 \\
                 --seconds 120 [--jobs still,steps,blur-in,stroke,frosted,glow]

Lengths in the markup are written for 1920 wide and scaled with the
output's width, as a generator writes them for the size it renders.
With --native, the same captions are also written as an .srt
(NAME.srt) and a word file (NAME.words.json) for `subtitles --burn`.

A video shorter than the job is laid end to end as often as it takes;
give its length with --video-seconds. Without --video the bottom layer
is a solid, which is what the many-clips job uses to measure geneva
rather than the decoder.
"""

import argparse
import json
import os
import re

WORDS = (
    "we shipped the new build on friday and nobody noticed until the "
    "lights went out across the whole of the east side of the city "
    "which is when the phones started ringing and did not stop for "
    "three days while the team took turns sleeping under the desks"
).split()

EVERY = 2.7
SHOWN = 2.5
PER_LINE = 7


def lines(i):
    """Two lines of words for caption `i`, cycling through WORDS."""
    n = 2 * PER_LINE - (i % 3)
    start = (i * 5) % len(WORDS)
    words = [WORDS[(start + k) % len(WORDS)] for k in range(n)]
    return [words[:PER_LINE], words[PER_LINE:]]


def rows(i, word):
    return "".join(
        '<div class="row">' + "".join(word(r, k, w) for k, w in enumerate(line)) + "</div>"
        for r, line in enumerate(lines(i))
    )


def word_times(i):
    """Each word's moment in the caption, spread over its 2.5 s."""
    n = sum(len(l) for l in lines(i))
    return [0.1 + 2.2 * k / n for k in range(n)]


STYLE = {
    "still": (
        ".cap{position:absolute;left:8%;right:8%;bottom:7%;display:flex;flex-direction:column;"
        "align-items:center;gap:6px;font:400 36px Inter;color:#F4F0E8}"
        ".row{display:flex;gap:9px;padding:4px 14px;background:#000000b3;border-radius:8px}"
    ),
    "steps": (
        ".cap{position:absolute;left:8%;right:8%;bottom:7%;display:flex;flex-direction:column;"
        "align-items:center;gap:6px;font:600 36px Inter;color:#F4F0E8;"
        "text-shadow:0 1px 4px #000000cc}"
        ".row{display:flex;gap:9px}"
        "@keyframes lit{from{color:#F4F0E8}to{color:#FFD400}}"
    ),
    "blur-in": (
        "@keyframes arrive{from{opacity:0;filter:blur(11px);translate:0 3px}"
        "to{opacity:1;filter:blur(0px);translate:0 0}}"
        ".cap{position:absolute;left:8%;right:8%;bottom:7%;display:flex;flex-direction:column;"
        "align-items:center;gap:6px;font:400 36px Inter;color:#F4F0E8;"
        "text-shadow:0 1px 4px #000000cc,0 0 25px #0000004d}"
        ".row{display:flex;gap:9px}"
    ),
    "stroke": (
        ".cap{position:absolute;left:8%;right:8%;bottom:7%;display:flex;flex-direction:column;"
        "align-items:center;gap:6px;font:700 40px Inter;color:#fff;"
        "-webkit-text-stroke:0.1em #000;paint-order:stroke fill;"
        "text-shadow:0 2px 6px #000000aa,0 0 20px #00000066}"
        ".row{display:flex;gap:10px}"
    ),
    "frosted": (
        ".cap{position:absolute;left:8%;right:8%;bottom:7%;display:flex;flex-direction:column;"
        "align-items:center;gap:6px;font:400 36px Inter;color:#F4F0E8}"
        ".plate{display:flex;flex-direction:column;align-items:center;gap:4px;padding:10px 22px;"
        "background:#3A465659;border-radius:12px;backdrop-filter:blur(9px) saturate(115%)}"
        ".row{display:flex;gap:9px}"
    ),
    "glow": (
        "@keyframes glow{0%,100%{text-shadow:0 0 0px #FFD40000}"
        "50%{text-shadow:0 0 20px #FFD400cc}}"
        ".cap{position:absolute;left:8%;right:8%;bottom:7%;display:flex;flex-direction:column;"
        "align-items:center;gap:6px;font:600 36px Inter;color:#F4F0E8;"
        "text-shadow:0 1px 4px #000000cc}"
        ".row{display:flex;gap:9px}"
    ),
}


def scaled(css, k):
    """Every px length in `css` times `k`."""
    return re.sub(r"(\d+(?:\.\d+)?)px", lambda m: f"{float(m.group(1)) * k:g}px", css)


def caption(job, i, k=1.0):
    times = iter(word_times(i))
    if job == "still":
        body = rows(i, lambda r, k, w: f"<span>{w}</span>")
    elif job == "steps":
        body = rows(
            i,
            lambda r, k, w: f'<span style="animation:lit 0.01s steps(1,jump-end) {next(times):.3f}s both">{w}</span>',
        )
    elif job == "blur-in":
        body = rows(
            i,
            lambda r, k, w: f'<span style="animation:arrive 0.55s cubic-bezier(.2,.7,.2,1) {next(times) - 0.1:.3f}s both">{w}</span>',
        )
    elif job == "stroke":
        body = rows(i, lambda r, k, w: f"<span>{w}</span>")
    elif job == "frosted":
        body = '<div class="plate">' + rows(i, lambda r, k, w: f"<span>{w}</span>") + "</div>"
    elif job == "glow":
        body = rows(
            i,
            lambda r, k, w: f'<span style="animation:glow 0.56s ease-in-out {next(times):.3f}s both">{w}</span>',
        )
    return f'<style>{scaled(STYLE[job], k)}</style><div class="cap">{body}</div>'


def timeline(job, args, out_dir):
    w, h = map(int, args.size.split("x"))
    count = int(args.seconds / EVERY)
    assets = {"inter-400": {"src": "fonts/Inter-400.ttf"}, "inter-600": {"src": "fonts/Inter-600.ttf"},
              "inter-700": {"src": "fonts/Inter-700.ttf"}}
    if args.video:
        assets["video"] = {"src": os.path.relpath(args.video, out_dir)}
        length = args.video_seconds or args.seconds
        base, t = [], 0.0
        while t < args.seconds - 1e-6:
            d = min(length, args.seconds - t)
            base.append({"source": {"kind": "video", "asset": "video", "out": f"{d:.3f}s"}})
            t += d
    else:
        base = [{"source": {"kind": "solid", "color": "#334455"}}]
    clips = [
        {"source": {"kind": "html", "html": caption(job, i, w / 1920)},
         "start": f"{i * EVERY:.3f}s", "duration": f"{SHOWN}s"}
        for i in range(count)
    ]
    return {
        "geneva": "1.1",
        "output": {"width": w, "height": h, "fps": args.fps, "duration": f"{args.seconds}s"},
        "assets": assets,
        "layers": [{"id": "video", "clips": base}, {"id": "captions", "clips": clips}],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--video")
    ap.add_argument("--video-seconds", type=float)
    ap.add_argument("--size", default="1920x800")
    ap.add_argument("--fps", default=24, type=int)
    ap.add_argument("--seconds", default=120, type=float)
    ap.add_argument("--jobs", default="still,steps,blur-in,stroke,frosted,glow")
    ap.add_argument("--suffix", default="")
    ap.add_argument("--native", help="also write NAME.srt and NAME.words.json")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    for job in args.jobs.split(","):
        path = os.path.join(args.out, f"{job}{args.suffix}.json")
        with open(path, "w") as f:
            json.dump(timeline(job, args, args.out), f, indent=1)
        print(path)
    if args.native:
        native(args, args.native)


def native(args, name):
    """The captions as an .srt (two lines each) and as a word file with
    one segment per caption, so `--burn` groups the words the same way."""
    def stamp(t):
        ms = round(t * 1000)
        return f"{ms // 3600000:02}:{ms // 60000 % 60:02}:{ms // 1000 % 60:02},{ms % 1000:03}"
    cues, segments = [], []
    for i in range(int(args.seconds / EVERY)):
        start = i * EVERY
        ls = lines(i)
        cues.append(f"{i + 1}\n{stamp(start)} --> {stamp(start + SHOWN)}\n" + "\n".join(" ".join(l) for l in ls) + "\n")
        flat = [w for l in ls for w in l]
        times = word_times(i)
        words = []
        for k, (w, t) in enumerate(zip(flat, times)):
            end = times[k + 1] if k + 1 < len(times) else SHOWN
            words.append({"word": w, "start": round(start + t, 3), "end": round(start + end, 3)})
        segments.append({"words": words})
    with open(os.path.join(args.out, f"{name}.srt"), "w") as f:
        f.write("\n".join(cues))
    with open(os.path.join(args.out, f"{name}.words.json"), "w") as f:
        json.dump({"segments": segments}, f)


if __name__ == "__main__":
    main()
