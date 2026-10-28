"""python3 experiments/webp/compare.py RUN [RUN ...]: pngdeer's WebP output (wpauto) against its PNG
(pdauto) and against libwebp's own cwebp, from bench.py runs with the pipelines
pdauto,wpauto,cw50,...,cw100,cwll. Several runs are pooled by image (the main and held-out sets);
where two have the same pipeline for an image, the later run's row counts (a rerun of wpauto
alone, say).

- WebP / PNG: per image, wpauto's bytes over pdauto's at the same automatic target (both reach it),
  and the score difference; by category and by which WebP file won.
- cwebp at equal quality: cwebp's size/quality curve (every cw pipeline, lossless included) per
  image, read at wpauto's score: how many bytes cwebp needs for the same quality, had its quality
  been picked per image after the fact (log size interpolated linearly in score, as bench.py does
  against TinyPNG).
- A second wpauto-like pipeline (e.g. one run with --pd-args), if given as --other NAME, is set
  against wpauto the same way as the PNG.
- TinyPNG's WebP (tinywebp), where a run has it: pngdeer's WebP curve (every wp pipeline, wpT and
  wpauto) and cwebp's read at TinyPNG's score (bytes for the same quality) and at its size (score
  for the same bytes), as bench.py places TinyPNG's PNG on the PNG tools' curves. --tiny NAME reads
  another pipeline in its place (tiny_emulate.py's stand-in, tinyemu), and where a run has both,
  the stand-in is checked against the real files.
"""

import argparse
import csv
import math
import statistics
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "bench"))
from bench import bytes_at_score, score_at_bytes  # noqa: E402


def load(runs: list[Path]) -> dict[str, dict[str, dict]]:
    by_image: dict[str, dict[str, dict]] = defaultdict(dict)
    for run in runs:
        for r in csv.DictReader(open(run / "results.csv")):
            if r["out_bytes"] and r["ssimulacra2"]:
                key = r["image"]  # later runs replace earlier ones' rows of the same pipeline
                row = {"bytes": int(r["out_bytes"]), "score": float(r["ssimulacra2"]), "category": r["category"], "params": r["params"], "seconds": float(r["seconds"]) if r["seconds"] else None}
                if r["pipeline"] == "tinywebp":
                    webp = run / "tinywebp" / Path(r["image"]).with_suffix(".webp")
                    row["kind"] = "lossy" if webp.exists() and b"VP8 " in riff_kinds(webp.read_bytes()) else "lossless"
                by_image[key][r["pipeline"]] = row
    return by_image


def riff_kinds(webp: bytes) -> list[bytes]:
    kinds, pos = [], 12
    while pos + 8 <= len(webp):
        size = int.from_bytes(webp[pos + 4 : pos + 8], "little")
        kinds.append(webp[pos : pos + 4])
        pos += 8 + size + size % 2
    return kinds


def frontier(points: list[tuple[float, int]]) -> list[tuple[float, int]]:
    front = []
    for score, size in sorted(points, key=lambda p: (-p[0], p[1])):
        if not front or size < front[-1][1]:
            front.append((score, size))
    return front[::-1]


def pct(x: float) -> str:
    return f"{x * 100:.1f}%"


def table(headers: list[str], rows: list[list]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    return "\n".join(lines + ["| " + " | ".join(str(c) for c in r) + " |" for r in rows])


def vs(pairs: list[tuple[dict, dict]], label: str) -> list:
    """`a` against `b` for each pair: size ratio (median and total) and score difference."""
    if not pairs:
        return [label, 0, "-", "-", "-", "-"]
    ratios = sorted(a["bytes"] / b["bytes"] for a, b in pairs)
    return [
        label,
        len(pairs),
        pct(statistics.median(ratios)),
        pct(sum(a["bytes"] for a, _ in pairs) / sum(b["bytes"] for _, b in pairs)),
        f"{pct(ratios[len(ratios) // 10])} – {pct(ratios[-1 - len(ratios) // 10])}",
        f"{statistics.median(a['score'] - b['score'] for a, b in pairs):+.2f}",
    ]


def on_curve(outputs: dict[str, dict], prefix: str, tiny: dict) -> tuple[float | None, float | None]:
    """The curve of the pipelines named prefix* read at TinyPNG's score (bytes) and size (score)."""
    front = frontier([(v["score"], v["bytes"]) for name, v in outputs.items() if name.startswith(prefix)])
    if not front:
        return None, None
    return bytes_at_score(front, tiny["score"]), score_at_bytes(front, tiny["bytes"])


def tiny_rows(pairs: list[tuple[float | None, float | None, dict]], label: str) -> list:
    reached = [(b, t) for b, _, t in pairs if b is not None]
    deltas = sorted(s - t["score"] for _, s, t in pairs if s is not None)
    if not pairs:
        return [label, 0, "-", "-", "-", "-", "-"]
    return [
        label,
        len(pairs),
        pct(statistics.median(b / t["bytes"] for b, t in reached)) if reached else "-",
        pct(sum(b for b, _ in reached) / sum(t["bytes"] for _, t in reached)) if reached else "-",
        len(pairs) - len(reached),
        f"{statistics.median(deltas):+.1f}" if deltas else "-",
        f"{deltas[len(deltas) // 10]:+.1f}" if deltas else "-",
    ]


def tiny_section(data: dict[str, dict[str, dict]], name: str = "tinywebp") -> None:
    both = [o for o in data.values() if "tinyemu" in o and "tinywebp" in o]
    if both:
        print(f"\n## The stand-in (tinyemu) against TinyPNG's real WebP, {len(both)} images\n")
        same = sum(o["tinyemu"]["params"] == ("lossy" if o["tinywebp"]["kind"] == "lossy" else "lossless") for o in both)
        print(f"Same kind (lossy or lossless) on {same} of {len(both)}.\n")
        print(table(["Images", "n", "Size ratio (median)", "Size ratio (total)", "P10 – P90", "Score difference (median)"], [vs([(o["tinyemu"], o["tinywebp"]) for o in both], "tinyemu / tinywebp")]))
    images = {img: dict(o, tinywebp=o[name]) for img, o in data.items() if name in o}
    if not images:
        return
    print(f"\n## TinyPNG's WebP ({name}), {len(images)} images\n")
    tiny = [o["tinywebp"] for o in images.values()]
    print(f"TinyPNG's WebP scores {statistics.median(t['score'] for t in tiny):.1f} on the median image "
          f"(P10 {sorted(t['score'] for t in tiny)[len(tiny) // 10]:.1f})" + (f"; its PNG {statistics.median(png):.1f}.\n" if (png := [o['tinypng']['score'] for o in images.values() if 'tinypng' in o]) else ".\n"))
    headers = ["Curve", "n", "Bytes at TinyPNG's score / TinyPNG's (median)", "(total)", "Can't reach its score", "Score at TinyPNG's size, difference (median)", "P10"]
    rows = []
    for label, prefix in (("pngdeer WebP (wp*)", "wp"), ("cwebp (cw*)", "cw")):
        pairs = [(*on_curve(o, prefix, o["tinywebp"]), o["tinywebp"]) for o in images.values()]
        rows.append(tiny_rows(pairs, label))
        if prefix == "wp":
            for cat in sorted({o["tinywebp"]["category"] for o in images.values()}):
                rows.append(tiny_rows([(*on_curve(o, prefix, o["tinywebp"]), o["tinywebp"]) for o in images.values() if o["tinywebp"]["category"] == cat], f"  {cat}"))
    print(table(headers, rows))
    headers = ["Images", "n", "Size ratio (median)", "Size ratio (total)", "P10 – P90", "Score difference (median)"]
    print("\nOne file each, as they come (different qualities):\n")
    print(table(headers, [
        vs([(o["wpauto"], o["tinywebp"]) for o in images.values() if "wpauto" in o], "pngdeer WebP (wpauto) / TinyPNG's WebP"),
        *([vs([(o["tinywebp"], o["tinypng"]) for o in images.values() if "tinypng" in o], "TinyPNG's WebP / TinyPNG's PNG")] if any("tinypng" in o for o in images.values()) else []),
        vs([(o["tinywebp"], o["pdauto"]) for o in images.values() if "pdauto" in o], "TinyPNG's WebP / pngdeer PNG (pdauto)"),
    ]))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runs", nargs="+", type=Path)
    parser.add_argument("--other", help="a second WebP pipeline to set against wpauto")
    parser.add_argument("--tiny", default="tinywebp", help="the pipeline that stands for TinyPNG's WebP (tinyemu: tiny_emulate.py's)")
    parser.add_argument("--without", help="only images without this pipeline (e.g. tinywebp: those TinyPNG's WebP wasn't fetched for)")
    args = parser.parse_args()
    data = load(args.runs)
    if args.without:
        data = {img: o for img, o in data.items() if args.without not in o}
    images = [o for o in data.values() if "wpauto" in o and "pdauto" in o]
    print(f"{len(images)} images with wpauto and pdauto ({', '.join(r.name for r in args.runs)})\n")

    headers = ["Images", "n", "Size ratio (median)", "Size ratio (total)", "P10 – P90", "Score difference (median)"]
    print("## WebP (wpauto) / PNG (pdauto), same automatic target\n")
    rows = [vs([(o["wpauto"], o["pdauto"]) for o in images], "all")]
    for mode in ("lossy", "lossless"):
        rows.append(vs([(o["wpauto"], o["pdauto"]) for o in images if o["wpauto"]["params"].startswith(mode)], f"{mode} won"))
    for cat in sorted({o["wpauto"]["category"] for o in images}):
        rows.append(vs([(o["wpauto"], o["pdauto"]) for o in images if o["wpauto"]["category"] == cat], cat))
    print(table(headers, rows))

    print("\n## cwebp's bytes at wpauto's score / wpauto's bytes (cwebp's quality picked per image afterwards)\n")
    cw_rows, out_of_reach = [], 0
    by_cat: dict[str, list[float]] = defaultdict(list)
    for o in images:
        points = [(v["score"], v["bytes"]) for name, v in o.items() if name.startswith("cw")]
        if not points:
            continue
        size = bytes_at_score(frontier(points), o["wpauto"]["score"])
        if size is None:
            out_of_reach += 1
            continue
        ratio = size / o["wpauto"]["bytes"]
        cw_rows.append((ratio, size, o["wpauto"]["bytes"]))
        by_cat[o["wpauto"]["category"]].append(ratio)
    if cw_rows:
        ratios = sorted(r for r, _, _ in cw_rows)
        print(f"{len(cw_rows)} images ({out_of_reach} where no cwebp setting reaches wpauto's score): cwebp needs "
              f"{pct(statistics.median(ratios))} of wpauto's bytes on the median image, {pct(sum(s for _, s, _ in cw_rows) / sum(w for _, _, w in cw_rows))} in total "
              f"(P10 {pct(ratios[len(ratios) // 10])}, P90 {pct(ratios[-1 - len(ratios) // 10])})\n")
        print(table(["Category", "n", "cwebp / wpauto (median)"], [[c, len(v), pct(statistics.median(v))] for c, v in sorted(by_cat.items())]))
    cw75 = [(o["wpauto"], o["cw75"]) for o in images if "cw75" in o]
    if cw75:
        print("\nwpauto against plain cwebp -q 75 (one setting for every image):\n")
        print(table(headers, [vs(cw75, "wpauto / cw75")]))

    seconds = lambda name: [o[name]["seconds"] for o in images if o.get(name, {}).get("seconds") is not None]  # noqa: E731
    print("\n## Time (seconds per image, median / mean / max)\n")
    print(table(["Pipeline", "Median", "Mean", "Max"], [[n, f"{statistics.median(s):.2f}", f"{statistics.mean(s):.2f}", f"{max(s):.1f}"] for n in ("pdauto", "wpauto", args.other) if n and (s := seconds(n))]))

    tiny_section(data, args.tiny)

    if args.other:
        pairs = [(o[args.other], o["wpauto"]) for o in data.values() if args.other in o and "wpauto" in o]
        print(f"\n## {args.other} / wpauto\n")
        print(table(headers, [vs(pairs, "all")] + [vs([(a, b) for a, b in pairs if b["category"] == cat], cat) for cat in sorted({b["category"] for _, b in pairs})]))


if __name__ == "__main__":
    main()
