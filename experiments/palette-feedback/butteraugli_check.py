"""Scores a bench run's outputs with butteraugli (independent of SSIMULACRA2, which the search optimizes).

python3 experiments/palette-feedback/butteraugli_check.py RUN PIPELINES [IMAGES]: IMAGES is the run's
input directory (default bench/images). Writes <run>/butteraugli.csv: image, pipeline, max distance,
3-norm. Lower is better. Transparent images are composited over a dark and a light background and
the worse of the two is kept.
"""

import csv
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, "bench")
import bench  # noqa: E402

BUTTERAUGLI = "/opt/homebrew/opt/jpeg-xl/bin/butteraugli_main"
RUN = Path(sys.argv[1]).resolve()
PIPELINES = sys.argv[2].split(",")
IMAGES = Path(sys.argv[3]) if len(sys.argv) > 3 else Path("bench/images")
BACKGROUNDS = (24, 232)


def rgba(path: Path) -> np.ndarray:
    with tempfile.TemporaryDirectory() as t:
        p = Path(t) / "x.png"
        bench.strip_color_chunks(path, p)
        return np.asarray(Image.open(p).convert("RGBA")).astype(np.float32)


def over(px: np.ndarray, bg: int, dst: Path) -> None:
    a = px[..., 3:4] / 255
    Image.fromarray((px[..., :3] * a + bg * (1 - a)).round().astype(np.uint8), "RGB").save(dst)


def distance(ref: Path, dist: Path) -> tuple[float, float]:
    out = subprocess.run([BUTTERAUGLI, str(ref), str(dist), "--pnorm", "3"], capture_output=True, text=True, check=True).stdout.split()
    return float(out[0]), float(out[-1])


def score_image(image: str) -> list[tuple]:
    src = rgba(IMAGES / image)
    alpha = bool((src[..., 3] < 255).any())
    rows = []
    with tempfile.TemporaryDirectory() as t:
        t = Path(t)
        refs = []
        for bg in BACKGROUNDS if alpha else (None,):
            ref = t / f"ref-{bg}.png"
            over(src, bg, ref) if alpha else Image.fromarray(src[..., :3].astype(np.uint8), "RGB").save(ref)
            refs.append((bg, ref))
        for name in PIPELINES:
            path = RUN / name / image
            if not path.exists():
                continue
            px = rgba(path)
            worst = (0.0, 0.0)
            for bg, ref in refs:
                d = t / "d.png"
                over(px, bg, d) if alpha else Image.fromarray(px[..., :3].astype(np.uint8), "RGB").save(d)
                m, p3 = distance(ref, d)
                worst = (max(worst[0], m), max(worst[1], p3))
            rows.append((image, name, *worst))
    return rows


images = sorted({r["image"] for r in csv.DictReader(open(RUN / "results.csv"))})
with ThreadPoolExecutor(8) as pool:
    results = [row for rows in pool.map(score_image, images) for row in rows]
with open(RUN / "butteraugli.csv", "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["image", "pipeline", "max", "pnorm3"])
    w.writerows(results)
print(f"{len(results)} scores -> {RUN / 'butteraugli.csv'}")
