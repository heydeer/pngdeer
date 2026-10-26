#!/usr/bin/env python3
"""Blind A/B test: a challenger family (pd, pngdeer, by default) against TinyPNG, judged by eye.

Serves a local page that shows each image's two outputs as A and B in random order, plus the
original for reference. Votes go to <run>/blind/, next to the benchmark results they come from.

  python3 bench/blind.py                      # pngdeer vs TinyPNG, the pd output closest in file size
  python3 bench/blind.py --match quality      # smallest pd output scoring at least TinyPNG's
  python3 bench/blind.py --challenger pq      # pngquant's fixed levels instead
  python3 bench/blind.py --challenger liq --results bench/results/20261023-151136   # a run from before the rename
"""

import argparse
import csv
import json
import math
import random
import re
import statistics
import sys
import time
from collections import defaultdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench  # noqa: E402

BENCH_DIR = Path(__file__).resolve().parent
PAGE = BENCH_DIR / "blind.html"


def family(pipeline: str) -> str:
    """pd85 -> pd, pq80-d50 -> pq-d50 (also older runs' names, such as liq85)."""
    return re.sub(r"^([a-z]+)\d+", r"\1", pipeline)


def build_pairs(run: Path, challenger: str, match: str) -> list[dict]:
    by_image: dict[str, dict[str, dict]] = defaultdict(dict)
    for r in csv.DictReader(open(run / "results.csv")):
        if r["ssimulacra2"]:
            by_image[r["image"]][r["pipeline"]] = r
    pairs = []
    for image, outputs in sorted(by_image.items()):
        tiny = outputs.get("tinypng")
        mine = [r for name, r in outputs.items() if name != "tinypng" and family(name) == challenger]
        if tiny is None or not mine:
            continue
        tiny_bytes, tiny_score = int(tiny["out_bytes"]), float(tiny["ssimulacra2"])
        if match == "size":
            pick = min(mine, key=lambda r: abs(math.log(int(r["out_bytes"]) / tiny_bytes)))
        else:
            reaching = [r for r in mine if float(r["ssimulacra2"]) >= tiny_score]
            pick = min(reaching, key=lambda r: int(r["out_bytes"])) if reaching else max(mine, key=lambda r: float(r["ssimulacra2"]))
        pairs.append({
            "image": image,
            "category": tiny["category"],
            "width": int(tiny["width"]),
            "height": int(tiny["height"]),
            "alpha": has_alpha(run / "tinypng" / image) or "alpha" in tiny["color"] or tiny["color"].startswith("rgba"),
            "challenger": pick["pipeline"],
        })
    return pairs


def has_alpha(path: Path) -> bool:
    return any(kind == b"tRNS" for kind, _ in bench.png_chunks(path.read_bytes()))


class Session:
    """Pair order and A/B assignment, fixed on first start so reloads and restarts keep them."""

    def __init__(self, run: Path, challenger: str, match: str, images_dir: Path):
        self.run, self.images_dir = run, images_dir
        self.dir = run / "blind"
        self.dir.mkdir(exist_ok=True)
        stem = f"{challenger}-vs-tinypng-{match}"
        self.session_file, self.votes_file = self.dir / f"{stem}.json", self.dir / f"{stem}.votes.jsonl"
        if self.session_file.exists():
            self.pairs = json.loads(self.session_file.read_text())["pairs"]
        else:
            self.pairs = build_pairs(run, challenger, match)
            rng = random.Random()
            rng.shuffle(self.pairs)
            for i, p in enumerate(self.pairs):
                p["id"] = i
                p["a"], p["b"] = rng.sample([p["challenger"], "tinypng"], 2)
            self.session_file.write_text(json.dumps({"challenger": challenger, "match": match, "pairs": self.pairs}, indent=1))
        self.challenger, self.match = challenger, match
        self.results = {(r["image"], r["pipeline"]): r for r in csv.DictReader(open(run / "results.csv"))}
        self.votes: dict[int, dict] = {}
        if self.votes_file.exists():
            for line in self.votes_file.read_text().splitlines():
                v = json.loads(line)
                self.votes[v["id"]] = v

    def side(self, pair: dict, pipeline: str) -> dict:
        r = self.results[(pair["image"], pipeline)]
        label = "TinyPNG" if pipeline == "tinypng" else pipeline
        return {"pipeline": pipeline, "label": label, "bytes": int(r["out_bytes"]), "score": float(r["ssimulacra2"]), "params": r["params"]}

    def reveal(self, pair: dict) -> dict:
        return {"a": self.side(pair, pair["a"]), "b": self.side(pair, pair["b"]), "choice": self.votes[pair["id"]]["choice"]}

    def public(self) -> dict:
        pairs = []
        for p in self.pairs:
            item = {k: p[k] for k in ("id", "category", "width", "height", "alpha")}
            item["reveal"] = self.reveal(p) if p["id"] in self.votes else None
            pairs.append(item)
        return {"challenger": self.challenger, "match": self.match, "pairs": pairs}

    def vote(self, pair_id: int, choice: str) -> dict:
        pair = self.pairs[pair_id]
        picked = {"a": pair["a"], "b": pair["b"]}.get(choice, "same")
        record = {"id": pair_id, "image": pair["image"], "choice": choice, "picked": picked, "a": pair["a"], "b": pair["b"], "time": time.time()}
        with open(self.votes_file, "a") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
        self.votes[pair_id] = record
        return self.reveal(pair)

    def image_bytes(self, pair_id: int, which: str) -> bytes:
        pair = self.pairs[pair_id]
        if which == "o":
            path = self.images_dir / pair["image"]
        else:
            path = self.run / pair[which] / pair["image"]
        return bench.without_color_chunks(path.read_bytes())

    def summary(self) -> dict:
        def tally(votes: list[dict]) -> dict:
            return {
                "n": len(votes),
                "challenger": sum(v["picked"] not in ("tinypng", "same") for v in votes),
                "same": sum(v["picked"] == "same" for v in votes),
                "tinypng": sum(v["picked"] == "tinypng" for v in votes),
            }

        votes = [self.votes[p["id"]] | {"pair": p} for p in self.pairs if p["id"] in self.votes]
        by_cat = defaultdict(list)
        for v in votes:
            by_cat[v["pair"]["category"]].append(v)
        ratios, agree, decided = [], 0, 0
        for v in votes:
            mine = self.side(v["pair"], v["pair"]["challenger"])
            tiny = self.side(v["pair"], "tinypng")
            ratios.append(mine["bytes"] / tiny["bytes"])
            if v["picked"] != "same" and mine["score"] != tiny["score"]:
                decided += 1
                higher = v["pair"]["challenger"] if mine["score"] > tiny["score"] else "tinypng"
                agree += v["picked"] == higher
        return {
            "challenger": self.challenger,
            "total": len(self.pairs),
            "overall": tally(votes),
            "categories": {c: tally(vs) for c, vs in sorted(by_cat.items())},
            "size_ratio_median": statistics.median(ratios) if ratios else None,
            "size_ratio_geomean": math.exp(statistics.mean(math.log(r) for r in ratios)) if ratios else None,
            "metric_agreement": {"agree": agree, "decided": decided},
        }


def make_handler(session: Session):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, fmt, *args):
            pass

        def send(self, code: int, body: bytes, content_type: str, cache: bool = False) -> None:
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "max-age=86400" if cache else "no-store")
            self.end_headers()
            self.wfile.write(body)

        def send_json(self, obj, code: int = 200) -> None:
            self.send(code, json.dumps(obj, ensure_ascii=False).encode(), "application/json; charset=utf-8")

        def do_GET(self):
            if self.path in ("/", "/index.html"):
                self.send(200, PAGE.read_bytes(), "text/html; charset=utf-8")
            elif self.path == "/api/session":
                self.send_json(session.public())
            elif self.path == "/api/summary":
                self.send_json(session.summary())
            elif m := re.fullmatch(r"/img/(\d+)/([abo])\.png", self.path):
                pair_id = int(m.group(1))
                if pair_id >= len(session.pairs):
                    return self.send_json({"error": "no such pair"}, 404)
                self.send(200, session.image_bytes(pair_id, m.group(2)), "image/png", cache=True)
            else:
                self.send_json({"error": "not found"}, 404)

        def do_POST(self):
            if self.path != "/api/vote":
                return self.send_json({"error": "not found"}, 404)
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
            pair_id, choice = body.get("id"), body.get("choice")
            if not isinstance(pair_id, int) or not 0 <= pair_id < len(session.pairs) or choice not in ("a", "b", "same"):
                return self.send_json({"error": "bad vote"}, 400)
            self.send_json(session.vote(pair_id, choice))

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--results", type=Path, default=BENCH_DIR / "results" / "latest", help="benchmark run to judge (default: latest)")
    parser.add_argument("--images", type=Path, default=BENCH_DIR / "images", help="originals, as passed to bench.py")
    parser.add_argument("--challenger", default="pd", help="pipeline family to put against TinyPNG (default: pd)")
    parser.add_argument("--match", choices=("size", "quality"), default="size",
                        help="size: challenger output closest in bytes; quality: smallest one scoring >= TinyPNG")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()

    run = args.results.resolve()
    if not (run / "results.csv").exists():
        sys.exit(f"no results.csv in {run}; run bench/bench.py first")
    session = Session(run, args.challenger, args.match, args.images.resolve())
    if not session.pairs:
        sys.exit(f"no images have both tinypng and {args.challenger} results in {run}")
    print(f"{len(session.pairs)} pairs ({args.challenger} vs TinyPNG, matched by {args.match}), {len(session.votes)} voted")
    print(f"votes: {session.votes_file}")
    print(f"open http://127.0.0.1:{args.port}/", flush=True)
    ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(session)).serve_forever()


if __name__ == "__main__":
    main()
