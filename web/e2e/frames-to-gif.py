"""Turn a recorded guide shot (guide-shots.mjs, "record") into an animated GIF or WebP.

    python web/e2e/frames-to-gif.py <shot>.frames out.gif [--width 1280] [--fps 12] [--colors 256]

The frames come with their screencast times (frames.json); they are resampled at --fps, identical frames are merged
into one longer frame, and every frame shares one palette (no colour flicker between frames). A .webp output is an
animated WebP instead (smaller, full colour). Needs Pillow (pip install pillow).
"""

import argparse
import json
from pathlib import Path

from PIL import Image


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("frames", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--width", type=int, default=1280)
    ap.add_argument("--fps", type=float, default=12)
    ap.add_argument("--colors", type=int, default=256)
    a = ap.parse_args()

    meta = json.loads((a.frames / "frames.json").read_text())
    end = meta[-1]["t"] + 1.0 / a.fps
    times = [k / a.fps for k in range(int(end * a.fps) + 1)]
    # The frame shown at each tick is the last one captured at or before it.
    picks, j = [], 0
    for t in times:
        while j + 1 < len(meta) and meta[j + 1]["t"] <= t:
            j += 1
        picks.append(j)

    cache = {}

    def frame(k):
        if k not in cache:
            im = Image.open(a.frames / meta[k]["file"]).convert("RGB")
            if im.width > a.width:
                im = im.resize((a.width, round(im.height * a.width / im.width)), Image.LANCZOS)
            cache[k] = im
        return cache[k]

    # Merge runs of the same frame into one longer frame.
    runs = []
    for k in picks:
        if runs and runs[-1][0] == k:
            runs[-1][1] += 1
        else:
            runs.append([k, 1])
    ms = 1000.0 / a.fps
    durations = [round(n * ms) for _, n in runs]
    images = [frame(k) for k, _ in runs]

    if a.out.suffix.lower() == ".webp":
        images[0].save(a.out, save_all=True, append_images=images[1:], duration=durations, loop=0, quality=80, method=6)
    else:
        # One palette for all frames, from a strip of up to 24 of them.
        sample = images[:: max(1, len(images) // 24)][:24]
        strip = Image.new("RGB", (sample[0].width, sample[0].height * len(sample)))
        for i, im in enumerate(sample):
            strip.paste(im, (0, im.height * i))
        palette = strip.quantize(colors=a.colors, method=Image.Quantize.MEDIANCUT)
        frames = [im.quantize(palette=palette, dither=Image.Dither.NONE) for im in images]
        frames[0].save(a.out, save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=False, disposal=1)
    total = sum(durations) / 1000
    print(f"{a.out}: {len(images)} frames, {total:.1f} s, {a.out.stat().st_size / 1e6:.2f} MB")


if __name__ == "__main__":
    main()
