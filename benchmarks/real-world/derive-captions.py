"""Rebuilds the caption files the jobs read from the film's subtitles.

The committed files under set1/media and set2/media are this script's
output; run it after fetch.sh to check them. Where a job lights words,
each cue's time is shared out over its words by length, which is what a
transcript without word timings gives.
"""
import json
import pathlib
import re

here = pathlib.Path(__file__).parent
text = (here / "src" / "TOS-en.srt").read_text(encoding="utf-8-sig")
cues = []
for block in re.split(r"\n\s*\n", text.strip()):
    lines = block.strip().splitlines()
    if len(lines) < 3:
        continue
    times = [int(h) * 3600 + int(m) * 60 + int(s) + int(ms) / 1000
             for h, m, s, ms in re.findall(r"(\d+):(\d+):(\d+)[,.](\d+)", lines[1])]
    cues.append((times[0], times[1], [line.strip() for line in lines[2:]]))


def stamp(t):
    return "%02d:%02d:%02d,%03d" % (t // 3600, t % 3600 // 60, int(t % 60), round((t % 1) * 1000) % 1000)


def srt(start, length, path, joiner="\n"):
    out = []
    for a, b, lines in cues:
        if b <= start or a >= start + length:
            continue
        a, b = max(a, start) - start, min(b, start + length) - start
        out.append(f"{len(out) + 1}\n{stamp(a)} --> {stamp(b)}\n" + joiner.join(lines) + "\n")
    path.write_text("\n".join(out))


def words(start, length, path):
    segments = []
    for a, b, lines in cues:
        if b <= start or a >= start + length:
            continue
        a, b = max(a, start) - start, min(b, start + length) - start
        line = " ".join(lines)
        total = sum(len(w) + 1 for w in line.split())
        t, ws = a, []
        for w in line.split():
            d = (b - a) * (len(w) + 1) / total
            ws.append({"word": " " + w, "start": round(t, 3), "end": round(t + d, 3)})
            t += d
        segments.append({"start": round(a, 3), "end": round(b, 3), "text": " " + line, "words": ws})
    path.write_text(json.dumps({"text": " ".join(s["text"] for s in segments), "segments": segments}, indent=1))


words(190, 60, here / "set1/media/social-words.json")
# The broadcast job ran with each cue on one line; the captions job kept
# the film's line breaks.
srt(0, 180, here / "set1/media/broadcast-subs.srt", joiner=" ")
srt(190, 240, here / "set2/media/captions.srt")
words(300, 90, here / "set2/media/dynamic-words.json")
print("wrote the four caption files")
