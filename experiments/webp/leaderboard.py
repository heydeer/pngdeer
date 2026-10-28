"""python3 experiments/webp/leaderboard.py MAIN_RUN HOLDOUT_RUN (bench/results/wlb-*): the WebP leaderboard,
by the PNG one's method (experiments/palette-feedback/tool_rank.py) with TinyPNG's WebP as the yardstick.

Per tool, all its settings form one size-quality frontier per image (bench.py's `frontier`):
- Points: its score at TinyPNG's file size, minus TinyPNG's score (median). Only where TinyPNG's
  file is no smaller than the tool's smallest, so each tool's median covers other images; a last
  line gives the medians over the images all tools cover.
- Reaches: images where its best setting scores at least TinyPNG's.
- Size at TinyPNG's quality: its bytes there over TinyPNG's (median and total, where it reaches).

TinyPNG's WebP is the real one where it was fetched (bench/results/tiny-*: all 71 held-out images and
49 of the 109 tuning ones, the 120 WebP's settings were chosen on), the stand-in for the other 60
(bench/results/emu-*, tiny_emulate.py). Default settings (one file each) are compared directly."""
import csv, glob, re, statistics, sys
from collections import defaultdict
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "bench"))
from bench import bytes_at_score, frontier, score_at_bytes  # noqa: E402

TOOLS = {"pngdeer": r"wp\d+|wpauto", "cwebp": r"cw(af)?\d+|cwll", "pngquant + cwebp": r"pqw\d+(-d0)?"}
DEFAULTS = {"cwebp -q 75 (its default)": "cwd75", "cwebp -q 80": "cwd80", "pngdeer, automatic": "wpauto"}


def load(pattern, into, only=None):
    for f in glob.glob(pattern):
        for r in csv.DictReader(open(f)):
            if r["ssimulacra2"] and (only is None or r["pipeline"] in only):
                into[r["image"]][r["pipeline"]] = SimpleNamespace(out_bytes=int(r["out_bytes"]), ssimulacra2=float(r["ssimulacra2"]))


outputs = defaultdict(dict)
for run in sys.argv[1:3]:
    load(f"{run}/results.csv", outputs)
load("bench/results/tiny-*/results.csv", outputs, {"tinywebp"})
load("bench/results/emu-*/results.csv", outputs, {"tinyemu"})
groups = {
    "TinyPNG's WebP (120)": [o for o in outputs.values() if "tinywebp" in o and "wpauto" in o],
    "the stand-in (other 60)": [o for o in outputs.values() if "tinywebp" not in o and "tinyemu" in o and "wpauto" in o],
}


def median(xs):
    return statistics.median(xs) if xs else float("nan")


for title, images in groups.items():
    ref = "tinywebp" if "120" in title else "tinyemu"
    print(f"\n### Against {title}\n")
    print("| Tool | Settings | Points | Reaches | Size at its quality, median | total |\n|---|---:|---:|---:|---:|---:|")
    by_tool = {}
    for tool, pattern in TOOLS.items():
        names = {p for o in images for p in o if re.fullmatch(pattern, p)}
        if not names:
            continue
        points, reach, sized = {}, 0, []
        for i, o in enumerate(images):
            tiny = o[ref]
            front = frontier([r for p, r in o.items() if p in names])
            if not front:
                continue
            if (s := score_at_bytes(front, tiny.out_bytes)) is not None:
                points[i] = s - tiny.ssimulacra2
            if (b := bytes_at_score(front, tiny.ssimulacra2)) is not None:
                reach += 1
                sized.append((b, tiny.out_bytes))
        ratio = median([b / t for b, t in sized]) * 100
        total = sum(b for b, _ in sized) / sum(t for _, t in sized) * 100
        by_tool[tool] = points
        print(f"| {tool} | {len(names)} | {median(list(points.values())):+.1f} ({len(points)}) | {reach}/{len(images)} | {ratio:.1f}% | {total:.1f}% |")
    common = set.intersection(*(set(p) for p in by_tool.values()))
    print(f"\nPoints on the {len(common)} images all tools cover: " + ", ".join(f"{t} {median([p[i] for i in common]):+.1f}" for t, p in by_tool.items()))
    print("\n| One setting | Size, median | total | Score difference, median | Bigger and worse |\n|---|---:|---:|---:|---:|")
    for label, p in DEFAULTS.items():
        pairs = [(o[p], o[ref]) for o in images if p in o]
        if not pairs:
            continue
        ratio = median([a.out_bytes / b.out_bytes for a, b in pairs]) * 100
        total = sum(a.out_bytes for a, _ in pairs) / sum(b.out_bytes for _, b in pairs) * 100
        diff = median([a.ssimulacra2 - b.ssimulacra2 for a, b in pairs])
        worse = sum(a.out_bytes > b.out_bytes and a.ssimulacra2 < b.ssimulacra2 for a, b in pairs)
        print(f"| {label} | {ratio:.1f}% | {total:.1f}% | {diff:+.2f} | {worse}/{len(pairs)} |")
