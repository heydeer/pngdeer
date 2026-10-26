"""Compares TinyPNG and a size-matched pngquant output on UI screenshots: palette, flat-color fidelity,
dither noise by region type, and where the error concentrates. Writes crop montages for the worst tiles."""

import csv
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, "bench")
import bench  # noqa: E402

R = Path("bench/results/latest")
OUT = Path(sys.argv[1])
IMAGES = sys.argv[2].split(",")
rows = list(csv.DictReader(open(R / "results.csv")))


def load(path: Path) -> np.ndarray:
    with tempfile.TemporaryDirectory() as t:
        p = Path(t) / "x.png"
        bench.strip_color_chunks(path, p)
        return np.asarray(Image.open(p).convert("RGBA")).astype(np.int16)


def key(a: np.ndarray) -> np.ndarray:  # pack RGBA into one int per pixel
    a = a.astype(np.int64)
    return (a[..., 0] << 24) | (a[..., 1] << 16) | (a[..., 2] << 8) | a[..., 3]


def neighbors_equal(k: np.ndarray) -> np.ndarray:
    """True where the 3x3 neighbourhood has a single value."""
    eq = np.ones(k.shape, bool)
    p = np.pad(k, 1, mode="edge")
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            eq &= p[1 + dy : 1 + dy + k.shape[0], 1 + dx : 1 + dx + k.shape[1]] == k
    return eq


def local_range(y: np.ndarray) -> np.ndarray:
    p = np.pad(y, 1, mode="edge")
    stack = [p[1 + dy : 1 + dy + y.shape[0], 1 + dx : 1 + dx + y.shape[1]] for dy in (-1, 0, 1) for dx in (-1, 0, 1)]
    s = np.stack(stack)
    return s.max(0) - s.min(0)


for img in IMAGES:
    by = {r["pipeline"]: r for r in rows if r["image"] == img}
    tiny = by["tinypng"]
    tb = int(tiny["out_bytes"])
    pq_name = min((n for n in by if n.startswith("pq")), key=lambda n: abs(int(by[n]["out_bytes"]) - tb))
    pq = by[pq_name]
    o = load(Path("bench/images") / img)
    outs = {"tinypng": load(R / "tinypng" / img), pq_name: load(R / pq_name / img)}
    ko = key(o)
    y = (0.299 * o[..., 0] + 0.587 * o[..., 1] + 0.114 * o[..., 2]).astype(np.float32)
    flat = neighbors_equal(ko)
    rng = local_range(y)
    edge = ~flat & (rng >= 24)
    smooth = ~flat & ~edge
    regions = {"flat": flat, "smooth": smooth, "edge": edge}
    vals, counts = np.unique(ko, return_counts=True)
    order = np.argsort(-counts)
    top = vals[order[:32]]
    print(f"\n=== {img}  {o.shape[1]}x{o.shape[0]}  original unique colors {len(vals):,}; top-16 colors cover {counts[order[:16]].sum() / ko.size:.1%}, top-64 {counts[order[:64]].sum() / ko.size:.1%}")
    print("    region share: " + ", ".join(f"{n} {m.mean():.1%}" for n, m in regions.items()))
    err_maps = {}
    for name, out in outs.items():
        r = by[name]
        kout = key(out)
        pal = np.unique(kout)
        err = np.abs(out - o)[..., :3].mean(-1)
        err_maps[name] = err
        exact = kout == ko
        # dither noise: flat original neighbourhood but output not flat
        noise = flat & ~neighbors_equal(kout)
        top_exact = np.mean([np.isin(c, pal) for c in top])
        top_px = np.isin(ko, top)
        print(f"  {name:10s} {int(r['out_bytes']):>9,}B score {float(r['ssimulacra2']):5.1f} | palette {len(pal):3d} | top-32 colors in palette {top_exact:.0%}, their pixels exact {exact[top_px].mean():.1%} | all pixels exact {exact.mean():.1%}")
        for n, m in regions.items():
            extra = f" noisy {noise[m].mean():.1%}" if n == "flat" else ""
            print(f"      {n:6s} exact {exact[m].mean():6.1%}  mean|Δ| {err[m].mean():5.2f}{extra}")
    # tiles where pngquant is much worse than TinyPNG
    T = 48
    h, w = o.shape[:2]
    hh, ww = h // T, w // T
    def tiles(e):
        return e[: hh * T, : ww * T].reshape(hh, T, ww, T).mean((1, 3))
    diff = tiles(err_maps[pq_name]) - tiles(err_maps["tinypng"])
    worst = np.dstack(np.unravel_index(np.argsort(-diff, axis=None)[:4], diff.shape))[0]
    crops = []
    for ty, tx in worst:
        y0, x0 = max(0, ty * T - T // 2), max(0, tx * T - T // 2)
        box = (x0, y0, x0 + 2 * T, y0 + 2 * T)
        trio = [o, outs["tinypng"], outs[pq_name]]
        row = Image.new("RGB", (3 * 2 * T * 3 + 20, 2 * T * 3 + 14), (255, 255, 255))
        for i, a in enumerate(trio):
            c = Image.fromarray(a[box[1] : box[3], box[0] : box[2]].astype(np.uint8), "RGBA").convert("RGB").resize((2 * T * 3, 2 * T * 3), Image.NEAREST)
            row.paste(c, (i * (2 * T * 3 + 10), 14))
        ImageDraw.Draw(row).text((2, 1), f"{img} @({x0},{y0})  original | tinypng | {pq_name}", fill=(0, 0, 0))
        crops.append(row)
    sheet = Image.new("RGB", (crops[0].width, sum(c.height for c in crops)), (255, 255, 255))
    yy = 0
    for c in crops:
        sheet.paste(c, (0, yy)); yy += c.height
    name = img.replace("/", "_")
    sheet.save(OUT / f"crops_{name}")
    print(f"    worst tiles (pq err - tiny err): " + ", ".join(f"({tx * T},{ty * T}) +{diff[ty, tx]:.1f}" for ty, tx in worst))
