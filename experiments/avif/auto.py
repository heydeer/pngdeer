"""AVIF output prototyped end to end (FINDINGS.md round 8): an automatic target, a search at a fast
speed and a final encode at speed 4, with no setting picked by looking at TinyPNG's answer, against
TinyPNG's AVIF.

  python3 experiments/avif/auto.py refs PHOTOS IMAGES TINYAVIF TINYPNGS refs.csv
  python3 experiments/avif/auto.py fit refs.csv
  python3 experiments/avif/auto.py run PHOTOS IMAGES TINYAVIF TINYPNGS refs.csv auto.csv tinypng|pngdeer|at-tinypng [MARGIN]
  python3 experiments/avif/auto.py summary auto.csv

The images are compare.py's: 15 web photos and 15 camera originals of experiments/jpeg, and 4 PNGs per
category of bench/images (PICK=4), scored as compare.py scores them, with TinyPNG's AVIF of each
(TINYAVIF and TINYPNGS, from compare.py's `tiny` and `tiny-pngs`).

TinyPNG writes every AVIF at one quantizer (base_q_idx 104, its alpha planes too), so its score
follows the image, not its size. `refs` records, for each image, TinyPNG's AVIF, pngdeer's automatic
target for the image as it stands (JPEG output's for a photo, PNG output's for a PNG), and a
reference encode made as TinyPNG's headers say its AVIF was (REFERENCE: libaom all-intra at
quantizer 26, tune=ssim, quantization matrices, chroma and perceptual delta-q, which leaves CDEF
off; 4:2:0 10-bit, or 4:4:4 8-bit with alpha where the image has transparency; speed 6). `fit`
checks how well each of those predicts TinyPNG's score, every image predicted from the others.

`run` is the prototype, with one of two automatic targets: `tinypng`, about TinyPNG's AVIF's quality
(photos: the median TinyPNG score of the other photos; PNGs: PNG output's target less the median
gap to TinyPNG's AVIF on the other PNGs), or `pngdeer`, pngdeer's own target as it stands; plus
MARGIN points (default 0). `at-tinypng` aims at each image's TinyPNG score instead, to compare at
the same quality, as the leaderboards do.

Images over SMALL_MP: the lowest quality whose speed-8 file reaches the target, then the lowest at
speed 4 from there, bracketed in steps of 2, then interpolated. FINAL=step instead takes the first
quality from one step below the speed-8 one up whose speed-4 file reaches the target; FINAL=margin
does the same unless that first file beats the target by more than a quality step's worth of points
(the speed-8 files' slope), when it brackets downward from there. Smaller images, where encodes are
cheap: each of SMALL_SETTINGS searched at its own speed, the smallest file that reaches the target
kept. Every setting is 4:4:4 with tune=iq (avifenc's default for stills) and alpha at the color's
quality, as compare.py's. Encodes run one thread each, 10 images at a time, so their times are
contended.
"""
import csv
import json
import math
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from compare import AVIFLAB, PNGDEER, append_rows, bench, chosen_photos, chosen_pngs, photo_scorer, png_scorer  # noqa: E402

REFERENCE = ["-q", "59", "-a", "tune=ssim", "-a", "enable-qm=1", "-a", "enable-chroma-deltaq=1", "-a", "deltaq-mode=3"]
SMALL_MP = 0.5
# (name, avifenc flags, speed) for small images
SMALL_SETTINGS = [
    ("10bit-s4", ["-d", "10"], 4),
    ("10bit-screen-s4", ["-d", "10", "-a", "tune-content=screen"], 4),
    ("8bit-s4", ["-d", "8"], 4),
    ("10bit-s2", ["-d", "10"], 2),
]
REF_FIELDS = ["image", "kind", "megapixels", "alpha", "base_target", "tiny_bytes", "tiny_score", "ref_bytes", "ref_score"]
AUTO_FIELDS = ["image", "kind", "megapixels", "policy", "target", "tiny_bytes", "tiny_score", "setting", "quality", "bytes", "score", "encodes", "ms",
               "s8_quality", "s8_bytes", "s8_score"]


def images(photos: Path, pngs: Path, tiny_photos: Path, tiny_pngs: Path):
    """(name, kind, path, TinyPNG's AVIF, is a photo) for every image measured."""
    for p in chosen_photos(photos):
        yield f"{p.parent.name}/{p.name}", p.parent.name, p, tiny_photos / f"{p.parent.name}-{p.stem}.avif", True
    os.environ.setdefault("PICK", "4")
    for p in chosen_pngs(pngs):
        yield str(p.relative_to(pngs)), p.parent.name, p, tiny_pngs / f"{p.parent.name}-{p.stem}.avif", False


def is_photo(row: dict) -> bool:
    return row["kind"] in ("web", "orig")


def prepare(path: Path, photo: bool, t: Path):
    """The source to encode, the scorer, its megapixels and whether it has transparency (as pngdeer decodes it)."""
    src, score = (photo_scorer if photo else png_scorer)(path, t)
    w, h, _ = bench.png_info(src)
    alpha = False
    if not photo:
        subprocess.run([AVIFLAB, "png", src, t / "rgba.png"], check=True)  # RGB when every pixel is opaque
        alpha = bench.png_info(t / "rgba.png")[2].startswith("RGBA")
    return src, score, round(w * h / 1e6, 6), alpha


def base_target(path: Path, photo: bool, mp: float, t: Path) -> float:
    """pngdeer's automatic target as it stands: src/jpeg.rs's auto_target for a photo, PNG output's for a PNG."""
    if photo:
        return round(min(72.3 - 3.2 * math.log(mp), 90.0), 3)
    report = subprocess.run([PNGDEER, path, "-o", t / "pngdeer.png"], check=True, capture_output=True, text=True).stdout
    return json.loads(report.strip().splitlines()[-1])["target"]


def avifenc(src: Path, out: Path, speed: int, flags: list[str]) -> int:
    """Encodes with one thread; returns milliseconds."""
    start = time.perf_counter()
    subprocess.run(["avifenc", "-j", "1", "-s", str(speed), *flags, src, out], check=True, capture_output=True)
    return round(1000 * (time.perf_counter() - start))


def refs(photos: Path, pngs: Path, tiny_photos: Path, tiny_pngs: Path, out: Path) -> None:
    done = {r["image"] for r in csv.DictReader(open(out))} if out.exists() else set()

    def run(item) -> list[dict]:
        name, kind, path, tiny, photo = item
        if name in done:
            return []
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            src, score, mp, alpha = prepare(path, photo, t)
            avifenc(src, t / "r.avif", 6, (["-y", "444", "-d", "8"] if alpha else ["-y", "420", "-d", "10"]) + REFERENCE)
            return [{"image": name, "kind": kind, "megapixels": mp, "alpha": int(alpha), "base_target": base_target(path, photo, mp, t),
                     "tiny_bytes": tiny.stat().st_size, "tiny_score": score(tiny), "ref_bytes": (t / "r.avif").stat().st_size, "ref_score": score(t / "r.avif")}]

    append_rows(out, REF_FIELDS, list(images(photos, pngs, tiny_photos, tiny_pngs)), run, 10)


def least_squares(xs: list[list[float]], ys: list[float]) -> list[float]:
    """Weights w (w[0] the intercept) minimizing sum (y - w . [1, x])^2, by Gaussian elimination."""
    rows = [[1.0, *x] for x in xs]
    k = len(rows[0])
    a = [[sum(r[i] * r[j] for r in rows) for j in range(k)] + [sum(r[i] * y for r, y in zip(rows, ys))] for i in range(k)]
    for c in range(k):
        p = max(range(c, k), key=lambda r: abs(a[r][c]))
        a[c], a[p] = a[p], a[c]
        for r in range(k):
            if r != c:
                f = a[r][c] / a[c][c]
                a[r] = [x - f * y for x, y in zip(a[r], a[c])]
    return [a[i][k] / a[i][i] for i in range(k)]


def fit(path: str) -> None:
    rows = list(csv.DictReader(open(path)))
    models = {
        "a constant": lambda r: [],
        "pngdeer's target as it stands": lambda r: [float(r["base_target"])],
        "the reference's score": lambda r: [float(r["ref_score"])],
        "both, their distances from 100 added": lambda r: [float(r["base_target"]) + float(r["ref_score"]) - 100],
    }
    for group, keep in [("Photos", is_photo), ("PNGs", lambda r: not is_photo(r))]:
        g = [r for r in rows if keep(r)]
        print(f"{group}: TinyPNG's AVIF scores {min(float(r['tiny_score']) for r in g):.1f} to {max(float(r['tiny_score']) for r in g):.1f}"
              f" (median {statistics.median(float(r['tiny_score']) for r in g):.1f}); each predicted by a line through the others, its error:")
        for name, features in models.items():
            errors = []
            for r in g:
                others = [o for o in g if o is not r]
                w = least_squares([features(o) for o in others], [float(o["tiny_score"]) for o in others])
                errors.append(float(r["tiny_score"]) - w[0] - sum(a * b for a, b in zip(w[1:], features(r))))
            print(f"  {name:38} median {statistics.median(errors):+5.2f}, rms {statistics.fmean([e * e for e in errors]) ** 0.5:5.2f},"
                  f" worst {min(errors):+6.1f} / {max(errors):+6.1f}")
        gap = [float(r["tiny_score"]) - float(r["base_target"]) for r in g]
        print(f"  TinyPNG's AVIF less pngdeer's target: median {statistics.median(gap):+.2f}; reference's bytes over TinyPNG's: median"
              f" {statistics.median(int(r['ref_bytes']) / int(r['tiny_bytes']) for r in g):.1%}")


def target(ref_rows: list[dict], name: str, policy: str, margin: float) -> float:
    """The image's automatic target under policy, from the other images only (see the module docs)."""
    row = next(r for r in ref_rows if r["image"] == name)
    if policy == "pngdeer":
        return float(row["base_target"]) + margin
    if policy == "at-tinypng":
        return float(row["tiny_score"]) + margin
    others = [r for r in ref_rows if r["image"] != name and is_photo(r) == is_photo(row)]
    if is_photo(row):
        return statistics.median(float(r["tiny_score"]) for r in others) + margin
    return float(row["base_target"]) + statistics.median(float(r["tiny_score"]) - float(r["base_target"]) for r in others) + margin


def run(photos: Path, pngs: Path, tiny_photos: Path, tiny_pngs: Path, refs_path: str, out: Path, policy: str, margin: float) -> None:
    ref_rows = list(csv.DictReader(open(refs_path)))
    done = {r["image"] for r in csv.DictReader(open(out))} if out.exists() else set()

    def run_image(item) -> list[dict]:
        name, kind, path, tiny, photo = item
        if name in done:
            return []
        ref_row = next(r for r in ref_rows if r["image"] == name)
        goal = round(target(ref_rows, name, policy, margin), 3)
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            src, score, mp, alpha = prepare(path, photo, t)
            tried, encodes, ms = {}, 0, 0

            def attempt(setting, flags, speed, q):
                nonlocal encodes, ms
                key = (setting, speed, q)
                if key not in tried:
                    out = t / f"{setting}-s{speed}-q{q}.avif"
                    ms += avifenc(src, out, speed, ["-y", "444", "-q", str(q), *flags])
                    encodes += 1
                    tried[key] = (out.stat().st_size, score(out))
                return tried[key]

            def lowest(setting, flags, speed, start, step):
                """The lowest quality whose file reaches the target, by bracketing from start in steps,
                then interpolating; (quality, bytes, score), the file at 100 if none reaches it."""
                passes = lambda q: attempt(setting, flags, speed, q)[1] >= goal
                lo, hi, q = None, None, start  # highest failing, lowest passing
                while True:
                    if passes(q):
                        hi = q
                        if lo is not None or q == 0:
                            break
                        q = max(0, q - step)
                    else:
                        lo = q
                        if hi is not None or q == 100:
                            break
                        q = min(100, q + step)
                if hi is None:
                    return 100, *attempt(setting, flags, speed, 100)
                while lo is not None and hi - lo > 1:
                    s_lo, s_hi = attempt(setting, flags, speed, lo)[1], attempt(setting, flags, speed, hi)[1]
                    guess = lo + (goal - s_lo) / (s_hi - s_lo) * (hi - lo) if s_hi > s_lo else (lo + hi) / 2
                    q = min(hi - 1, max(lo + 1, round(guess)))
                    if passes(q):
                        hi = q
                    else:
                        lo = q
                return hi, *attempt(setting, flags, speed, hi)

            s8 = (None, None, None)
            if mp > SMALL_MP:
                s8 = lowest("10bit", ["-d", "10"], 8, 60, 8)
                final = os.environ.get("FINAL", "bracket")
                q = max(0, s8[0] - 1)
                # points per quality step near the target, from the speed-8 files within 10 steps
                near = sorted((k[2], v[1]) for k, v in tried.items() if k[1] == 8 and abs(k[2] - s8[0]) <= 10)
                xs, ys = [x for x, _ in near], [y for _, y in near]
                per_step = max(least_squares([[x] for x in xs], ys)[1], 0.3) if len(set(xs)) > 1 else 0.7
                if final in ("step", "margin") and not (final == "margin" and attempt("10bit", ["-d", "10"], 4, q)[1] >= goal + per_step):
                    while q < 100 and attempt("10bit", ["-d", "10"], 4, q)[1] < goal:
                        q += 1
                    pick = (f"10bit-s4-{final}", q, *attempt("10bit", ["-d", "10"], 4, q))
                elif final == "margin":  # room for more than a step below: search down from there
                    pick = ("10bit-s4-margin-down", *lowest("10bit", ["-d", "10"], 4, q, 2))
                else:
                    pick = ("10bit-s4", *lowest("10bit", ["-d", "10"], 4, s8[0], 2))
            else:
                found = [(setting, *lowest(setting, flags, speed, 60, 8)) for setting, flags, speed in SMALL_SETTINGS]
                pick = min([f for f in found if f[3] >= goal] or found, key=lambda f: f[2])
        return [{"image": name, "kind": kind, "megapixels": mp, "policy": policy, "target": goal, "tiny_bytes": ref_row["tiny_bytes"],
                 "tiny_score": ref_row["tiny_score"], "setting": pick[0], "quality": pick[1], "bytes": pick[2], "score": pick[3], "encodes": encodes,
                 "ms": ms, "s8_quality": s8[0], "s8_bytes": s8[1], "s8_score": s8[2]}]

    append_rows(out, AUTO_FIELDS, list(images(photos, pngs, tiny_photos, tiny_pngs)), run_image, 10)


def summary(path: str) -> None:
    rows = list(csv.DictReader(open(path)))
    print(f"Target: {rows[0]['policy']}")
    print("Group       Images  Bytes vs TinyPNG (median / total)  Smaller  Score vs TinyPNG (median)  Smaller, not worse  Encodes  Encode time (median / max)")
    for name, keep in [("web photos", lambda r: r["kind"] == "web"), ("originals", lambda r: r["kind"] == "orig"), ("PNGs", lambda r: not is_photo(r)),
                       ("  icons", lambda r: r["kind"] == "icons"), ("all", lambda r: True)]:
        g = [r for r in rows if keep(r)]
        if not g:
            continue
        ratios = [int(r["bytes"]) / int(r["tiny_bytes"]) for r in g]
        diffs = [float(r["score"]) - float(r["tiny_score"]) for r in g]
        total = sum(int(r["bytes"]) for r in g) / sum(int(r["tiny_bytes"]) for r in g)
        both = sum(1 for x, d in zip(ratios, diffs) if x < 1 and d >= 0)
        times = [int(r["ms"]) / 1000 for r in g]
        print(f"{name:11} {len(g):6}  {statistics.median(ratios):7.1%} / {total:6.1%}            {sum(x < 1 for x in ratios):3}/{len(g):<3}  "
              f"{statistics.median(diffs):+6.2f}                     {both:3}/{len(g):<3}             {statistics.median(int(r['encodes']) for r in g):4}     "
              f"{statistics.median(times):5.1f} / {max(times):5.1f} s")
    big = [r for r in rows if r["s8_bytes"]]
    if big:
        print(f"\nOver {SMALL_MP} MP: the speed-8 search's file is {statistics.median(int(r['s8_bytes']) / int(r['bytes']) for r in big):.1%} of the final"
              f" speed-4 file's bytes (median); the final quality is {statistics.median(int(r['quality']) - int(r['s8_quality']) for r in big):+.0f} from the"
              f" speed-8 one (median), {min(int(r['quality']) - int(r['s8_quality']) for r in big):+d} to {max(int(r['quality']) - int(r['s8_quality']) for r in big):+d}")
    small = [r for r in rows if not r["s8_bytes"]]
    if small:
        print(f"Up to {SMALL_MP} MP, the setting kept: " + ", ".join(f"{s} {sum(r['setting'] == s for r in small)}" for s, _, _ in SMALL_SETTINGS))
    misses = [r for r in rows if float(r["score"]) < float(r["target"])]
    print(f"Target missed: {len(misses)}" + "".join(f"\n  {r['image']} {float(r['score']):.2f} < {float(r['target']):.2f}" for r in misses))


if __name__ == "__main__":
    command, args = sys.argv[1], sys.argv[2:]
    if command == "refs":
        refs(*map(Path, args[:4]), Path(args[4]))
    elif command == "fit":
        fit(args[0])
    elif command == "run":
        run(*map(Path, args[:4]), args[4], Path(args[5]), args[6], float(args[7]) if len(args) > 7 else 0.0)
    elif command == "summary":
        summary(args[0])
    else:
        sys.exit(__doc__)
