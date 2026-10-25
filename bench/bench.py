#!/usr/bin/env python3
"""PNG compression benchmark: pngdeer, pngquant and other open-source quantizers vs TinyPNG.

Every output is scored against its original with SSIMULACRA2
(100 = identical, 90 = indistinguishable at 1:1, 70 = high quality, 50 = medium).

Pipelines (pick with --pipelines):
  oxipng   lossless re-encode only (reference point)
  pdT      pngdeer --target T: the smallest file scoring >= T, exactly as the CLI and browser make it
  pdauto   pngdeer's automatic quality (its default, what the browser does)
  pqN      pngquant --quality 0-N, then oxipng (e.g. pq80)
  pqN-dD   same with dithering level D/100 (pq80-d50 = --floyd=0.5, pq80-d0 = --nofs)
  <tool>N  other open-source quantizers at N colors, then oxipng (see QUANT_TOOLS)
  tinypng  TinyPNG API (results cached in bench/.cache/tinypng to save quota)
"""

import argparse
import base64
import csv
import hashlib
import json
import math
import os
import re
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass, fields
from datetime import datetime
from pathlib import Path


BENCH_DIR = Path(__file__).resolve().parent
TINYPNG_SHRINK_URL = "https://api.tinify.com/shrink"
DEFAULT_PIPELINES = "oxipng,pq100,pd70,pd75,pd80,pd85,pd90,pd95,pdauto,tinypng"
COLOR_TYPES = {0: "gray", 2: "rgb", 3: "palette", 4: "gray+alpha", 6: "rgba"}
# Dropped from inputs before any tool sees them, and from outputs before scoring: pngquant converts
# gAMA/cHRM/iCCP images to sRGB while TinyPNG and oxipng drop the chunks, and ssimulacra2 honors
# them, so leaving them in measures metadata policy instead of compression.
COLOR_CHUNKS = {b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"cICP", b"mDCV", b"cLLI"}


@dataclass(frozen=True)
class Pipeline:
    name: str
    kind: str  # "oxipng" | "pngdeer" | "pngquant" | "quant" | "tinypng"
    quality: int | None = None  # pngquant: --quality; quant: palette size
    dither: int | None = None  # pngquant and quant; None = the tool's default dithering
    target: int | None = None  # pngdeer: SSIMULACRA2 score to reach (None: automatic)
    tool: str | None = None  # quant: a QUANT_TOOLS key

    @property
    def family(self) -> str:
        return re.sub(r"^(pq|pd|" + "|".join(QUANT_TOOLS) + r")\d+", r"\1", self.name)


@dataclass(frozen=True)
class Image:
    path: Path
    rel: Path
    category: str


@dataclass
class Row:
    image: str
    category: str
    width: int
    height: int
    color: str
    pipeline: str
    orig_bytes: int
    out_bytes: int | None = None
    ratio: float | None = None  # out_bytes / orig_bytes
    ssimulacra2: float | None = None
    seconds: float | None = None
    pngquant_bytes: int | None = None  # pngquant output before oxipng
    params: str = ""  # pngdeer only: what the search picked
    note: str = ""


@dataclass
class Config:
    pngquant: str
    oxipng: str
    ssimulacra2: str
    oxipng_level: str
    zopfli: bool
    pq_speed: int | None
    pngdeer: str | None = None
    pd_variants: str | None = None  # pngdeer --variants (None: its default)
    pd_preview_pixels: int | None = None  # pngdeer --preview-pixels (None: its default, as shipped)
    quantlab: str | None = None  # tools/quantlab, for the exq / qte / nq / qzr pipelines
    small_side: int = 256  # score images with a shorter longer side enlarged; 0 = off


class TinyPNGError(Exception):
    def __init__(self, message: str, status: int | None = None):
        super().__init__(message)
        self.status = status


class TinyPNG:
    def __init__(self, api_key: str, cache_dir: Path, concurrency: int = 4):
        self.auth = "Basic " + base64.b64encode(f"api:{api_key}".encode()).decode()
        self.cache_dir = cache_dir
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.slots = threading.Semaphore(concurrency)
        self.compression_count: int | None = None
        # Set on 401/429 so the remaining images fail fast instead of hammering the API.
        self.fatal: str | None = None

    def validate(self) -> None:
        """Checks the key without spending a compression: an empty upload answers 400 if the key is good."""
        try:
            self._request("POST", TINYPNG_SHRINK_URL, b"")
        except TinyPNGError as e:
            if e.status != 400:
                raise

    def compress(self, src: Path, dst: Path) -> tuple[bool, float | None]:
        """Writes TinyPNG's output for src to dst.

        Returns whether it came from the cache, and how long the API round trip took (for a cached
        result, the time measured when it was fetched, if recorded).
        """
        data = src.read_bytes()
        cached = self.cache_dir / f"{hashlib.sha256(data).hexdigest()}.png"
        timing = cached.with_suffix(".json")
        if cached.exists():
            shutil.copyfile(cached, dst)
            return True, json.loads(timing.read_text())["seconds"] if timing.exists() else None
        if self.fatal:
            raise TinyPNGError(self.fatal)
        with self.slots:
            start = time.perf_counter()
            status, headers, body = self._request("POST", TINYPNG_SHRINK_URL, data)
            url = headers.get("Location") or json.loads(body)["output"]["url"]
            _, _, out = self._request("GET", url)
            seconds = time.perf_counter() - start
        tmp = cached.with_suffix(f".{threading.get_ident()}.tmp")
        tmp.write_bytes(out)
        os.replace(tmp, cached)
        timing.write_text(json.dumps({"seconds": round(seconds, 3)}))
        shutil.copyfile(cached, dst)
        return False, seconds

    def _request(self, method: str, url: str, data: bytes | None = None):
        req = urllib.request.Request(url, data=data, method=method, headers={"Authorization": self.auth})
        if data is not None:
            req.add_header("Content-Type", "application/octet-stream")
        for attempt in range(4):
            try:
                with urllib.request.urlopen(req, timeout=120) as resp:
                    count = resp.headers.get("Compression-Count")
                    if count and count.isdigit():
                        self.compression_count = int(count)
                    return resp.status, resp.headers, resp.read()
            except urllib.error.HTTPError as e:
                body = e.read()
                if e.code >= 500 and attempt < 3:
                    time.sleep(2**attempt)
                    continue
                try:
                    message = json.loads(body).get("message", "")
                except ValueError:
                    message = body[:200].decode(errors="replace")
                error = f"TinyPNG HTTP {e.code}: {message}"
                if e.code in (401, 429):
                    self.fatal = error
                raise TinyPNGError(error, e.code) from None
            except (urllib.error.URLError, TimeoutError) as e:
                if attempt < 3:
                    time.sleep(2**attempt)
                    continue
                raise TinyPNGError(f"TinyPNG network error: {e}") from None
        raise AssertionError("unreachable")


# ---------------------------------------------------------------- tools


PNGDEER_BUILT = BENCH_DIR.parent / "target/release/pngdeer"
QUANTLAB_BUILT = BENCH_DIR.parent / "target/release/quantlab"
# Other open-source palette quantizers, for side-by-side curves: pipelines like im256 (dithered) and
# im256-d0. ImageMagick, FFmpeg and quantette support only on/off transparency in a palette PNG.
QUANT_TOOLS = {
    "im": "ImageMagick（-colors，PNG8）",
    "ff": "FFmpeg（palettegen / paletteuse）",
    "pil": "Pillow（median cut；有透明时 fast octree）",
    "exq": "exoquant（k-means）",
    "qte": "quantette（Wu + k-means，Oklab）",
    "nq": "NeuQuant（color_quant）",
    "qzr": "quantizr（imgproxy）",
}
QUANTLAB_ALGOS = {"exq": "exoquant", "qte": "quantette", "nq": "neuquant", "qzr": "quantizr"}


def find_pngdeer() -> str | None:
    return os.environ.get("PNGDEER") or (str(PNGDEER_BUILT) if PNGDEER_BUILT.exists() else shutil.which("pngdeer"))


def find_tools(need_pngdeer: bool, quant_tools: set[str] = frozenset()) -> dict[str, str]:
    tools = {}
    missing = []
    needed = ["pngquant", "oxipng", "ssimulacra2"] + ["magick"] * ("im" in quant_tools) + ["ffmpeg"] * ("ff" in quant_tools)
    for name in needed:
        path = shutil.which(name)
        if path:
            tools[name] = path
        else:
            missing.append(name)
    if missing:
        sys.exit(f"missing tools: {', '.join(missing)}\ninstall with: brew install pngquant oxipng jpeg-xl imagemagick ffmpeg")
    if quant_tools & set(QUANTLAB_ALGOS):
        path = os.environ.get("QUANTLAB") or (str(QUANTLAB_BUILT) if QUANTLAB_BUILT.exists() else None)
        if not path:
            sys.exit("exq / qte / nq / qzr pipelines need quantlab; build it with: cargo build --release -p quantlab")
        tools["quantlab"] = path
    if path := find_pngdeer():
        tools["pngdeer"] = path
    elif need_pngdeer:
        sys.exit("pd pipelines need pngdeer; build it with: cargo build --release")
    return tools


def tool_version(cmd: list[str]) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True).stdout.strip().splitlines()[0]
    except (OSError, IndexError):
        return "?"


def run(cmd: list[str]) -> None:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip().splitlines()
        raise RuntimeError(f"{Path(cmd[0]).name} exited {proc.returncode}: {detail[-1] if detail else ''}")


def run_oxipng(cfg: Config, src: Path, dst: Path, level: str | None = None) -> None:
    cmd = [cfg.oxipng, "-q", "-o", level or cfg.oxipng_level, "--strip", "safe"]
    if cfg.zopfli:
        cmd.append("--zopfli")
    run(cmd + ["--out", str(dst), str(src)])
    if not dst.exists():  # nothing to improve: oxipng may skip writing
        shutil.copyfile(src, dst)


def run_pngquant(cfg: Config, src: Path, dst: Path, quality: int, dither: int | None) -> None:
    cmd = [cfg.pngquant, "--force", "--strip", "--quality", f"0-{quality}"]
    if dither == 0:
        cmd.append("--nofs")
    elif dither is not None:
        cmd.append(f"--floyd={dither / 100:g}")
    if cfg.pq_speed is not None:
        cmd += ["--speed", str(cfg.pq_speed)]
    run(cmd + ["--output", str(dst), "--", str(src)])


def run_quantizer(cfg: Config, tool: str, colors: int, dither: bool, src: Path, dst: Path) -> None:
    """One palette PNG from another open-source quantizer (QUANT_TOOLS), with its own dithering or none."""
    if tool == "im":
        run(["magick", str(src), "-dither", "FloydSteinberg" if dither else "None", "-colors", str(colors), f"PNG8:{dst}"])
    elif tool == "ff":
        use = "floyd_steinberg" if dither else "none"
        graph = f"[0:v]split[a][b];[a]palettegen=max_colors={colors}:reserve_transparent=1:stats_mode=full[p];[b][p]paletteuse=dither={use}:alpha_threshold=128"
        run(["ffmpeg", "-v", "error", "-y", "-i", str(src), "-filter_complex", graph, "-frames:v", "1", str(dst)])
    elif tool == "pil":
        code = ("import sys; from PIL import Image; im = Image.open(sys.argv[1]); "
                "im = im if im.mode in ('RGB', 'RGBA') else im.convert('RGBA'); "
                "method = Image.Quantize.FASTOCTREE if im.mode == 'RGBA' else Image.Quantize.MEDIANCUT; "
                "dither = Image.Dither.FLOYDSTEINBERG if sys.argv[4] == '1' else Image.Dither.NONE; "
                "im.quantize(int(sys.argv[3]), method=method, dither=dither).save(sys.argv[2])")
        run([sys.executable, "-c", code, str(src), str(dst), str(colors), "1" if dither else "0"])
    else:
        run([cfg.quantlab, QUANTLAB_ALGOS[tool], str(colors), "1" if dither else "0", str(src), str(dst)])


def png_chunks(data: bytes):
    """Yields (type, raw chunk bytes) for each chunk after the PNG signature."""
    pos = 8
    while pos < len(data):
        end = pos + 12 + struct.unpack(">I", data[pos : pos + 4])[0]
        yield data[pos + 4 : pos + 8], data[pos:end]
        pos = end


def without_color_chunks(data: bytes) -> bytes:
    return data[:8] + b"".join(chunk for kind, chunk in png_chunks(data) if kind not in COLOR_CHUNKS)


def strip_color_chunks(src: Path, dst: Path) -> None:
    """Copies a PNG without color-space chunks, so both sides of a comparison decode as plain sRGB."""
    dst.write_bytes(without_color_chunks(src.read_bytes()))


def ssimulacra2(cfg: Config, orig: Path, distorted: Path) -> float:
    proc = subprocess.run([cfg.ssimulacra2, str(orig), str(distorted)], capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"ssimulacra2 exited {proc.returncode}: {proc.stderr.strip()}")
    return float(proc.stdout.split()[-1])


class Scorer:
    """Scores outputs of one image against its untagged source with SSIMULACRA2.

    Images whose longer side is under cfg.small_side are enlarged on both sides first (nearest
    neighbour, by a power of two, at most 8x): SSIMULACRA2 compares the image at six scales, which
    leaves almost nothing to compare on a 72-pixel icon and lets damaged edges through.
    """

    def __init__(self, cfg: Config, source: Path, workdir: Path, width: int, height: int):
        self.cfg, self.workdir, self.reference, self.factor = cfg, workdir, source, 1
        if cfg.small_side and cfg.pngdeer:
            while max(width, height) * self.factor < cfg.small_side and self.factor < 8:
                self.factor *= 2
        if self.factor > 1:
            self.reference = workdir / "score-reference.png"
            run([cfg.pngdeer, str(source), "-o", str(self.reference), "--upscale", str(self.factor)])

    def score(self, path: Path) -> float:
        scored = self.workdir / "score-candidate.png"
        strip_color_chunks(path, scored)
        if self.factor > 1:
            enlarged = self.workdir / "score-candidate-big.png"
            run([self.cfg.pngdeer, str(scored), "-o", str(enlarged), "--upscale", str(self.factor)])
            scored = enlarged
        return ssimulacra2(self.cfg, self.reference, scored)


def run_pngdeer(cfg: Config, src: Path, dst: Path, target: int | None) -> tuple[str, float]:
    """pngdeer's whole pipeline (search, oxipng -o 2, lossless fallbacks), as the CLI and the browser
    run it. Returns what it picked and the time it took."""
    cmd = [cfg.pngdeer, str(src), "-o", str(dst), "--target", "auto" if target is None else str(target)]
    if cfg.pd_variants:
        cmd += ["--variants", cfg.pd_variants]
    if cfg.pd_preview_pixels is not None:
        cmd += ["--preview-pixels", str(cfg.pd_preview_pixels)]
    start = time.perf_counter()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    seconds = time.perf_counter() - start
    if proc.returncode != 0:
        raise RuntimeError(f"pngdeer exited {proc.returncode}: {proc.stderr.strip()}")
    report = json.loads(proc.stdout)
    outcome = "" if report["outcome"] == "hit" else f" {report['outcome']}"
    return f"n{report['colors']} {report['variant']} t{report['target']:g} evals={report['evals']}{outcome}", seconds


# ---------------------------------------------------------------- inputs


def parse_pipelines(spec: str) -> list[Pipeline]:
    pipelines = []
    for name in (s.strip() for s in spec.split(",") if s.strip()):
        if name == "oxipng":
            pipelines.append(Pipeline(name, "oxipng"))
        elif name == "tinypng":
            pipelines.append(Pipeline(name, "tinypng"))
        elif m := re.fullmatch(r"pq(\d{1,3})(?:-d(\d{1,3}))?", name):
            quality = int(m.group(1))
            dither = int(m.group(2)) if m.group(2) else None
            if not 1 <= quality <= 100 or (dither is not None and dither > 100):
                sys.exit(f"pngquant quality must be 1-100 and dithering 0-100: {name}")
            pipelines.append(Pipeline(name, "pngquant", quality, dither))
        elif m := re.fullmatch(r"(" + "|".join(QUANT_TOOLS) + r")(\d{1,3})(-d0)?", name):
            colors = int(m.group(2))
            if not 2 <= colors <= 256:
                sys.exit(f"palette size must be 2-256: {name}")
            pipelines.append(Pipeline(name, "quant", colors, 0 if m.group(3) else None, tool=m.group(1)))
        elif name == "pdauto":
            pipelines.append(Pipeline(name, "pngdeer"))
        elif m := re.fullmatch(r"pd(\d{1,3})", name):
            target = int(m.group(1))
            if not 1 <= target <= 100:
                sys.exit(f"pngdeer target must be an SSIMULACRA2 score 1-100: {name}")
            pipelines.append(Pipeline(name, "pngdeer", target=target))
        else:
            sys.exit(f"unknown pipeline: {name} (expected oxipng, tinypng, pdT, pdauto, pqN, pqN-dD, or <tool>N / <tool>N-d0 with tool {'/'.join(QUANT_TOOLS)})")
    return pipelines


def load_api_key() -> str | None:
    if key := os.environ.get("TINYPNG_API_KEY"):
        return key.strip()
    for env_file in (BENCH_DIR.parent / ".env", BENCH_DIR / ".env"):
        if env_file.exists():
            for line in env_file.read_text().splitlines():
                name, _, value = line.partition("=")
                if name.strip() == "TINYPNG_API_KEY" and value.strip():
                    return value.strip().strip("'\"")
    return None


def find_images(root: Path) -> list[Image]:
    images = []
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root)
        if path.is_file() and path.suffix.lower() == ".png" and not any(p.startswith(".") for p in rel.parts):
            images.append(Image(path, rel, rel.parts[0] if len(rel.parts) > 1 else "(root)"))
    return images


def png_info(path: Path) -> tuple[int, int, str]:
    with open(path, "rb") as f:
        head = f.read(26)
    if head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        raise ValueError("not a PNG file")
    width, height, depth, color_type = struct.unpack(">IIBB", head[16:26])
    return width, height, f"{COLOR_TYPES.get(color_type, color_type)}/{depth}"


# ---------------------------------------------------------------- benchmark


def process_image(img: Image, pipelines: list[Pipeline], cfg: Config, tiny: TinyPNG | None, out_dir: Path) -> list[Row]:
    width, height, color = png_info(img.path)
    orig_bytes = img.path.stat().st_size
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        # Every tool gets the same untagged pixels, which are also the scoring reference.
        source = Path(tmp) / "source.png"
        strip_color_chunks(img.path, source)
        scorer = Scorer(cfg, source, Path(tmp), width, height)
        for p in pipelines:
            row = Row(str(img.rel), img.category, width, height, color, p.name, orig_bytes)
            dst = out_dir / p.name / img.rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            start = time.perf_counter()
            seconds = None
            try:
                if p.kind == "oxipng":
                    run_oxipng(cfg, source, dst)
                elif p.kind == "pngquant":
                    quantized = Path(tmp) / "quantized.png"
                    run_pngquant(cfg, source, quantized, p.quality, p.dither)
                    row.pngquant_bytes = quantized.stat().st_size
                    run_oxipng(cfg, quantized, dst)
                elif p.kind == "quant":
                    quantized = Path(tmp) / "quantized.png"
                    run_quantizer(cfg, p.tool, p.quality, p.dither != 0, source, quantized)
                    row.pngquant_bytes = quantized.stat().st_size
                    run_oxipng(cfg, quantized, dst)
                elif p.kind == "pngdeer":
                    row.params, seconds = run_pngdeer(cfg, source, dst, p.target)
                elif p.kind == "tinypng":
                    cached, seconds = tiny.compress(source, dst)
                    if cached:
                        row.note = "cached"
                if p.kind in ("oxipng", "pngquant", "quant"):
                    seconds = time.perf_counter() - start
                if seconds is not None:
                    row.seconds = round(seconds, 3)
                row.out_bytes = dst.stat().st_size
                row.ratio = round(row.out_bytes / orig_bytes, 4)
                row.ssimulacra2 = round(scorer.score(dst), 2)
            except (RuntimeError, TinyPNGError, OSError, ValueError) as e:
                row.note = f"error: {e}"
            rows.append(row)
    return rows


# ---------------------------------------------------------------- report


def percentile(sorted_values: list[float], pct: float) -> float:
    return sorted_values[max(0, math.ceil(pct / 100 * len(sorted_values)) - 1)]


def pct(x: float) -> str:
    return f"{x * 100:.1f}%"


def md_table(headers: list[str], rows: list[list]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    lines += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(lines)


def ok_rows(rows: list[Row]) -> list[Row]:
    return [r for r in rows if r.ssimulacra2 is not None]


def pipeline_table(rows: list[Row], pipelines: list[Pipeline]) -> str:
    table = []
    for p in pipelines:
        mine = [r for r in rows if r.pipeline == p.name]
        ok = ok_rows(mine)
        if not ok:
            table.append([p.name, 0, "-", "-", "-", "-", "-", "-", len(mine)])
            continue
        scores = sorted(r.ssimulacra2 for r in ok)
        total = sum(r.out_bytes for r in ok) / sum(r.orig_bytes for r in ok)
        seconds = [r.seconds for r in ok if r.seconds is not None]
        table.append([
            p.name,
            len(ok),
            pct(total),
            pct(statistics.median(r.ratio for r in ok)),
            f"{statistics.median(scores):.1f}",
            f"{percentile(scores, 10):.1f}",
            f"{scores[0]:.1f}",
            f"{statistics.median(seconds):.2f}" if seconds else "-",
            len(mine) - len(ok),
        ])
    headers = ["方案", "图片数", "总体积比", "体积比中位数", "SSIMU2 中位数", "SSIMU2 P10", "SSIMU2 最低", "单张耗时中位数（秒）", "失败"]
    return md_table(headers, table)


@dataclass
class VsTiny:
    tiny: Row
    bytes_at_tiny_score: float | None  # open-source bytes needed to match TinyPNG's score; None = out of reach
    score_at_tiny_bytes: float | None  # open-source score with TinyPNG's byte count; None = can't get that small


def frontier(rows: list[Row]) -> list[tuple[float, int]]:
    """(score, bytes) points that no other point beats on both axes, ascending in both."""
    front = []
    for r in sorted(rows, key=lambda r: (-r.ssimulacra2, r.out_bytes)):
        if not front or r.out_bytes < front[-1][1]:
            front.append((r.ssimulacra2, r.out_bytes))
    return front[::-1]


def bytes_at_score(front: list[tuple[float, int]], score: float) -> float | None:
    if score > front[-1][0]:
        return None
    if score <= front[0][0]:
        return front[0][1]
    for (s0, b0), (s1, b1) in zip(front, front[1:]):
        if s0 < score <= s1:  # interpolate log(bytes) linearly in score
            return math.exp(math.log(b0) + (score - s0) / (s1 - s0) * (math.log(b1) - math.log(b0)))
    raise AssertionError("unreachable")


def score_at_bytes(front: list[tuple[float, int]], size: int) -> float | None:
    if size < front[0][1]:
        return None
    if size >= front[-1][1]:
        return front[-1][0]
    for (s0, b0), (s1, b1) in zip(front, front[1:]):
        if b0 <= size < b1:
            return s0 + (math.log(size) - math.log(b0)) / (math.log(b1) - math.log(b0)) * (s1 - s0)
    raise AssertionError("unreachable")


def compare_with_tinypng(rows: list[Row], lossy: set[str]) -> list[VsTiny]:
    """Places TinyPNG on each image's open-source size/quality curve built from the `lossy` pipelines.

    Interpolates between pngquant levels rather than jumping to the next level that clears TinyPNG's
    score, which would turn a 0.1-point miss into a lossless-sized file. The lossless oxipng output
    only counts when it beats the curve outright (pngquant can inflate smooth gradients).
    """
    by_image: dict[str, dict[str, Row]] = defaultdict(dict)
    for r in ok_rows(rows):
        by_image[r.image][r.pipeline] = r
    result = []
    for outputs in by_image.values():
        tiny = outputs.get("tinypng")
        points = [r for name, r in outputs.items() if name in lossy]
        if tiny is None or not points:
            continue
        front = frontier(points)
        size = bytes_at_score(front, tiny.ssimulacra2)
        score = score_at_bytes(front, tiny.out_bytes)
        if (lossless := outputs.get("oxipng")) and lossless.ssimulacra2 >= tiny.ssimulacra2:
            if size is not None and lossless.out_bytes < size:
                size = lossless.out_bytes
            if lossless.out_bytes <= tiny.out_bytes:
                score = max(score or 0, lossless.ssimulacra2)
        result.append(VsTiny(tiny, size, score))
    return result


def vs_tiny_row(label: str, comps: list[VsTiny]) -> list:
    reached = [c for c in comps if c.bytes_at_tiny_score is not None]
    sized = [c for c in comps if c.score_at_tiny_bytes is not None]
    ratios = [c.bytes_at_tiny_score / c.tiny.out_bytes for c in reached]
    deltas = sorted(c.score_at_tiny_bytes - c.tiny.ssimulacra2 for c in sized)
    return [
        label,
        len(comps),
        pct(statistics.median(ratios)) if ratios else "-",
        pct(sum(c.bytes_at_tiny_score for c in reached) / sum(c.tiny.out_bytes for c in reached)) if reached else "-",
        len(comps) - len(reached),
        f"{statistics.median(deltas):+.1f}" if deltas else "-",
        f"{percentile(deltas, 10):+.1f}" if deltas else "-",
        len(comps) - len(sized),
    ]


VS_TINY_HEADERS = ["范围", "图片数", "同画质体积比（中位数）", "同画质体积比（总）", "够不着", "同体积分差（中位数）", "同体积分差 P10", "不可比"]


def category_rows(comps: list[VsTiny], prefix: str = "") -> list[list]:
    categories = sorted({c.tiny.category for c in comps})
    if len(categories) < 2:
        return []
    return [vs_tiny_row(prefix + cat, [c for c in comps if c.tiny.category == cat]) for cat in categories]


LOSSY_KINDS = ("pngquant", "quant", "pngdeer")


def on_curve(p: Pipeline, kinds: tuple[str, ...] = LOSSY_KINDS) -> bool:
    """Pipelines that make a size/quality curve per image: every lossy one but pdauto (one point)."""
    return p.kind in kinds and not (p.kind == "pngdeer" and p.target is None)


def lossy_names(pipelines: list[Pipeline], kinds: tuple[str, ...] = LOSSY_KINDS) -> set[str]:
    return {p.name for p in pipelines if on_curve(p, kinds)}


def vs_tiny_table(rows: list[Row], pipelines: list[Pipeline]) -> str:
    """One row per pipeline family, then all pq levels pooled (best level picked after the fact), by category."""
    lossy = [p for p in pipelines if on_curve(p)]
    table = []
    for family in sorted({p.family for p in lossy}):
        table.append(vs_tiny_row(f"{family}（全部图片）", compare_with_tinypng(rows, {p.name for p in lossy if p.family == family})))
    pq = lossy_names(pipelines, ("pngquant",))
    if len({p.family for p in lossy if p.kind == "pngquant"}) > 1:
        table.append(vs_tiny_row("所有 pq 档位（全部图片）", compare_with_tinypng(rows, pq)))
    for tool in QUANT_TOOLS:
        if len({p.family for p in lossy if p.tool == tool}) > 1:
            names = {p.name for p in lossy if p.tool == tool}
            table.append(vs_tiny_row(f"所有 {tool} 档位（全部图片）", compare_with_tinypng(rows, names)))
    table += category_rows(compare_with_tinypng(rows, pq or lossy_names(pipelines)))
    return md_table(VS_TINY_HEADERS, table)


def auto_table(rows: list[Row], pipelines: list[Pipeline]) -> str:
    by_image: dict[str, dict[str, Row]] = defaultdict(dict)
    for r in ok_rows(rows):
        by_image[r.image][r.pipeline] = r

    def timing(rs: list[Row]) -> list[str]:
        seconds = [r.seconds for r in rs if r.seconds is not None]
        if not seconds:
            return ["-", "-"]
        return [f"{statistics.median(seconds):.2f}", f"{statistics.mean(seconds):.2f}" + ("" if len(seconds) == len(rs) else f"（{len(seconds)} 张有计时）")]

    table = []
    for p in pipelines:
        if p.kind != "pngdeer":
            continue
        mine = [o[p.name] for o in by_image.values() if p.name in o]
        if not mine:
            continue
        pairs = [(o[p.name], o["tinypng"]) for o in by_image.values() if p.name in o and "tinypng" in o]
        evals = [int(m.group(1)) for r in mine if (m := re.search(r"evals=(\d+)", r.params))]
        table.append([
            p.name,
            f"{sum(r.ssimulacra2 >= p.target for r in mine)}/{len(mine)}" if p.target is not None else "-",
            f"{statistics.median(r.ssimulacra2 for r in mine):.1f}",
            pct(sum(a.out_bytes for a, _ in pairs) / sum(t.out_bytes for _, t in pairs)) if pairs else "-",
            pct(statistics.median(a.out_bytes / t.out_bytes for a, t in pairs)) if pairs else "-",
            f"{statistics.median(a.ssimulacra2 - t.ssimulacra2 for a, t in pairs):+.1f}" if pairs else "-",
            f"{statistics.median(evals):g}" if evals else "-",
            *timing(mine),
        ])
    tiny = [o["tinypng"] for o in by_image.values() if "tinypng" in o]
    if tiny:
        table.append(["tinypng", "-", f"{statistics.median(r.ssimulacra2 for r in tiny):.1f}", "100%", "100%", "+0.0", "-", *timing(tiny)])
    headers = ["方案", "达标", "分数中位数", "体积 / TinyPNG（总）", "体积 / TinyPNG（中位数）", "分差 vs TinyPNG（中位数）", "评估次数中位数", "单张耗时中位数（秒）", "单张耗时平均（秒）"]
    return md_table(headers, table)


def timing_by_category_table(rows: list[Row], pipelines: list[Pipeline]) -> str:
    names = [p.name for p in pipelines if p.kind in ("pngdeer", "tinypng")]
    table = []
    for cat in sorted({r.category for r in rows}):
        line = [cat]
        for name in names:
            seconds = [r.seconds for r in ok_rows(rows) if r.category == cat and r.pipeline == name and r.seconds is not None]
            line.append(f"{statistics.median(seconds):.2f}" if seconds else "-")
        table.append(line)
    return md_table(["分类"] + [f"{n} 中位数（秒）" for n in names], table)


def vs_tiny_per_image(comps: list[VsTiny], limit: int = 30) -> str:
    # Worst for open source first: out of reach, then the largest size ratio.
    def key(c: VsTiny) -> float:
        return math.inf if c.bytes_at_tiny_score is None else c.bytes_at_tiny_score / c.tiny.out_bytes

    ordered: list[VsTiny | None] = sorted(comps, key=key, reverse=True)
    if len(ordered) > limit:
        ordered = ordered[: limit // 2] + [None] + ordered[-limit // 2 :]
    table = []
    for c in ordered:
        if c is None:
            table.append(["…"] + [""] * 5)
            continue
        reached = c.bytes_at_tiny_score is not None
        table.append([
            c.tiny.image,
            f"{c.tiny.out_bytes:,}",
            f"{c.tiny.ssimulacra2:.1f}",
            f"{c.bytes_at_tiny_score:,.0f}" if reached else "够不着",
            pct(c.bytes_at_tiny_score / c.tiny.out_bytes) if reached else "-",
            f"{c.score_at_tiny_bytes - c.tiny.ssimulacra2:+.1f}" if c.score_at_tiny_bytes is not None else "-",
        ])
    return md_table(["图片", "TinyPNG 字节", "TinyPNG 分数", "开源同画质字节", "同画质体积比", "同体积分差"], table)


def head_to_head_table(rows: list[Row], pipelines: list[Pipeline]) -> str:
    by_image: dict[str, dict[str, Row]] = defaultdict(dict)
    for r in ok_rows(rows):
        by_image[r.image][r.pipeline] = r
    table = []
    for p in pipelines:
        if p.kind == "tinypng":
            continue
        better = worse = mixed = 0
        for outputs in by_image.values():
            tiny, mine = outputs.get("tinypng"), outputs.get(p.name)
            if tiny is None or mine is None:
                continue
            smaller = mine.out_bytes <= tiny.out_bytes
            sharper = mine.ssimulacra2 >= tiny.ssimulacra2
            if smaller and sharper:
                better += 1
            elif not smaller and not sharper:
                worse += 1
            else:
                mixed += 1
        table.append([p.name, better, worse, mixed])
    return md_table(["方案", "全面占优（更小且画质不低）", "全面落后（更大且画质更低）", "各有取舍"], table)


def build_report(rows: list[Row], pipelines: list[Pipeline], images: list[Image], cfg: Config, meta: dict) -> str:
    parts = [
        "# PNG 压缩基准报告",
        "",
        f"- 时间：{meta['time']}",
        f"- 图片：{len(images)} 张（{meta['images_dir']}）",
        f"- 工具：{meta['pngquant']}；{meta['oxipng']}（-o {cfg.oxipng_level}{'，zopfli' if cfg.zopfli else ''}）；ssimulacra2（libjxl）",
        f"- pngquant speed：{cfg.pq_speed if cfg.pq_speed is not None else '默认 (4)'}",
        f"- 并行：{meta['jobs']} 张图同时处理（本地耗时会因争用偏高）",
    ]
    if used := [t for t in QUANT_TOOLS if any(p.tool == t for p in pipelines)]:
        parts.append("- 其他开源量化器（`<名字><颜色数>` 带各自的 Floyd–Steinberg 抖动，`-d0` 不抖动，最后都过同样的 oxipng）："
                     + "；".join(f"`{t}` {QUANT_TOOLS[t]}" for t in used) + "。ImageMagick、FFmpeg、quantette 的调色板 PNG 只支持全透明 / 不透明")
    if cfg.small_side:
        parts.append(f"- 小图打分：长边小于 {cfg.small_side} 的图，原图和输出都按最近邻放大到不小于 {cfg.small_side}（2 的幂，最多 8 倍）再打分")
    if meta.get("tinypng_count") is not None:
        parts.append(f"- TinyPNG 本月已用压缩次数：{meta['tinypng_count']}")
    parts += [
        "",
        "SSIMULACRA2 分数：100 = 完全一致，90 = 1:1 下看不出区别，70 = 高质量，50 = 中等。"
        "原图先去掉色彩空间块（gAMA/cHRM/sRGB/iCCP）再交给各工具，所有工具拿到同样的像素，打分也只比较像素。"
        "体积比 = 输出 / 原图，越小越好。P10 是最差 10% 的分界线，看的是翻车情况。",
        "",
        "## 各方案总览",
        "",
        pipeline_table(rows, pipelines),
    ]
    categories = sorted({img.category for img in images})
    if len(categories) > 1:
        parts += ["", "## 分类明细"]
        for cat in categories:
            parts += ["", f"### {cat}", "", pipeline_table([r for r in rows if r.category == cat], pipelines)]
    comps = compare_with_tinypng(rows, lossy_names(pipelines, ("pngquant",)) or lossy_names(pipelines))
    if comps:
        parts += [
            "",
            "## 开源 vs TinyPNG",
            "",
            "每张图上，同一族方案（如 pq80、pq70… 或 pd80、pd85…）连成一条“体积–画质”曲线，"
            "把 TinyPNG 的结果放到这条曲线上比较（档位之间按插值算）：",
            "",
            "- **同画质体积比**：开源要达到 TinyPNG 同样的分数，需要的字节数 ÷ TinyPNG 字节数。低于 100% 说明开源更小。",
            "- **够不着**：TinyPNG 的分数比开源最高档还高，开源用调色板方案达不到。",
            "- **同体积分差**：开源用和 TinyPNG 一样多的字节，分数高多少（负数说明开源更差）。",
            "- **不可比**：TinyPNG 的文件比开源最小档还小。",
            "",
            "pq 各族是事后从固定档位里挑，相当于“按图自动选参数”的上限；pd 族是 pngdeer 在压缩时真的搜出来的。"
            "分类各行用的是所有 pq 档位。",
            "",
            vs_tiny_table(rows, pipelines),
            "",
            "### 逐图（开源输得最多的在前）",
            "",
            vs_tiny_per_image(comps),
            "",
            "## 固定参数逐图对比 TinyPNG",
            "",
            head_to_head_table(rows, pipelines),
        ]
    searched = [p for p in pipelines if p.kind == "pngdeer"]
    if searched:
        parts += [
            "",
            "## pngdeer（pdT / pdauto）",
            "",
            "`pngdeer IN.png -o OUT.png --target T`（`pdauto` 是 `--target auto`，也就是默认），和命令行、浏览器版完全相同的流程："
            "每个颜色数（2–256）上试几种重新映射的变体，找到刚好达到目标 SSIMULACRA2 的最小文件；超过 35 万像素的图先在缩小图上搜索，"
            "再用原图确认；最后过 oxipng -o 2，原图不超过 256 种颜色、无损版本只大一点时用无损，比原图还大时退回原图。"
            "分数用 fast-ssim2 在进程内算，和这里打分用的 libjxl ssimulacra2 相差不到 0.25 分。",
            "",
            "耗时是每个目标单独运行一次 pngdeer（会用到多个 CPU 核）。"
            "TinyPNG 的耗时是 API 往返（上传、服务端处理、下载），只统计真正请求过的图。",
            "",
            auto_table(rows, pipelines),
        ]
        curve_rows = []
        for family in sorted({p.family for p in searched if on_curve(p)}):
            comps = compare_with_tinypng(rows, {p.name for p in searched if on_curve(p) and p.family == family})
            if comps:
                curve_rows += [vs_tiny_row(f"{family} · 全部", comps)] + category_rows(comps, f"{family} · ")
        if curve_rows:
            parts += ["", "### 目标分数曲线 vs TinyPNG（按分类）", "", md_table(VS_TINY_HEADERS, curve_rows)]
        parts += ["", "### 耗时（按分类）", "", timing_by_category_table(rows, pipelines)]
    errors = [r for r in rows if r.note.startswith("error")]
    if errors:
        parts += ["", "## 失败", "", md_table(["图片", "方案", "原因"], [[r.image, r.pipeline, r.note[7:]] for r in errors[:50]])]
    return "\n".join(parts) + "\n"


# ---------------------------------------------------------------- main


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--images", type=Path, default=BENCH_DIR / "images", help="input directory; first-level subfolders are categories")
    parser.add_argument("--out", type=Path, help="output directory (default: bench/results/<timestamp>)")
    parser.add_argument("--pipelines", default=DEFAULT_PIPELINES, help=f"comma-separated (default: {DEFAULT_PIPELINES})")
    parser.add_argument("--oxipng-level", default="max", help="oxipng -o level for the oxipng, pq and tool pipelines (default: max)")
    parser.add_argument("--zopfli", action="store_true", help="use zopfli in oxipng (smaller, much slower)")
    parser.add_argument("--pq-speed", type=int, choices=range(1, 12), metavar="1-11", help="pngquant --speed (default: pngquant's own, 4)")
    parser.add_argument("--pd-variants", help="pngdeer --variants for the pd pipelines: what it tries per palette size (default: pngdeer's own)")
    parser.add_argument("--pd-preview-pixels", type=int,
                        help="pngdeer --preview-pixels for the pd pipelines; 0 searches at full size (default: pngdeer's own, as shipped)")
    parser.add_argument("--small-side", type=int, default=256,
                        help="score images whose longer side is below this enlarged to at least this size; 0 = off (default: 256)")
    parser.add_argument("-j", "--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2), help="images processed in parallel")
    args = parser.parse_args()

    pipelines = parse_pipelines(args.pipelines)
    tools = find_tools(need_pngdeer=any(p.kind == "pngdeer" for p in pipelines), quant_tools={p.tool for p in pipelines if p.kind == "quant"})
    cfg = Config(
        tools["pngquant"], tools["oxipng"], tools["ssimulacra2"], args.oxipng_level, args.zopfli, args.pq_speed,
        tools.get("pngdeer"), args.pd_variants, args.pd_preview_pixels, tools.get("quantlab"),
        args.small_side if tools.get("pngdeer") else 0,
    )
    if args.small_side and not tools.get("pngdeer"):
        print("warning: pngdeer not found, so small images are scored at their own size", file=sys.stderr)

    tiny = None
    if any(p.kind == "tinypng" for p in pipelines):
        key = load_api_key()
        if key:
            tiny = TinyPNG(key, BENCH_DIR / ".cache" / "tinypng")
            try:
                tiny.validate()
            except TinyPNGError as e:
                if e.status != 429:
                    sys.exit(f"TinyPNG key check failed: {e}")
                print(f"warning: {e}; only cached TinyPNG results will be used", file=sys.stderr)
        else:
            print("warning: TINYPNG_API_KEY not set (env or .env), skipping tinypng", file=sys.stderr)
            pipelines = [p for p in pipelines if p.kind != "tinypng"]
    if not pipelines:
        sys.exit("no pipelines to run")

    images_dir = args.images.resolve()
    images = find_images(images_dir)
    if not images:
        sys.exit(f"no PNG files found in {images_dir}")

    started = datetime.now()
    out_dir = (args.out or BENCH_DIR / "results" / started.strftime("%Y%m%d-%H%M%S")).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    print(f"{len(images)} images × {len(pipelines)} pipelines ({', '.join(p.name for p in pipelines)}) → {out_dir}")

    rows_by_image: dict[Path, list[Row]] = {}
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(process_image, img, pipelines, cfg, tiny, out_dir): img for img in images}
        for done, future in enumerate(as_completed(futures), 1):
            img = futures[future]
            try:
                rows_by_image[img.rel] = future.result()
            except (OSError, ValueError) as e:
                print(f"[{done}/{len(images)}] {img.rel}: skipped ({e})", file=sys.stderr)
                continue
            failed = sum(r.note.startswith("error") for r in rows_by_image[img.rel])
            print(f"[{done}/{len(images)}] {img.rel}" + (f"  ({failed} failed)" if failed else ""), flush=True)
    rows = [r for img in images for r in rows_by_image.get(img.rel, [])]

    with open(out_dir / "results.csv", "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=[fld.name for fld in fields(Row)])
        writer.writeheader()
        writer.writerows(asdict(r) for r in rows)

    meta = {
        "time": started.strftime("%Y-%m-%d %H:%M"),
        "images_dir": images_dir,
        "pngquant": "pngquant " + tool_version([cfg.pngquant, "--version"]),
        "oxipng": tool_version([cfg.oxipng, "--version"]),
        "tinypng_count": tiny.compression_count if tiny else None,
        "jobs": args.jobs,
    }
    report = build_report(rows, pipelines, images, cfg, meta)
    (out_dir / "report.md").write_text(report)

    latest = out_dir.parent / "latest"
    if args.out is None:
        if latest.is_symlink():
            latest.unlink()
        if not latest.exists():
            latest.symlink_to(out_dir.name)

    print()
    print(report)
    print(f"results: {out_dir / 'report.md'}, {out_dir / 'results.csv'}")
    if tiny and tiny.fatal:
        print(f"warning: TinyPNG stopped early: {tiny.fatal}", file=sys.stderr)


if __name__ == "__main__":
    main()
