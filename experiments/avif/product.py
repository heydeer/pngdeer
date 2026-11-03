"""pngdeer's own AVIF output (FINDINGS.md round 9) on round 8's images, beside round 8's prototype.

  python3 experiments/avif/product.py tiny PHOTOS IMAGES TINYAVIF TINYPNGS TINY.csv
  python3 experiments/avif/product.py run PHOTOS IMAGES AUTO.csv OUT.csv [TINY.csv]
  python3 experiments/avif/product.py summary OUT.csv

`run` compresses compare.py's images (the first 15 web photos and 15 camera originals of PHOTOS, 4
PNGs per category of IMAGES, their color chunks stripped as round 8's were) with `pngdeer IN -o
OUT.avif` at its automatic target, one at a time, and writes each file's target, setting, quality,
size, score and time next to the prototype's row of AUTO.csv (auto.py's `run ... tinypng` output with
FINAL=margin), which also holds TinyPNG's AVIF's size and score. With TINY.csv, each image's target
is instead TinyPNG's AVIF's score as Chrome shows it, from `tiny`: TinyPNG's files (compare.py's
`tiny` and `tiny-pngs`) decoded by chrome.mjs and scored by `pngdeer --score`, as pngdeer scores its
own; the rows' TinyPNG score is then that one. `summary` compares them.
"""
import csv
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from compare import PNGDEER, bench, chosen_photos, chosen_pngs  # noqa: E402

FIELDS = ["image", "target", "setting", "quality", "bytes", "score", "tries", "rounds", "ms",
          "p_target", "p_setting", "p_quality", "p_bytes", "p_score", "tiny_bytes", "tiny_score"]


def images(photos: Path, pngs: Path):
    """(name, path, is a photo) for compare.py's images."""
    os.environ.setdefault("PICK", "4")
    return [(f"{p.parent.name}/{p.name}", p, True) for p in chosen_photos(photos)] + [(str(p.relative_to(pngs)), p, False) for p in chosen_pngs(pngs)]


def source(path: Path, photo: bool, t: Path) -> Path:
    """What pngdeer compresses: the photo, or the PNG with its color chunks stripped as round 8's were."""
    if photo:
        return path
    src = t / "src.png"
    bench.strip_color_chunks(path, src)
    return src


def tiny(photos: Path, pngs: Path, tiny_photos: Path, tiny_pngs: Path, out: Path) -> None:
    with tempfile.TemporaryDirectory() as t:
        t = Path(t)
        named = []
        for name, path, photo in images(photos, pngs):
            avif = (tiny_photos / f"{path.parent.name}-{path.stem}.avif") if photo else (tiny_pngs / f"{path.parent.name}-{path.stem}.avif")
            copy = t / f"{len(named)}.avif"
            copy.write_bytes(avif.read_bytes())
            named.append((name, path, photo, copy))
        subprocess.run(["node", Path(__file__).parent / "chrome.mjs", *[c for *_, c in named]], check=True, capture_output=True)
        with open(out, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(["image", "tiny_bytes", "tiny_score"])
            for name, path, photo, copy in named:
                score = subprocess.run([PNGDEER, source(path, photo, t), "--score", f"{copy}.chrome.png"], check=True, capture_output=True, text=True).stdout
                writer.writerow([name, copy.stat().st_size, round(float(score), 3)])


def run(photos: Path, pngs: Path, auto: Path, out: Path, tiny_scores: Path | None) -> None:
    proto = {r["image"]: r for r in csv.DictReader(open(auto))}
    at = {r["image"]: r for r in csv.DictReader(open(tiny_scores))} if tiny_scores else {}
    items = images(photos, pngs)
    done = {r["image"] for r in csv.DictReader(open(out))} if out.exists() else set()
    new = not out.exists()
    with open(out, "a", newline="") as f:
        writer = csv.DictWriter(f, FIELDS)
        if new:
            writer.writeheader()
        for name, path, photo in items:
            if name in done:
                continue
            target = ["--target", at[name]["tiny_score"]] if at else []
            with tempfile.TemporaryDirectory() as t:
                src = source(path, photo, Path(t))
                start = time.perf_counter()
                report = subprocess.run([PNGDEER, src, "-o", Path(t) / "out.avif", "--json", *target], check=True, capture_output=True, text=True).stdout
                ms = round(1000 * (time.perf_counter() - start))
            r, p = json.loads(report.strip().splitlines()[-1]), dict(proto[name])
            if at:
                p["tiny_score"] = at[name]["tiny_score"]
            writer.writerow({"image": name, "target": r["target"], "setting": r["setting"], "quality": r["quality"], "bytes": r["bytes"],
                             "score": r["score"], "tries": r["tries"], "rounds": r["rounds"], "ms": ms, "p_target": p["target"],
                             "p_setting": p["setting"], "p_quality": p["quality"], "p_bytes": p["bytes"], "p_score": p["score"],
                             "tiny_bytes": p["tiny_bytes"], "tiny_score": p["tiny_score"]})
            f.flush()
            print(name, r["setting"], r["quality"], r["bytes"], p["bytes"], f"{ms / 1000:.1f} s", flush=True)


def summary(path: str) -> None:
    rows = list(csv.DictReader(open(path)))
    group = lambda r: r["image"].split("/")[0] if r["image"].split("/")[0] in ("web", "orig") else "PNGs"
    print("Group   Images  Bytes vs round 8 (median / total)  vs TinyPNG's AVIF  Score vs TinyPNG (median)  Smaller, not worse  Time (median / max)")
    for name in ["web", "orig", "PNGs", "all"]:
        g = [r for r in rows if name == "all" or group(r) == name]
        if not g:
            continue
        total = lambda key: sum(int(r["bytes"]) for r in g) / sum(int(r[key]) for r in g)
        to_proto = [int(r["bytes"]) / int(r["p_bytes"]) for r in g]
        to_tiny = [int(r["bytes"]) / int(r["tiny_bytes"]) for r in g]
        points = [float(r["score"]) - float(r["tiny_score"]) for r in g]
        both = sum(1 for x, d in zip(to_tiny, points) if x < 1 and d >= 0)
        times = [int(r["ms"]) / 1000 for r in g]
        print(f"{name:7} {len(g):6}  {statistics.median(to_proto):7.1%} / {total('p_bytes'):6.1%}            {statistics.median(to_tiny):6.1%} / "
              f"{total('tiny_bytes'):6.1%}   {statistics.median(points):+6.2f}                     {both:3}/{len(g):<3}             "
              f"{statistics.median(times):5.1f} / {max(times):5.1f} s")
    print("\nMore than 3% from round 8's file:")
    for r in rows:
        x = int(r["bytes"]) / int(r["p_bytes"])
        if abs(x - 1) > 0.03:
            print(f"  {r['image'][:44]:44} {r['setting']:16} q{r['quality']:>3} {x:6.1%}  (round 8: {r['p_setting']} q{r['p_quality']}), "
                  f"score {float(r['score']):.1f} vs {float(r['p_score']):.1f}")


if __name__ == "__main__":
    command, args = sys.argv[1], sys.argv[2:]
    if command == "tiny":
        tiny(*map(Path, args[:5]))
    elif command == "run":
        run(Path(args[0]), Path(args[1]), Path(args[2]), Path(args[3]), Path(args[4]) if len(args) > 4 else None)
    elif command == "summary":
        summary(args[0])
    else:
        sys.exit(__doc__)
