"""JPEG to smaller JPEG: pngdeer against TinyPNG and open-source encoders, on photos.

  python3 experiments/jpeg/compare.py run PHOTOS TINY CSV [--cjpeg BIN] [--cjpegli BIN]
  python3 experiments/jpeg/compare.py summary CSV

PHOTOS holds the photos in subfolders (their kind: web/, orig/), TINY TinyPNG's file for each as
KIND-NAME (bench.py's TinyPNG client, which caches them). Every file is scored against its photo with
`pngdeer PHOTO --score FILE`, both decoded as pngdeer decodes them (zune-jpeg; within 0.1 point of
libjpeg-turbo, which browsers use). Per photo:

- tinypng: TinyPNG's file.
- pngdeer auto: pngdeer's automatic quality. pngdeer at tiny: pngdeer with TinyPNG's score as the
  target, the lowest quality reaching it.
- mozjpeg q75: cjpeg's defaults (mozjpeg 4.1.5: its table 3, 4:2:0, progressive, trellis), Squoosh's
  default quality. mozjpeg qN: a sweep, cjpeg choosing the chroma (4:2:0 below quality 80, 4:4:4 at
  90). jpegli qN and qN-420: a sweep with cjpegli's defaults (4:4:4, adaptive quantization) and with
  4:2:0. Each tool's settings are pooled into one size-quality frontier per photo. Both encode the
  photo as libjpeg-turbo decodes it (Pillow).

The summary compares each with TinyPNG as bench.py and the WebP leaderboard do: on the frontier,
the bytes needed for TinyPNG's score (median ratio and total, where reached) and the score at
TinyPNG's size; single files directly. Rows already in CSV aren't run again.
"""
import csv, json, statistics, subprocess, sys, tempfile
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
from bench import bytes_at_score, frontier, score_at_bytes  # noqa: E402

PNGDEER = ROOT / "target/release/pngdeer"
SWEEP = [40, 50, 60, 70, 80, 90]
FIELDS = ["image", "kind", "megapixels", "input_bytes", "tool", "setting", "bytes", "score", "quality", "ms"]


def score(photo: Path, file: Path) -> float:
    return float(subprocess.run([PNGDEER, photo, "--score", file], capture_output=True, text=True, check=True).stdout)


def pngdeer(photo: Path, out: Path, target: float | None) -> dict:
    args = [PNGDEER, photo, "-o", out] + (["--target", f"{target:.3f}"] if target is not None else [])
    return json.loads(subprocess.run(args, capture_output=True, text=True, check=True).stdout.strip().splitlines()[-1])


def run_photo(photo: Path, tiny_dir: Path, cjpeg: str | None, cjpegli: str | None) -> list[dict]:
    from PIL import Image

    kind = photo.parent.name
    image = Image.open(photo)
    base = {"image": f"{kind}/{photo.name}", "kind": kind, "megapixels": round(image.width * image.height / 1e6, 3), "input_bytes": photo.stat().st_size}
    rows = []

    def add(tool, setting, file: Path, quality=None, ms=None, score_=None):
        rows.append({**base, "tool": tool, "setting": setting, "bytes": file.stat().st_size, "score": round(score_ if score_ is not None else score(photo, file), 3), "quality": quality, "ms": ms})

    with tempfile.TemporaryDirectory() as t:
        t = Path(t)
        tiny = tiny_dir / f"{kind}-{photo.name}"
        add("tinypng", "", tiny)
        tiny_score = rows[-1]["score"]
        for tool, target in [("pngdeer auto", None), ("pngdeer at tiny", tiny_score)]:
            r = pngdeer(photo, t / "p.jpg", target)
            add(tool, r["outcome"], t / "p.jpg", r["quality"], r["ms"]["decode"] + r["ms"]["search"], r["score"] if r["quality"] is not None else 100.0)
        ppm = t / "in.ppm"
        image.convert("RGB").save(ppm)
        if cjpeg:
            for q in sorted({75, *SWEEP}):
                subprocess.run([cjpeg, "-quality", str(q), "-outfile", t / "m.jpg", ppm], check=True, capture_output=True)
                add("mozjpeg", f"q{q}", t / "m.jpg", q)
        if cjpegli:
            for q in SWEEP:
                for chroma in ["", "420"]:
                    subprocess.run([cjpegli, ppm, t / "j.jpg", "-q", str(q)] + ([f"--chroma_subsampling={chroma}"] if chroma else []), check=True, capture_output=True)
                    add("jpegli", f"q{q}" + (f"-{chroma}" if chroma else ""), t / "j.jpg", q)
    return rows


def run(photos_dir: Path, tiny_dir: Path, out: Path, cjpeg: str | None, cjpegli: str | None) -> None:
    done = {r["image"] for r in csv.DictReader(open(out))} if out.exists() else set()
    photos = [p for p in sorted(photos_dir.glob("*/*.jpg")) if f"{p.parent.name}/{p.name}" not in done]
    new = not out.exists()
    with open(out, "a", newline="") as f, ThreadPoolExecutor(3) as pool:
        w = csv.DictWriter(f, FIELDS)
        if new:
            w.writeheader()
        for rows in pool.map(lambda p: run_photo(p, tiny_dir, cjpeg, cjpegli), photos):
            w.writerows(rows)
            f.flush()
            print(rows[0]["image"], file=sys.stderr)


def summary(path: Path) -> None:
    by_image = defaultdict(list)
    for r in csv.DictReader(open(path)):
        by_image[r["image"]].append(r)
    pct = lambda x: f"{100 * x:.1f}%"
    images = sorted(by_image)
    tiny = {i: next(r for r in by_image[i] if r["tool"] == "tinypng") for i in images}
    print(f"{len(images)} photos; TinyPNG keeps {pct(sum(int(t['bytes']) for t in tiny.values()) / sum(int(t['input_bytes']) for t in tiny.values()))} of their bytes, median score {statistics.median(float(t['score']) for t in tiny.values()):.1f}\n")
    print("Frontier against TinyPNG: bytes at its score (median / total, where reached), reached, points at its size (median)")
    for tool in ["pngdeer at tiny", "mozjpeg", "jpegli"]:
        ratios, sizes, points, reached = [], [0, 0], [], 0
        for i in images:
            rows = [r for r in by_image[i] if r["tool"] == tool]
            if not rows:
                continue
            front = frontier([type("R", (), {"ssimulacra2": float(r["score"]), "out_bytes": int(r["bytes"])}) for r in rows])
            t = tiny[i]
            size = bytes_at_score(front, float(t["score"]))
            if size is not None:
                reached += 1
                ratios.append(size / int(t["bytes"]))
                sizes[0] += size
                sizes[1] += int(t["bytes"])
            s = score_at_bytes(front, int(t["bytes"]))
            if s is not None and len(front) > 1:  # one file has no size-quality curve
                points.append(s - float(t["score"]))
        if ratios:
            print(f"  {tool:16} {pct(statistics.median(ratios))} / {pct(sizes[0] / sizes[1])}  {reached}/{len(images)}  {f'{statistics.median(points):+.2f}' if points else '-'}")
    print("\nSingle files against TinyPNG: bytes (median ratio / total), score difference (median), files bigger than the input")
    for tool, setting in [("pngdeer auto", None), ("mozjpeg", "q75")]:
        pairs = [(next(r for r in by_image[i] if r["tool"] == tool and (setting is None or r["setting"] == setting)), tiny[i]) for i in images]
        ratio = statistics.median(int(a["bytes"]) / int(t["bytes"]) for a, t in pairs)
        total = sum(int(a["bytes"]) for a, _ in pairs) / sum(int(t["bytes"]) for _, t in pairs)
        diff = statistics.median(float(a["score"]) - float(t["score"]) for a, t in pairs)
        bigger = sum(int(a["bytes"]) >= int(a["input_bytes"]) for a, _ in pairs)
        print(f"  {tool + (' ' + setting if setting else ''):16} {pct(ratio)} / {pct(total)}  {diff:+.2f}  {bigger}")
    auto = [next(r for r in by_image[i] if r["tool"] == "pngdeer auto") for i in images]
    for kind in sorted({r["kind"] for r in auto}):
        mine = [(a, tiny[a["image"]]) for a in auto if a["kind"] == kind]
        print(f"    {kind}: {len(mine)} photos, bytes {pct(statistics.median(int(a['bytes']) / int(t['bytes']) for a, t in mine))}, score {statistics.median(float(a['score']) - float(t['score']) for a, t in mine):+.2f}")
    qualities = [int(a["quality"]) for a in auto if a["quality"]]
    outcomes = dict(Counter(a["setting"] for a in auto))
    print(f"  pngdeer auto: quality {min(qualities)}-{max(qualities)} (median {statistics.median(qualities)}), outcomes {outcomes}, median time {statistics.median(float(a['ms']) for a in auto) / 1000:.1f} s")


if __name__ == "__main__":
    if sys.argv[1] == "run":
        option = lambda name: sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None
        run(Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]), option("--cjpeg"), option("--cjpegli"))
    else:
        summary(Path(sys.argv[2]))
