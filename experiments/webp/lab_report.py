"""python3 experiments/webp/lab_report.py LAB.tsv TARGETS.tsv [--pick A,B,...]: summarizes webplab's
output (tools/webplab) on images whose target is TinyPNG's WebP score.

TARGETS.tsv is webplab's LIST with a third column, TinyPNG's WebP file (path, target, file).
- Per setting: bytes at the target against the default setting's (base), and against TinyPNG's.
- Per image, the best of several settings picked after the fact (all of them, or --pick's): how
  much trying them all would save, and which win.
- Near-lossless: where a near-lossless file reaches the target in fewer bytes than the lossy one.
"""

import argparse
import collections
import statistics
from pathlib import Path


def pct(x: float) -> str:
    return f"{x * 100:.1f}%"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("lab", type=Path)
    parser.add_argument("targets", type=Path)
    parser.add_argument("--pick", help="settings to choose among per image (default: all lossy ones)")
    args = parser.parse_args()

    tiny, kind, target = {}, {}, {}
    for line in args.targets.read_text().splitlines():
        path, score, webp = line.split("\t")
        data = Path(webp).read_bytes()
        tiny[path], target[path] = len(data), float(score)
        kind[path] = "lossy" if b"VP8 " in data[12:64] or b"ALPH" in data[12:64] else "lossless"
    rows = collections.defaultdict(dict)
    for line in args.lab.read_text().splitlines():
        path, name, quality, size, score, _ = line.split("\t")
        rows[path][name] = (float(quality), int(size), float(score))
    settings = [n for n in next(iter(rows.values())) if not n.startswith("near")]
    print(f"{len(rows)} images ({sum(k == 'lossy' for k in kind.values())} where TinyPNG's WebP is lossy)\n")

    def reached(o, n):
        return n in o and o[n][0] >= 0

    print("| Setting | Reaches the target | Bytes / base's, median | total | Bytes / TinyPNG's (lossy ones), median | total |")
    print("|---|---|---|---|---|---|")
    for n in settings:
        both = [(o[n][1], o["base"][1]) for o in rows.values() if reached(o, n) and reached(o, "base")]
        tl = [(o[n][1], tiny[p]) for p, o in rows.items() if reached(o, n) and kind[p] == "lossy"]
        print(f"| {n} | {sum(reached(o, n) for o in rows.values())} | {pct(statistics.median(a / b for a, b in both))} | {pct(sum(a for a, _ in both) / sum(b for _, b in both))} | "
              f"{pct(statistics.median(a / b for a, b in tl))} | {pct(sum(a for a, _ in tl) / sum(b for _, b in tl))} |")

    pick = args.pick.split(",") if args.pick else settings
    best = {}
    for p, o in rows.items():
        options = [(o[n][1], n) for n in pick if reached(o, n)]
        if options and reached(o, "base"):
            best[p] = min(options)
    wins = collections.Counter(n for _, n in best.values())
    sel = [(b, rows[p]["base"][1], tiny[p], kind[p]) for p, (b, _) in best.items()]
    print(f"\nBest of {', '.join(pick)} per image, picked after the fact ({len(sel)} images): "
          f"{pct(statistics.median(b / a for b, a, _, _ in sel))} of base's bytes on the median image, {pct(sum(b for b, *_ in sel) / sum(a for _, a, _, _ in sel))} in total; "
          f"against TinyPNG's lossy files {pct(statistics.median(b / t for b, _, t, k in sel if k == 'lossy'))} median, "
          f"{pct(sum(b for b, _, t, k in sel if k == 'lossy') / sum(t for b, _, t, k in sel if k == 'lossy'))} total")
    print("Winners:", ", ".join(f"{n} {c}" for n, c in wins.most_common()))

    print("\nNear-lossless reaching the target in fewer bytes than the best lossy setting:")
    for p, o in sorted(rows.items()):
        near = [(o[n][1], n, o[n][2]) for n in o if n.startswith("near") and o[n][2] >= target[p]]
        lossy = best.get(p, (10**12, None))[0]
        if near and min(near)[0] < lossy:
            size, n, score = min(near)
            print(f"  {p}: {n} {size} B (score {score:.1f}, target {target[p]:.1f}); best lossy {lossy if lossy < 10**12 else 'none'}; TinyPNG {tiny[p]} ({kind[p]})")


if __name__ == "__main__":
    main()
