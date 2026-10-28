"""python3 experiments/webp/tiny_emulate.py IMAGES OUT [--quality Q] [--lossy ARGS] [--lossless ARGS]
[--lossy-from source|tinypng]: a stand-in for TinyPNG's WebP conversion, for images whose real one
wasn't fetched (two TinyPNG compressions each).

TinyPNG's WebP is its compressed PNG stored as lossless WebP, or a lossy file at about libwebp
quality 70 where that is smaller (FINDINGS.md, round 3). This makes both from the cached TinyPNG
PNG (bench/.cache/tinypng; nothing is fetched) and the source, keeps the smaller one, and writes
bench.py-style rows (pipeline "tinyemu") to OUT/results.csv, scored as bench.py scores, so
compare.py can read them next to real ones (pass --stand-in tinyemu there to compare against it).
"""

import argparse
import csv
import hashlib
import shutil
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, fields
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "bench"))
import bench  # noqa: E402


def emulate(img: bench.Image, cfg: bench.Config, args: argparse.Namespace) -> bench.Row:
    width, height, color = bench.png_info(img.path)
    row = bench.Row(str(img.rel), img.category, width, height, color, "tinyemu", img.path.stat().st_size)
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        source = tmp / "source.png"
        bench.strip_color_chunks(img.path, source)
        tiny = bench.BENCH_DIR / ".cache" / "tinypng" / (hashlib.sha256(source.read_bytes()).hexdigest() + ".png")
        if not tiny.exists():
            row.note = "error: no cached TinyPNG PNG"
            return row
        lossless, lossy = tmp / "lossless.webp", tmp / "lossy.webp"
        bench.run([cfg.cwebp, "-quiet", "-lossless", *args.lossless.split(), str(tiny), "-o", str(lossless)])
        lossy_input = tiny if args.lossy_from == "tinypng" else source
        bench.run([cfg.cwebp, "-quiet", "-q", str(args.quality), *args.lossy.split(), str(lossy_input), "-o", str(lossy)])
        pick, kind = (lossless, "lossless") if lossless.stat().st_size <= lossy.stat().st_size else (lossy, "lossy")
        out = args.out / "tinyemu" / img.rel.with_suffix(".webp")
        out.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(pick, out)
        row.out_bytes = out.stat().st_size
        row.ratio = round(row.out_bytes / row.orig_bytes, 4)
        row.ssimulacra2 = round(bench.Scorer(cfg, source, tmp, width, height).score(out), 2)
        row.params = kind
    return row


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("images", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--quality", type=int, default=70)
    parser.add_argument("--lossy", default="-m 6 -sharp_yuv -alpha_q 100", help="more cwebp arguments for the lossy file")
    parser.add_argument("--lossless", default="", help="more cwebp arguments for the lossless file")
    parser.add_argument("--lossy-from", choices=("source", "tinypng"), default="source")
    parser.add_argument("-j", "--jobs", type=int, default=7)
    args = parser.parse_args()
    tools = bench.find_tools(need_pngdeer=True, webp=True)
    cfg = bench.Config(tools["pngquant"], tools["oxipng"], tools["ssimulacra2"], "max", False, None, tools["pngdeer"], cwebp=tools["cwebp"], dwebp=tools["dwebp"])
    images = bench.find_images(args.images.resolve())
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        rows = list(pool.map(lambda img: emulate(img, cfg, args), images))
    args.out.mkdir(parents=True, exist_ok=True)
    with open(args.out / "results.csv", "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=[fld.name for fld in fields(bench.Row)])
        writer.writeheader()
        writer.writerows(asdict(r) for r in rows)
    print(f"{len(rows)} images → {args.out / 'results.csv'}")


if __name__ == "__main__":
    main()
