"""AVIF from photos and PNGs: libaom (avifenc) and rav1e (tools/aviflab) against pngdeer's JPEG and
WebP and against TinyPNG.

  python3 experiments/avif/compare.py tiny PHOTOS TINYAVIF
  python3 experiments/avif/compare.py photos PHOTOS TINYAVIF CSV [CONFIG...]
  python3 experiments/avif/compare.py tiny-pngs IMAGES TINYAVIF
  python3 experiments/avif/compare.py pngs IMAGES TINYAVIF CSV [CONFIG...]
  python3 experiments/avif/compare.py summary CSV JPEG.csv WEBP.csv
  python3 experiments/avif/compare.py summary-pngs CSV BENCH-RESULTS.csv...
  python3 experiments/avif/compare.py summary-tiny-pngs CSV BENCH-RESULTS.csv
  python3 experiments/avif/compare.py speed PHOTO...

PHOTOS holds experiments/jpeg's photos in web/ and orig/; the first 15 of each are measured (KINDS=web
measures the web ones only). `tiny` fetches TinyPNG's AVIF of each into TINYAVIF as KIND-STEM.avif
(bench.py's `convert_avif`: a shrink, then a conversion, two compressions; cached). `tiny-pngs` does
the same for bench/images (PICK=4: 4 per category) as CATEGORY-STEM.avif.

`photos` encodes each photo as pngdeer decodes it (zune-jpeg, turned upright), written as a PNG by
`aviflab png`, with every CONFIG over a sweep of qualities (default: those at speed 6). Every file is
decoded with avifdec (dav1d; libavif's own YUV to RGB, bilinear chroma) to 8-bit and scored against
the photo with `pngdeer PHOTO --score`. `pngs` does the same for bench/images (default: aom-s6 and
rav1e-s6; PICK=N: N per category), scored as bench.py scores (color chunks stripped, small images
enlarged, libjxl's ssimulacra2), with TinyPNG's AVIF where TINYAVIF has it. Rows already in CSV
aren't run again. Encodes run one thread each, 10 photos (4 images) at a time: their times are
contended; `speed` times encodes one at a time.

`summary` compares on frontiers, as experiments/jpeg does: the bytes each config needs for a score
(TinyPNG's AVIF's and JPEG's, pngdeer's JPEG's and WebP's at automatic quality, from experiments/jpeg's
CSVs) against those files' bytes. `summary-pngs` compares at TinyPNG's PNG score against pngdeer's
WebP frontier (targets 40-98: the smaller of its palette as lossless WebP and lossy WebP), from
bench.py results (the first file with a pipeline for an image wins). `summary-tiny-pngs` compares at
TinyPNG's AVIF's score, on the images that have it.
"""
import csv, os, statistics, subprocess, sys, tempfile, time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
import bench  # noqa: E402
from bench import bytes_at_score, frontier, score_at_bytes  # noqa: E402

PNGDEER = ROOT / "target/release/pngdeer"
AVIFLAB = ROOT / "target/release/aviflab"
AVIFLAB_WASM = ROOT / "target/wasm32-unknown-unknown/release/aviflab.wasm"
AOM_Q = [30, 40, 50, 55, 60, 65, 70, 80]
RAV1E_Q = [50, 60, 65, 70, 75, 80, 88]
PNG_AOM_Q = [20, 30, 40, 50, 60, 70, 80, 90, 95]
PNG_RAV1E_Q = [40, 50, 60, 70, 80, 90, 95]


def aom(speed: int, *flags: str):
    return lambda q, src, out: ["avifenc", "-j", "1", "-s", str(speed), *flags, "-q", str(q), src, out]


def rav1e(speed: int):
    return lambda q, src, out: [AVIFLAB, "enc", src, out, str(q), str(speed)]


PHOTO_CONFIGS = {
    "aom-s6-444": (AOM_Q, aom(6)),
    "aom-s6-420": (AOM_Q, aom(6, "-y", "420")),
    "aom-s6-420-10bit": (AOM_Q, aom(6, "-y", "420", "-d", "10")),
    "aom-s6-444-10bit": (AOM_Q, aom(6, "-d", "10")),
    "rav1e-s6": (RAV1E_Q, rav1e(6)),
    # what speed costs (on the web photos)
    "aom-s4-444-10bit": (AOM_Q, aom(4, "-d", "10")),
    "aom-s5-444-10bit": (AOM_Q, aom(5, "-d", "10")),
    "aom-s8-444-10bit": (AOM_Q, aom(8, "-d", "10")),
    "aom-s9-444-10bit": (AOM_Q, aom(9, "-d", "10")),
    "rav1e-s8": (RAV1E_Q, rav1e(8)),
    "rav1e-s10": (RAV1E_Q, rav1e(10)),
}
PNG_CONFIGS = {
    "aom-s6": (PNG_AOM_Q, aom(6)),
    "rav1e-s6": (PNG_RAV1E_Q, rav1e(6)),
    # pngdeer's best shot, on the images TinyPNG's AVIF was fetched for (PICK=4)
    "aom-s6-10bit": (PNG_AOM_Q, aom(6, "-d", "10")),
    "aom-s4-10bit": (PNG_AOM_Q, aom(4, "-d", "10")),
    "aom-s6-10bit-screen": (PNG_AOM_Q, aom(6, "-d", "10", "-a", "tune-content=screen")),
    "aom-s6-420-10bit": (PNG_AOM_Q, aom(6, "-y", "420", "-d", "10")),
    "aom-lossless": ([100], aom(6, "--cicp", "1/13/0")),  # RGB coded losslessly: score 100
}
FIELDS = ["image", "kind", "megapixels", "tool", "setting", "bytes", "score", "quality", "ms"]
PNG_FIELDS = ["image", "category", "width", "height", "tool", "setting", "bytes", "score", "quality", "ms"]


def chosen_photos(photos: Path) -> list[Path]:
    return [p for kind in os.environ.get("KINDS", "web,orig").split(",") for p in sorted(photos.glob(f"{kind}/*.jpg"))[:15]]


def encode(command, q: int, src: Path, out: Path, is_rav1e: bool) -> int:
    """Encodes; returns milliseconds (aviflab's own count, or avifenc's whole run)."""
    start = time.perf_counter()
    r = subprocess.run(command(q, src, out), check=True, capture_output=True, text=True)
    return int(r.stdout.strip().splitlines()[-1]) if is_rav1e else round(1000 * (time.perf_counter() - start))


def avifdec(avif: Path, png: Path) -> None:
    subprocess.run(["avifdec", "-j", "1", "-d", "8", avif, png], check=True, capture_output=True)


def append_rows(out: Path, fields: list[str], items, work, jobs: int) -> None:
    new = not out.exists()
    with open(out, "a", newline="") as f, ThreadPoolExecutor(jobs) as pool:
        w = csv.DictWriter(f, fields)
        if new:
            w.writeheader()
        for rows in pool.map(work, items):
            w.writerows(rows)
            f.flush()
            print(rows[0]["image"] if rows else "-", file=sys.stderr, flush=True)


def chosen_pngs(images: Path) -> list[Path]:
    """bench/images, or with PICK=N, N per category spread over its sorted list."""
    paths = [i.path for i in bench.find_images(images)]
    if not os.environ.get("PICK"):
        return paths
    n, by_category = int(os.environ["PICK"]), defaultdict(list)
    for p in paths:
        by_category[p.parent.name].append(p)
    return [ps[k * len(ps) // n] for ps in by_category.values() for k in range(n)]


def tiny(photos: Path, out: Path) -> None:
    client = bench.TinyPNG(bench.load_api_keys(), ROOT / "bench/.cache/tinypng")
    client.validate()
    out.mkdir(parents=True, exist_ok=True)
    chosen = chosen_photos(photos)
    cached = list(ThreadPoolExecutor(4).map(lambda p: client.convert_avif(p, out / f"{p.parent.name}-{p.stem}.avif")[0], chosen))
    print(f"{len(chosen)} photos: {cached.count(True)} cached, {cached.count(False)} fetched; compressions {client.compression_counts}")


def tiny_pngs(images: Path, out: Path) -> None:
    """TinyPNG's AVIF of each chosen image, sent without color chunks as bench.py sends PNGs."""
    client = bench.TinyPNG(bench.load_api_keys(), ROOT / "bench/.cache/tinypng")
    client.validate()
    out.mkdir(parents=True, exist_ok=True)

    def one(img: Path) -> bool:
        with tempfile.TemporaryDirectory() as t:
            src = Path(t) / "src.png"
            bench.strip_color_chunks(img, src)
            return client.convert_avif(src, out / f"{img.parent.name}-{img.stem}.avif")[0]

    chosen = chosen_pngs(images)
    cached = list(ThreadPoolExecutor(4).map(one, chosen))
    print(f"{len(chosen)} images: {cached.count(True)} cached, {cached.count(False)} fetched; compressions {client.compression_counts}")


def photo_scorer(photo: Path, t: Path):
    """The photo as pngdeer decodes it (turned upright), written to t/src.png to encode, and a function
    scoring an AVIF against the photo."""
    src, dec = t / "src.png", t / "dec.png"
    subprocess.run([AVIFLAB, "png", photo, src], check=True)

    def score(avif: Path) -> float:
        avifdec(avif, dec)
        return round(float(subprocess.run([PNGDEER, photo, "--score", dec], capture_output=True, text=True, check=True).stdout), 3)

    return src, score


def png_scorer(img: Path, t: Path):
    """The PNG with its color chunks stripped, written to t/src.png to encode, and a function scoring an
    AVIF against it as bench.py scores (small images enlarged)."""
    src, dec, big = t / "src.png", t / "dec.png", t / "big.png"
    bench.strip_color_chunks(img, src)
    w, h, _ = bench.png_info(src)
    factor = 1
    while max(w, h) * factor < 256 and factor < 8:  # as bench.Scorer
        factor *= 2
    ref = src
    if factor > 1:
        ref = t / "ref.png"
        subprocess.run([PNGDEER, src, "-o", ref, "--upscale", str(factor)], check=True, capture_output=True)

    def score(avif: Path) -> float:
        avifdec(avif, dec)
        scored = dec
        if factor > 1:
            subprocess.run([PNGDEER, dec, "-o", big, "--upscale", str(factor)], check=True, capture_output=True)
            scored = big
        return round(float(subprocess.run(["ssimulacra2", ref, scored], capture_output=True, text=True, check=True).stdout.split()[-1]), 3)

    return src, score


def photos(photos_dir: Path, tiny_dir: Path, out: Path, configs: list[str]) -> None:
    done = {(r["image"], r["tool"]) for r in csv.DictReader(open(out))} if out.exists() else set()

    def run_photo(photo: Path) -> list[dict]:
        kind, name = photo.parent.name, f"{photo.parent.name}/{photo.name}"
        rows = []
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            src, score = photo_scorer(photo, t)
            w, h, _ = bench.png_info(src)
            base = {"image": name, "kind": kind, "megapixels": round(w * h / 1e6, 3)}
            if (name, "tinypng") not in done:
                file = tiny_dir / f"{kind}-{photo.stem}.avif"
                rows.append({**base, "tool": "tinypng", "setting": "", "bytes": file.stat().st_size, "score": score(file)})
            for config in configs:
                if (name, config) in done:
                    continue
                qualities, command = PHOTO_CONFIGS[config]
                for q in qualities:
                    ms = encode(command, q, src, t / "o.avif", config.startswith("rav1e"))
                    rows.append({**base, "tool": config, "setting": f"q{q}", "bytes": (t / "o.avif").stat().st_size, "score": score(t / "o.avif"), "quality": q, "ms": ms})
        return rows

    append_rows(out, FIELDS, chosen_photos(photos_dir), run_photo, 10)


def pngs(images: Path, tiny_dir: Path, out: Path, configs: list[str]) -> None:
    done = {(r["image"], r["tool"]) for r in csv.DictReader(open(out))} if out.exists() else set()

    def run_image(img: Path) -> list[dict]:
        rows = []
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            src, score = png_scorer(img, t)
            w, h, _ = bench.png_info(src)
            name = str(img.relative_to(images))
            base = {"image": name, "category": img.parent.name, "width": w, "height": h}
            tiny_avif = tiny_dir / f"{img.parent.name}-{img.stem}.avif"
            if (name, "tinypng") not in done and tiny_avif.exists():
                rows.append({**base, "tool": "tinypng", "setting": "", "bytes": tiny_avif.stat().st_size, "score": score(tiny_avif)})
            for config in configs:
                if (name, config) in done:
                    continue
                qualities, command = PNG_CONFIGS[config]
                for q in qualities:
                    ms = encode(command, q, src, t / "o.avif", config.startswith("rav1e"))
                    rows.append({**base, "tool": config, "setting": f"q{q}", "bytes": (t / "o.avif").stat().st_size, "score": score(t / "o.avif"), "quality": q, "ms": ms})
        return rows

    append_rows(out, PNG_FIELDS, chosen_pngs(images), run_image, 4)


def as_row(score, size):
    return type("R", (), {"ssimulacra2": float(score), "out_bytes": int(size)})


def pct(x: float) -> str:
    return f"{100 * x:.1f}%"


def by_image(*paths: str) -> dict[str, list[dict]]:
    rows = defaultdict(list)
    for path in paths:
        for r in csv.DictReader(open(path)):
            rows[r["image"]].append(r)
    return rows


def summary(path: str, jpeg_path: str, webp_path: str) -> None:
    avif, jpeg, webp = by_image(path), by_image(jpeg_path), by_image(webp_path)
    images = sorted(i for i in avif if any(r["tool"] == "tinypng" for r in avif[i]))
    pick = lambda rows, tool: next(r for r in rows if r["tool"] == tool)
    print(f"{len(images)} photos ({sum(i.startswith('web/') for i in images)} web, {sum(i.startswith('orig/') for i in images)} originals)\n")
    pairs = [(pick(avif[i], "tinypng"), pick(jpeg[i], "tinypng")) for i in images]
    print(f"TinyPNG's AVIF against its JPEG: bytes {pct(statistics.median(int(a['bytes']) / int(j['bytes']) for a, j in pairs))} median / "
          f"{pct(sum(int(a['bytes']) for a, _ in pairs) / sum(int(j['bytes']) for _, j in pairs))} total, score {statistics.median(float(a['score']) - float(j['score']) for a, j in pairs):+.2f} (median)\n")
    anchors = {
        "TinyPNG's AVIF": lambda i: pick(avif[i], "tinypng"),
        "TinyPNG's JPEG": lambda i: pick(jpeg[i], "tinypng"),
        "pngdeer's JPEG (automatic)": lambda i: pick(jpeg[i], "pngdeer auto"),
        "pngdeer's WebP (automatic)": lambda i: pick(webp[i], "pngdeer auto"),
    }
    tools = [t for t in PHOTO_CONFIGS if any(r["tool"] == t for i in images for r in avif[i])]
    for name, anchor in anchors.items():
        print(f"At {name} score: bytes against it (median / total, where reached), reached, points at its size (median), median bytes by kind")
        for tool in tools:
            ratios, sizes, points, kinds, n = [], [0, 0], [], defaultdict(list), 0
            for i in images:
                rows = [r for r in avif[i] if r["tool"] == tool]
                if not rows:
                    continue
                n += 1
                a, front = anchor(i), frontier([as_row(r["score"], r["bytes"]) for r in rows])
                size = bytes_at_score(front, float(a["score"]))
                if size is not None:
                    ratios.append(size / int(a["bytes"]))
                    kinds[i.split("/")[0]].append(size / int(a["bytes"]))
                    sizes[0] += size
                    sizes[1] += int(a["bytes"])
                s = score_at_bytes(front, int(a["bytes"]))
                if s is not None:
                    points.append(s - float(a["score"]))
            if ratios:
                print(f"  {tool:18} {pct(statistics.median(ratios))} / {pct(sizes[0] / sizes[1])}  {len(ratios)}/{n}  {statistics.median(points):+.2f}   "
                      + "  ".join(f"{k} {pct(statistics.median(v))}" for k, v in sorted(kinds.items())))
        print()
    print("What speed costs, at pngdeer's JPEG's score: bytes against the same encoder at speed 6 (median / total), encode time against it (median)")
    for tool, base in [("aom-s4-444-10bit", "aom-s6-444-10bit"), ("aom-s5-444-10bit", "aom-s6-444-10bit"), ("aom-s8-444-10bit", "aom-s6-444-10bit"), ("aom-s9-444-10bit", "aom-s6-444-10bit"),
                       ("rav1e-s8", "rav1e-s6"), ("rav1e-s10", "rav1e-s6")]:
        ratios, sizes, times = [], [0, 0], []
        for i in images:
            rows, base_rows = [r for r in avif[i] if r["tool"] == tool], [r for r in avif[i] if r["tool"] == base]
            if not rows or not base_rows:
                continue
            target = float(pick(jpeg[i], "pngdeer auto")["score"])
            a, b = (bytes_at_score(frontier([as_row(r["score"], r["bytes"]) for r in rs]), target) for rs in (rows, base_rows))
            if a is not None and b is not None:
                ratios.append(a / b)
                sizes[0] += a
                sizes[1] += b
            times.append(sum(int(r["ms"]) for r in rows) / sum(int(r["ms"]) for r in base_rows))
        if ratios:
            print(f"  {tool:18} {pct(statistics.median(ratios))} / {pct(sizes[0] / sizes[1])}  time {pct(statistics.median(times))}  ({len(ratios)} photos)")


def summary_pngs(path: str, *bench_paths: str) -> None:
    avif, results, seen = by_image(path), defaultdict(list), set()
    for p in bench_paths:
        rows = list(csv.DictReader(open(p)))
        have = {(r["image"], r["pipeline"]) for r in rows} - seen
        for r in rows:
            if (r["image"], r["pipeline"]) in have:
                results[r["image"]].append(r)
        seen |= have
    images = sorted(avif)
    categories = sorted({avif[i][0]["category"] for i in images})
    print(f"{len(images)} images, at TinyPNG's PNG score: AVIF reached; AVIF's and pngdeer's WebP's bytes against TinyPNG's PNG (median, where reached);")
    print("AVIF against pngdeer's WebP (median, total, smaller in); the smaller of the two per image against pngdeer's WebP (total)")
    for tool in [t for t in PNG_CONFIGS if any(r["tool"] == t for i in images for r in avif[i])]:
        print(f"\n{tool}")
        for cat in categories + ["all"]:
            vs_tiny, wp_tiny, vs_wp, both, best, reached, n = [], [], [], [0, 0], 0, 0, 0
            for i in images:
                if cat != "all" and avif[i][0]["category"] != cat:
                    continue
                n += 1
                t = next(r for r in results[i] if r["pipeline"] == "tinypng")
                target, tiny_bytes = float(t["ssimulacra2"]), int(t["out_bytes"])
                a = bytes_at_score(frontier([as_row(r["score"], r["bytes"]) for r in avif[i] if r["tool"] == tool]), target)
                wp = bytes_at_score(frontier([as_row(r["ssimulacra2"], r["out_bytes"]) for r in results[i]
                                              if r["pipeline"].startswith("wp") and r["pipeline"] != "wpauto" and r["ssimulacra2"]]), target)
                reached += a is not None
                if a is not None:
                    vs_tiny.append(a / tiny_bytes)
                if wp is not None:
                    wp_tiny.append(wp / tiny_bytes)
                    if a is not None:
                        vs_wp.append(a / wp)
                    both[0] += min(a, wp) if a is not None else wp
                    both[1] += wp
                    best += 1
            line = f"  {cat:14} {reached:3}/{n:<3}"
            if vs_tiny:
                line += f"  AVIF {pct(statistics.median(vs_tiny))} of TinyPNG, pngdeer WebP {pct(statistics.median(wp_tiny))}"
            if vs_wp:
                line += f"  AVIF/WebP {pct(statistics.median(vs_wp))} median, smaller in {sum(x < 1 for x in vs_wp)}/{len(vs_wp)}"
            if best:
                line += f"  smaller of the two {pct(both[0] / both[1])}"
            print(line)


def summary_tiny_pngs(path: str, *bench_paths: str) -> None:
    """On the images with TinyPNG's AVIF: each config's bytes at its score, and the best config per
    image (each on its own curve; libaom only), against it; TinyPNG's AVIF against its PNG."""
    avif, results = by_image(path), by_image(*bench_paths)
    images = sorted(i for i in avif if any(r["tool"] == "tinypng" for r in avif[i]))
    tools = [t for t in PNG_CONFIGS if any(r["tool"] == t for i in images for r in avif[i])]
    aom_tools = [t for t in tools if t.startswith("aom")]
    print(f"{len(images)} images. TinyPNG's AVIF against its PNG (median bytes, score); then at TinyPNG's AVIF's score, bytes against it (median, wins)")
    for cat in sorted({avif[i][0]["category"] for i in images}) + ["all"]:
        mine = [i for i in images if cat == "all" or avif[i][0]["category"] == cat]
        tiny = {i: next(r for r in avif[i] if r["tool"] == "tinypng") for i in mine}
        png = {i: next(r for r in results[i] if r["pipeline"] == "tinypng") for i in mine}
        line = (f"{cat:13} {len(mine):2}  vs its PNG {pct(statistics.median(int(tiny[i]['bytes']) / int(png[i]['out_bytes']) for i in mine))} "
                f"{statistics.median(float(tiny[i]['score']) - float(png[i]['ssimulacra2']) for i in mine):+5.1f}  ")
        best = []
        for tool in tools:
            ratios = []
            for i in mine:
                size = bytes_at_score(frontier([as_row(r["score"], r["bytes"]) for r in avif[i] if r["tool"] == tool]), float(tiny[i]["score"]))
                ratios.append(size / int(tiny[i]["bytes"]) if size is not None else None)
            reached = [r for r in ratios if r is not None]
            line += f"| {tool} {pct(statistics.median(reached)) if reached else '-'} {sum(r < 1 for r in reached)}/{len(mine)} "
        for i in mine:
            sizes = [bytes_at_score(frontier([as_row(r["score"], r["bytes"]) for r in avif[i] if r["tool"] == t]), float(tiny[i]["score"])) for t in aom_tools]
            sizes = [s for s in sizes if s is not None]
            best.append(min(sizes) / int(tiny[i]["bytes"]) if sizes else None)
        reached = [b for b in best if b is not None]
        line += f"| best libaom {pct(statistics.median(reached))} {sum(b < 1 for b in reached)}/{len(mine)}"
        print(line)
    print("\nPer image, the best libaom setting against TinyPNG's AVIF:")
    for i in images:
        t = next(r for r in avif[i] if r["tool"] == "tinypng")
        sizes = {tool: bytes_at_score(frontier([as_row(r["score"], r["bytes"]) for r in avif[i] if r["tool"] == tool]), float(t["score"])) for tool in aom_tools}
        sizes = {k: v for k, v in sizes.items() if v is not None}
        k = min(sizes, key=sizes.get) if sizes else None
        print(f"  {i[:44]:44} TinyPNG {t['bytes']:>7} bytes, score {float(t['score']):5.1f}   " + (f"{pct(sizes[k] / int(t['bytes']))} ({k})" if k else "not reached"))


def speed(paths: list[str]) -> None:
    """One encode at a time: libaom (avifenc, one thread, 4:4:4 10-bit) and rav1e natively and in
    wasm (node), at quality 60, and pngdeer's whole JPEG and WebP compression for comparison."""
    with tempfile.TemporaryDirectory() as t:
        t = Path(t)
        for path in paths:
            photo, src, out = Path(path), t / "src.png", t / "o.avif"
            subprocess.run([AVIFLAB, "png", photo, src], check=True)
            w, h, _ = bench.png_info(src)
            print(f"{photo.name}: {w * h / 1e6:.1f} MP")
            for s in [4, 6, 8, 9]:
                print(f"  libaom s{s}       {encode(aom(s, '-d', '10'), 60, src, out, False) / 1000:6.1f} s  {out.stat().st_size} bytes")
            for s in [4, 6, 8, 10]:
                print(f"  rav1e s{s}        {encode(rav1e(s), 60, src, out, True) / 1000:6.1f} s  {out.stat().st_size} bytes")
            for s in [6, 8, 10]:
                ms = int(subprocess.run(["node", ROOT / "experiments/avif/wasm.mjs", AVIFLAB_WASM, src, out, "60", str(s)], capture_output=True, text=True, check=True).stdout)
                print(f"  rav1e wasm s{s}   {ms / 1000:6.1f} s  {out.stat().st_size} bytes")
            for fmt in ["jpg", "webp"]:
                start = time.perf_counter()
                subprocess.run([PNGDEER, photo, "-o", t / f"p.{fmt}"], check=True, capture_output=True)
                print(f"  pngdeer {fmt:5}    {time.perf_counter() - start:6.1f} s  (whole search)")


if __name__ == "__main__":
    command, args = sys.argv[1], sys.argv[2:]
    if command == "tiny":
        tiny(Path(args[0]), Path(args[1]))
    elif command == "photos":
        photos(Path(args[0]), Path(args[1]), Path(args[2]), args[3:] or [c for c in PHOTO_CONFIGS if "-s6" in c])
    elif command == "tiny-pngs":
        tiny_pngs(Path(args[0]), Path(args[1]))
    elif command == "pngs":
        pngs(Path(args[0]), Path(args[1]), Path(args[2]), args[3:] or ["aom-s6", "rav1e-s6"])
    elif command == "summary":
        summary(*args)
    elif command == "summary-pngs":
        summary_pngs(*args)
    elif command == "summary-tiny-pngs":
        summary_tiny_pngs(*args)
    elif command == "speed":
        speed(args)
    else:
        sys.exit(__doc__)
