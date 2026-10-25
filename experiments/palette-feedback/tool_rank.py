"""python3 experiments/palette-feedback/tool_rank.py RUN IMAGES (e.g. bench/results/tools-main-... bench/images): per tool (all its pipelines pooled into one size-quality frontier), how often it
reaches TinyPNG's quality and its score at TinyPNG's file size, over all images and opaque ones only."""
import csv, math, statistics, sys, collections, re
from pathlib import Path
from PIL import Image
import numpy as np
run, images = Path(sys.argv[1]), Path(sys.argv[2])
rows = [r for r in csv.DictReader(open(run / "results.csv")) if r["out_bytes"] and r["ssimulacra2"]]
by = collections.defaultdict(dict)
for r in rows:
    by[r["image"]][r["pipeline"]] = (int(r["out_bytes"]), float(r["ssimulacra2"]))
opaque = {img for img in by if (lambda a: a.shape[-1] < 4 or (a[..., 3] == 255).all())(np.asarray(Image.open(images / img).convert("RGBA")))}
def tool(p):
    if p.startswith("pd") or p.startswith("liq"): return "pngdeer (ours)"
    if p.startswith("pq"): return "pngquant"
    m = re.match(r"(im|ff|pil|exq|qte|nq|qzr)\d", p)
    return {"im": "ImageMagick", "ff": "FFmpeg", "pil": "Pillow", "exq": "exoquant", "qte": "quantette", "nq": "NeuQuant", "qzr": "quantizr"}[m.group(1)] if m else None
def frontier(pts):
    pts = sorted(pts); out = []
    for b, s in pts:
        if not out or s > out[-1][1]: out.append((b, s))
    return out
def at_size(front, b):  # score at file size b along the frontier (None outside its range)
    if b < front[0][0] or b > front[-1][0] * 1.0: return front[-1][1] if b > front[-1][0] else None
    for (b1, s1), (b2, s2) in zip(front, front[1:]):
        if b1 <= b <= b2:
            f = math.log(b / b1) / math.log(b2 / b1) if b2 > b1 else 0
            return s1 + f * (s2 - s1)
    return front[0][1]
tools = sorted({t for p in {r["pipeline"] for r in rows} if (t := tool(p))})
print(f"{run.name}: {len(by)} images, {len(opaque)} opaque")
print(f"{'tool':12s} | {'all: reach':>11s} {'score@Tiny size':>15s} | {'opaque: reach':>13s} {'score@Tiny size':>15s}")
res = []
for t in tools:
    out = []
    for subset in (set(by), opaque):
        reach, diffs = 0, []
        for img in subset:
            tb, ts = by[img]["tinypng"]
            front = frontier([v for p, v in by[img].items() if tool(p) == t])
            if not front: continue
            reach += front[-1][1] >= ts
            s = at_size(front, tb)
            if s is not None: diffs.append(s - ts)
        out.append((reach, len(subset), statistics.median(diffs) if diffs else float("nan"), len(diffs)))
    res.append((out[0][2], t, out))
for _, t, out in sorted(res, reverse=True):
    (r1, n1, d1, c1), (r2, n2, d2, c2) = out
    print(f"{t:12s} | {r1:4d}/{n1:<4d} {d1:+11.1f} ({c1}) | {r2:6d}/{n2:<4d} {d2:+11.1f} ({c2})")
