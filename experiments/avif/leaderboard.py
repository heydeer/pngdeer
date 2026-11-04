"""The AVIF leaderboard (the site's Research tab, the README's table), by the WebP and JPEG ones' method:
each tool's files pooled into one size-quality frontier per image, against TinyPNG's AVIF.

  python3 experiments/avif/leaderboard.py sweep PHOTOS IMAGES TINY.csv OUT.csv [TOOL]
  python3 experiments/avif/leaderboard.py summary TINY.csv OUT.csv...

The images are round 8's 54 (compare.py's 15 web photos, 15 camera originals and 4 PNGs per
category of IMAGES, their color chunks stripped); TINY.csv is product.py `tiny`'s, TinyPNG's AVIF
scored as Chrome shows it. `sweep` makes, per image:
- slimdeer: its runs at the automatic target (62.3 for a photo) and at TinyPNG's score less 6, less 3,
  plus 0, 3 and 6;
- avifenc (libavif with libaom, what Squoosh and sharp encode with): its default speed, 4:4:4 at 8
  bits (its default for a PNG), 4:4:4 at 10 bits and 4:2:0 at 10 bits (TinyPNG's), qualities 30-90;
- rav1e through ravif (10-bit 4:4:4; tools/aviflab), speed 6, the same qualities;
all from the PNG (for a photo, the upright PNG of it, as round 1 to 8 encoded, without its color
profile: Chrome would show a file with one color-managed, not as the photo's own pixels, which TinyPNG,
dropping the profile, is scored against), decodes every file in Chrome (chrome.mjs) and scores it with
`slimdeer --score`, as TinyPNG's were. With TOOL, only that tool's files. `summary` gives, per tool:
bytes saved at TinyPNG's quality (median, where it gets there; and in total), how many images it gets
there on, and its points above TinyPNG at TinyPNG's size; a later OUT.csv's rows of a tool replace
an earlier one's for the same image.
"""
import csv
import statistics
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from compare import AVIFLAB, SLIMDEER, bench  # noqa: E402
from product import images, source  # noqa: E402

QUALITIES = [30, 40, 50, 55, 60, 65, 70, 75, 80, 85, 90]
AVIFENC = {"444": ["-y", "444", "-d", "8"], "444-10bit": ["-y", "444", "-d", "10"], "420-10bit": ["-y", "420", "-d", "10"]}
OFFSETS = [-6, -3, 0, 3, 6]
FIELDS = ["image", "group", "tool", "setting", "bytes", "score"]
TOOLS = ["slimdeer", "avifenc", "rav1e"]


def group(name: str) -> str:
    return "photos" if name.split("/")[0] in ("web", "orig") else "pngs"


def sweep(photos: Path, pngs: Path, tiny_csv: Path, out: Path, only: str | None = None) -> None:
    tiny = {r["image"]: r for r in csv.DictReader(open(tiny_csv))}
    done = {r["image"] for r in csv.DictReader(open(out))} if out.exists() else set()
    new = not out.exists()
    with open(out, "a", newline="") as f:
        writer = csv.DictWriter(f, FIELDS)
        if new:
            writer.writeheader()
        for name, path, photo in images(photos, pngs):
            if name in done:
                continue
            with tempfile.TemporaryDirectory() as t:
                t = Path(t)
                src = source(path, photo, t)  # what the files are scored against
                upright = src
                if photo:
                    upright = t / "upright.png"
                    subprocess.run([AVIFLAB, "png", path, upright], check=True)
                score = float(tiny[name]["tiny_score"])
                # a photo's automatic target, src/avif.rs's PHOTO_TARGET, as it would get from the JPEG
                jobs = [("slimdeer", "auto", [SLIMDEER, upright, "-o", "{out}", *(["--target", "62.3"] if photo else [])])]
                jobs += [("slimdeer", f"tiny{o:+d}", [SLIMDEER, upright, "-o", "{out}", "--target", f"{score + o:.3f}"]) for o in OFFSETS]
                jobs += [("avifenc", f"{fmt}-q{q}", ["avifenc", "-j", "1", "-s", "6", *flags, "-q", str(q), upright, "{out}"]) for fmt, flags in AVIFENC.items()
                         for q in QUALITIES]
                jobs += [("rav1e", f"s6-q{q}", [AVIFLAB, "enc", upright, "{out}", str(q), "6"]) for q in QUALITIES]
                jobs = [j for j in jobs if only in (None, j[0])]

                def encode(i_job):
                    i, (tool, setting, command) = i_job
                    out_file = t / f"{i}.avif"
                    subprocess.run([str(c).replace("{out}", str(out_file)) for c in command], check=True, capture_output=True)
                    return tool, setting, out_file

                files = list(ThreadPoolExecutor(8).map(encode, enumerate(jobs)))
                subprocess.run(["node", Path(__file__).parent / "chrome.mjs", *[f for *_, f in files]], check=True, capture_output=True)

                def scored(file):
                    tool, setting, avif = file
                    s = subprocess.run([SLIMDEER, src, "--score", f"{avif}.chrome.png"], check=True, capture_output=True, text=True).stdout
                    return {"image": name, "group": group(name), "tool": tool, "setting": setting, "bytes": avif.stat().st_size, "score": round(float(s), 3)}

                for row in ThreadPoolExecutor(4).map(scored, files):
                    writer.writerow(row)
            f.flush()
            print(name, len(files), "files", flush=True)


def summary(tiny_csv: str, *rows_csvs: str) -> None:
    tiny = {r["image"]: r for r in csv.DictReader(open(tiny_csv))}
    by_image: dict[str, dict[str, list]] = {}
    for rows_csv in rows_csvs:
        fresh = set()
        for r in csv.DictReader(open(rows_csv)):
            row = type("Row", (), {"out_bytes": int(r["bytes"]), "ssimulacra2": float(r["score"])})
            tools = by_image.setdefault(r["image"], {})
            if (r["image"], r["tool"]) not in fresh:
                fresh.add((r["image"], r["tool"]))
                tools[r["tool"]] = []
            tools[r["tool"]].append(row)
    for name, keep in [("all", lambda i: True), ("photos", lambda i: group(i) == "photos"), ("PNGs", lambda i: group(i) == "pngs")]:
        imgs = [i for i in by_image if keep(i)]
        print(f"\n{name} ({len(imgs)} images): tool, saved at TinyPNG's quality (median / total, where reached), reaches, points at its size")
        for tool in TOOLS:
            saved, sizes, points, reach = [], [0, 0], [], 0
            for i in imgs:
                front = bench.frontier(by_image[i].get(tool, []))
                if not front:
                    continue
                tb, ts = int(tiny[i]["tiny_bytes"]), float(tiny[i]["tiny_score"])
                b = bench.bytes_at_score(front, ts)
                if b is not None:
                    reach += 1
                    saved.append(1 - b / tb)
                    sizes[0] += b
                    sizes[1] += tb
                s = bench.score_at_bytes(front, tb)
                if s is not None:
                    points.append(s - ts)
            med = lambda xs: statistics.median(xs) if xs else float("nan")
            total = 1 - sizes[0] / sizes[1] if sizes[1] else float("nan")
            print(f"  {tool:8} {med(saved):6.1%} / {total:6.1%}   {reach:2}/{len(imgs)}   {med(points):+5.1f} (on {len(points)})")


if __name__ == "__main__":
    command, args = sys.argv[1], sys.argv[2:]
    if command == "sweep":
        sweep(*map(Path, args[:4]), args[4] if len(args) > 4 else None)
    elif command == "summary":
        summary(*args)
    else:
        sys.exit(__doc__)
