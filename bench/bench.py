#!/usr/bin/env python3
"""PNG compression benchmark: pngdeer, pngquant and other open-source quantizers vs TinyPNG, and WebP.

Every output is scored against its original with SSIMULACRA2
(100 = identical, 90 = indistinguishable at 1:1, 70 = high quality, 50 = medium).

Pipelines (pick with --pipelines):
  oxipng   lossless re-encode only (reference point)
  pdT      pngdeer --target T: the smallest file scoring >= T, exactly as the CLI and browser make it
  pdauto   pngdeer's automatic quality (its default, what the browser does)
  pqN      pngquant --quality 0-N, then oxipng (e.g. pq80)
  pqN-dD   same with dithering level D/100 (pq80-d50 = --floyd=0.5, pq80-d0 = --nofs)
  <tool>N  other open-source quantizers at N colors, then oxipng (see QUANT_TOOLS)
  tinypng  TinyPNG API (results cached in bench/.cache/tinypng to save quota); the key is
           TINYPNG_API_KEY in the environment or .env, with TINYPNG_API_KEY_2 and so on taking over
           when one's monthly compressions run out

WebP pipelines (outputs decoded with dwebp, as browsers decode them, then scored the same way):
  wpT      pngdeer --format webp --target T
  wpauto   pngdeer --format webp at its automatic quality
  cwQ      cwebp -q Q -m 6 -sharp_yuv -alpha_q 100 (libwebp at its slowest and sharpest)
  cwafQ    the same with the auto filter (-af), as pngdeer encodes
  cwdQ     cwebp -q Q at cwebp's other defaults (method 4, plain YUV)
  cwll     cwebp -lossless -z 9
  pqwN     pngquant --quality 0-N, then cwebp -lossless -z 9: a palette image as lossless WebP
  pqwN-d0  same without dithering
  tinywebp TinyPNG API, converted to WebP (cached in bench/.cache/tinypng; costs two compressions)
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
    kind: str  # "oxipng" | "pngdeer" | "pngquant" | "quant" | "tinypng" | "pngdeer-webp" | "cwebp" | "pngquant-webp" | "tinywebp"
    quality: int | None = None  # pngquant(-webp): --quality; quant: palette size; cwebp: -q (None: lossless)
    dither: int | None = None  # pngquant(-webp) and quant; None = the tool's default dithering
    target: int | None = None  # pngdeer: SSIMULACRA2 score to reach (None: automatic)
    tool: str | None = None  # quant: a QUANT_TOOLS key
    cwebp: str | None = None  # cwebp: "af" adds the auto filter, "d" keeps cwebp's defaults (see run_cwebp)

    @property
    def family(self) -> str:
        return re.sub(r"^(pqw|pq|pd|wp|cwaf|cwd|cw|" + "|".join(QUANT_TOOLS) + r")\d+", r"\1", self.name)

    @property
    def webp(self) -> bool:
        return self.kind in ("pngdeer-webp", "cwebp", "pngquant-webp", "tinywebp")


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
    pd_args: tuple[str, ...] = ()  # more pngdeer arguments, e.g. --webp-method 5
    cwebp: str | None = None
    dwebp: str | None = None


class TinyPNGError(Exception):
    def __init__(self, message: str, status: int | None = None):
        super().__init__(message)
        self.status = status


class TinyPNG:
    def __init__(self, api_keys: list[str], cache_dir: Path, concurrency: int = 4):
        # Keys are used in turn: the next takes over once one's monthly compressions run out (429).
        self.auths = ["Basic " + base64.b64encode(f"api:{key}".encode()).decode() for key in api_keys]
        self.key = 0
        self.key_lock = threading.Lock()
        self.cache_dir = cache_dir
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.slots = threading.Semaphore(concurrency)
        # Each key's compressions this month, as its last response said.
        self.compression_counts: list[int | None] = [None] * len(api_keys)
        # Set on 401, or 429 with no key left, so the remaining images fail fast instead of hammering the API.
        self.fatal: str | None = None

    def validate(self) -> None:
        """Checks every key without spending a compression (an empty upload answers 400 for a good key,
        429 once its month is used up) and starts from the first with compressions left."""
        good, spent = [], None
        for i, auth in enumerate(self.auths):
            try:
                self._request("POST", TINYPNG_SHRINK_URL, b"", auth=auth)
            except TinyPNGError as e:
                if e.status == 429:
                    spent = e
                    continue
                if e.status != 400:
                    raise TinyPNGError(f"key {i + 1}: {e}", e.status) from None
            good.append(i)
        if not good:
            self.fatal = str(spent)
            raise spent
        self.key = good[0]

    def _with_key(self, call):
        """call(auth) with the key in use; again with the next key if this one's month runs out."""
        while True:
            key = self.key
            try:
                return call(self.auths[key])
            except TinyPNGError as e:
                if e.status != 429:
                    raise
                with self.key_lock:
                    if self.key == key:  # not moved on by another thread already
                        if key + 1 == len(self.auths):
                            self.fatal = str(e)
                            raise
                        self.key = key + 1
                        print(f"note: TinyPNG key {key + 1} is out of compressions; using key {key + 2}", file=sys.stderr)

    def compress(self, src: Path, dst: Path) -> tuple[bool, float | None]:
        """Writes TinyPNG's output for src (a PNG or a JPEG) to dst.

        Returns whether it came from the cache, and how long the API round trip took (for a cached
        result, the time measured when it was fetched, if recorded).
        """
        data = src.read_bytes()
        # TinyPNG answers a JPEG with a JPEG
        cached = self.cache_dir / f"{hashlib.sha256(data).hexdigest()}.{'jpg' if data[:3] == b'\xff\xd8\xff' else 'png'}"
        timing = cached.with_suffix(".json")
        if cached.exists():
            shutil.copyfile(cached, dst)
            return True, json.loads(timing.read_text())["seconds"] if timing.exists() else None
        if self.fatal:
            raise TinyPNGError(self.fatal)
        def shrink(auth: str) -> bytes:
            _, headers, body = self._request("POST", TINYPNG_SHRINK_URL, data, auth=auth)
            url = headers.get("Location") or json.loads(body)["output"]["url"]
            return self._request("GET", url, auth=auth)[2]

        with self.slots:
            start = time.perf_counter()
            out = self._with_key(shrink)
            seconds = time.perf_counter() - start
        tmp = cached.with_suffix(f".{threading.get_ident()}.tmp")
        tmp.write_bytes(out)
        os.replace(tmp, cached)
        timing.write_text(json.dumps({"seconds": round(seconds, 3)}))
        shutil.copyfile(cached, dst)
        return False, seconds

    def convert_webp(self, src: Path, dst: Path) -> tuple[bool, float | None]:
        """Writes TinyPNG's output for src, converted to WebP, to dst (a shrink, then a conversion of
        its result: two compressions). Returns whether it came from the cache and the round trip's time."""
        return self._convert(src, dst, "webp", lambda out: out[:4] == b"RIFF" and out[8:12] == b"WEBP")

    def convert_avif(self, src: Path, dst: Path) -> tuple[bool, float | None]:
        """As convert_webp, to AVIF."""
        return self._convert(src, dst, "avif", lambda out: out[4:12] in (b"ftypavif", b"ftypavis"))

    def _convert(self, src: Path, dst: Path, fmt: str, is_format) -> tuple[bool, float | None]:
        data = src.read_bytes()
        cached = self.cache_dir / f"{hashlib.sha256(data).hexdigest()}.{fmt}"
        timing = cached.with_suffix(f".{fmt}.json")
        if cached.exists():
            shutil.copyfile(cached, dst)
            return True, json.loads(timing.read_text())["seconds"] if timing.exists() else None
        if self.fatal:
            raise TinyPNGError(self.fatal)
        def convert(auth: str) -> bytes:
            # both steps on one key: the shrink's result belongs to its account
            _, headers, body = self._request("POST", TINYPNG_SHRINK_URL, data, auth=auth)
            url = headers.get("Location") or json.loads(body)["output"]["url"]
            return self._request("POST", url, json.dumps({"convert": {"type": f"image/{fmt}"}}).encode(), "application/json", auth)[2]

        with self.slots:
            start = time.perf_counter()
            out = self._with_key(convert)
            seconds = time.perf_counter() - start
        if not is_format(out):
            raise TinyPNGError(f"TinyPNG's conversion is not {fmt.upper()}")
        tmp = cached.with_suffix(f".{threading.get_ident()}.tmp")
        tmp.write_bytes(out)
        os.replace(tmp, cached)
        timing.write_text(json.dumps({"seconds": round(seconds, 3)}))
        shutil.copyfile(cached, dst)
        return False, seconds

    def _request(self, method: str, url: str, data: bytes | None = None, content_type: str = "application/octet-stream", auth: str = ""):
        auth = auth or self.auths[self.key]
        req = urllib.request.Request(url, data=data, method=method, headers={"Authorization": auth})
        if data is not None:
            req.add_header("Content-Type", content_type)

        def counted(headers) -> None:
            count = headers.get("Compression-Count")
            if count and count.isdigit():
                self.compression_counts[self.auths.index(auth)] = int(count)

        for attempt in range(4):
            try:
                with urllib.request.urlopen(req, timeout=120) as resp:
                    counted(resp.headers)
                    return resp.status, resp.headers, resp.read()
            except urllib.error.HTTPError as e:
                counted(e.headers)
                body = e.read()
                if e.code >= 500 and attempt < 3:
                    time.sleep(2**attempt)
                    continue
                try:
                    message = json.loads(body).get("message", "")
                except ValueError:
                    message = body[:200].decode(errors="replace")
                error = f"TinyPNG HTTP {e.code}: {message}"
                if e.code == 401:
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
    "im": "ImageMagick (-colors, PNG8)",
    "ff": "FFmpeg (palettegen / paletteuse)",
    "pil": "Pillow (median cut; fast octree with transparency)",
    "exq": "exoquant (k-means)",
    "qte": "quantette (Wu + k-means, Oklab)",
    "nq": "NeuQuant (color_quant)",
    "qzr": "quantizr (imgproxy)",
}
QUANTLAB_ALGOS = {"exq": "exoquant", "qte": "quantette", "nq": "neuquant", "qzr": "quantizr"}


def find_pngdeer() -> str | None:
    return os.environ.get("PNGDEER") or (str(PNGDEER_BUILT) if PNGDEER_BUILT.exists() else shutil.which("pngdeer"))


def find_tools(need_pngdeer: bool, quant_tools: set[str] = frozenset(), webp: bool = False) -> dict[str, str]:
    tools = {}
    missing = []
    needed = ["pngquant", "oxipng", "ssimulacra2"] + ["magick"] * ("im" in quant_tools) + ["ffmpeg"] * ("ff" in quant_tools) + ["cwebp", "dwebp"] * webp
    for name in needed:
        path = shutil.which(name)
        if path:
            tools[name] = path
        else:
            missing.append(name)
    if missing:
        sys.exit(f"missing tools: {', '.join(missing)}\ninstall with: brew install pngquant oxipng jpeg-xl imagemagick ffmpeg webp")
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


def run_cwebp(cfg: Config, src: Path, dst: Path, quality: int | None, style: str | None = None) -> None:
    """libwebp's own encoder: lossy at -q quality with its slowest method and sharp YUV, alpha kept
    lossless (style "af": also the auto filter; "d": cwebp's defaults but -q); or lossless at its
    highest effort."""
    if quality is None:
        args = ["-lossless", "-z", "9"]
    elif style == "d":
        args = ["-q", str(quality)]
    else:
        args = ["-q", str(quality), "-m", "6", "-sharp_yuv", "-alpha_q", "100"] + ["-af"] * (style == "af")
    run([cfg.cwebp, "-quiet", *args, str(src), "-o", str(dst)])


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
        if path.suffix == ".webp":
            run([self.cfg.dwebp, "-quiet", str(path), "-o", str(scored)])
        else:
            strip_color_chunks(path, scored)
        if self.factor > 1:
            enlarged = self.workdir / "score-candidate-big.png"
            run([self.cfg.pngdeer, str(scored), "-o", str(enlarged), "--upscale", str(self.factor)])
            scored = enlarged
        return ssimulacra2(self.cfg, self.reference, scored)


def run_pngdeer(cfg: Config, src: Path, dst: Path, target: int | None, webp: bool = False) -> tuple[str, float]:
    """pngdeer's whole pipeline (search, oxipng -o 2, lossless fallbacks), as the CLI and the browser
    run it, or its WebP output. Returns what it picked and the time it took."""
    cmd = [cfg.pngdeer, str(src), "-o", str(dst), "--target", "auto" if target is None else str(target), *cfg.pd_args]
    if webp:
        cmd += ["--format", "webp"]
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
    if webp:
        q = "" if report["quality"] is None else f" q{report['quality']}"
        tries = f"tries={report['tries']} rounds={report['rounds']}" if "tries" in report else f"probes={report['probes']}"
        return f"{report['mode']}{q} t{report['target']:g} {tries} z{report['lossless_level']} png={report['png']['bytes']}", seconds
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
        elif name == "wpauto":
            pipelines.append(Pipeline(name, "pngdeer-webp"))
        elif m := re.fullmatch(r"wp(\d{1,3})", name):
            pipelines.append(Pipeline(name, "pngdeer-webp", target=int(m.group(1))))
        elif name == "cwll":
            pipelines.append(Pipeline(name, "cwebp"))
        elif m := re.fullmatch(r"cw(af|d)?(\d{1,3})", name):
            if not 0 <= int(m.group(2)) <= 100:
                sys.exit(f"cwebp quality must be 0-100: {name}")
            pipelines.append(Pipeline(name, "cwebp", int(m.group(2)), cwebp=m.group(1)))
        elif m := re.fullmatch(r"pqw(\d{1,3})(?:-d(\d{1,3}))?", name):
            quality = int(m.group(1))
            dither = int(m.group(2)) if m.group(2) else None
            if not 1 <= quality <= 100 or (dither is not None and dither > 100):
                sys.exit(f"pngquant quality must be 1-100 and dithering 0-100: {name}")
            pipelines.append(Pipeline(name, "pngquant-webp", quality, dither))
        elif name == "tinywebp":
            pipelines.append(Pipeline(name, "tinywebp"))
        elif m := re.fullmatch(r"pd(\d{1,3})", name):
            target = int(m.group(1))
            if not 1 <= target <= 100:
                sys.exit(f"pngdeer target must be an SSIMULACRA2 score 1-100: {name}")
            pipelines.append(Pipeline(name, "pngdeer", target=target))
        else:
            sys.exit(f"unknown pipeline: {name} (expected oxipng, tinypng, pdT, pdauto, pqN, pqN-dD, <tool>N / <tool>N-d0 with tool {'/'.join(QUANT_TOOLS)}, wpT, wpauto, cwQ, cwafQ, cwdQ, cwll, pqwN, pqwN-dD or tinywebp)")
    return pipelines


def load_api_keys() -> list[str]:
    """TINYPNG_API_KEY, then TINYPNG_API_KEY_2, _3 and so on, each from the environment or else the
    first .env that sets it."""
    names: dict[str, str] = {}
    for env_file in (BENCH_DIR / ".env", BENCH_DIR.parent / ".env"):  # the later wins
        if env_file.exists():
            for line in env_file.read_text().splitlines():
                name, _, value = line.partition("=")
                names[name.strip()] = value.strip().strip("'\"")
    names.update((name, value) for name, value in os.environ.items() if value.strip())
    numbered = sorted((int(m[1] or 1), value.strip()) for name, value in names.items() if (m := re.fullmatch(r"TINYPNG_API_KEY(?:_(\d+))?", name)))
    return list(dict.fromkeys(value for _, value in numbered if value))


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
            dst = out_dir / p.name / (img.rel.with_suffix(".webp") if p.webp else img.rel)
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
                elif p.kind in ("pngdeer", "pngdeer-webp"):
                    row.params, seconds = run_pngdeer(cfg, source, dst, p.target, p.kind == "pngdeer-webp")
                elif p.kind == "cwebp":
                    run_cwebp(cfg, source, dst, p.quality, p.cwebp)
                elif p.kind == "pngquant-webp":
                    quantized = Path(tmp) / "quantized.png"
                    run_pngquant(cfg, source, quantized, p.quality, p.dither)
                    row.pngquant_bytes = quantized.stat().st_size
                    run_cwebp(cfg, quantized, dst, None)
                elif p.kind in ("tinypng", "tinywebp"):
                    cached, seconds = (tiny.compress if p.kind == "tinypng" else tiny.convert_webp)(source, dst)
                    if cached:
                        row.note = "cached"
                if p.kind in ("oxipng", "pngquant", "quant", "cwebp", "pngquant-webp"):
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
    headers = ["Pipeline", "Images", "Total size ratio", "Median size ratio", "SSIMU2 median", "SSIMU2 P10", "SSIMU2 min", "Median seconds per image", "Failed"]
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


VS_TINY_HEADERS = ["Scope", "Images", "Size ratio at equal quality (median)", "Size ratio at equal quality (total)", "Can't reach", "Score difference at equal size (median)", "Score difference at equal size P10", "Not comparable"]


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
        table.append(vs_tiny_row(f"{family} (all images)", compare_with_tinypng(rows, {p.name for p in lossy if p.family == family})))
    pq = lossy_names(pipelines, ("pngquant",))
    if len({p.family for p in lossy if p.kind == "pngquant"}) > 1:
        table.append(vs_tiny_row("All pq settings (all images)", compare_with_tinypng(rows, pq)))
    for tool in QUANT_TOOLS:
        if len({p.family for p in lossy if p.tool == tool}) > 1:
            names = {p.name for p in lossy if p.tool == tool}
            table.append(vs_tiny_row(f"All {tool} settings (all images)", compare_with_tinypng(rows, names)))
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
        return [f"{statistics.median(seconds):.2f}", f"{statistics.mean(seconds):.2f}" + ("" if len(seconds) == len(rs) else f" ({len(seconds)} timed)")]

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
    headers = ["Pipeline", "Hit target", "Median score", "Size / TinyPNG (total)", "Size / TinyPNG (median)", "Score difference vs TinyPNG (median)", "Median evaluations", "Median seconds per image", "Mean seconds per image"]
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
    return md_table(["Category"] + [f"{n} median (s)" for n in names], table)


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
            f"{c.bytes_at_tiny_score:,.0f}" if reached else "can't reach",
            pct(c.bytes_at_tiny_score / c.tiny.out_bytes) if reached else "-",
            f"{c.score_at_tiny_bytes - c.tiny.ssimulacra2:+.1f}" if c.score_at_tiny_bytes is not None else "-",
        ])
    return md_table(["Image", "TinyPNG bytes", "TinyPNG score", "Open-source bytes at equal quality", "Size ratio at equal quality", "Score difference at equal size"], table)


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
    return md_table(["Pipeline", "Better (smaller, quality no lower)", "Worse (larger and lower quality)", "Trade-off"], table)


def build_report(rows: list[Row], pipelines: list[Pipeline], images: list[Image], cfg: Config, meta: dict) -> str:
    parts = [
        "# PNG compression benchmark",
        "",
        f"- Time: {meta['time']}",
        f"- Images: {len(images)} ({meta['images_dir']})",
        f"- Tools: {meta['pngquant']}; {meta['oxipng']} (-o {cfg.oxipng_level}{', zopfli' if cfg.zopfli else ''}); ssimulacra2 (libjxl)",
        f"- pngquant speed: {cfg.pq_speed if cfg.pq_speed is not None else 'default (4)'}",
        f"- Parallel: {meta['jobs']} images at a time (contention inflates local timings)",
    ]
    if used := [t for t in QUANT_TOOLS if any(p.tool == t for p in pipelines)]:
        parts.append("- Other open-source quantizers (`<name><colors>` with each tool's own Floyd–Steinberg dithering, `-d0` without, all followed by the same oxipng): "
                     + "; ".join(f"`{t}` {QUANT_TOOLS[t]}" for t in used) + ". ImageMagick, FFmpeg and quantette support only fully transparent or opaque pixels in a palette PNG")
    if cfg.small_side:
        parts.append(f"- Small images: when the long side is under {cfg.small_side}, the original and the output are both enlarged with nearest neighbour to at least {cfg.small_side} (by a power of 2, at most 8×) before scoring")
    if meta.get("tinypng_count") is not None:
        parts.append(f"- TinyPNG compressions used this month: {meta['tinypng_count']}")
    parts += [
        "",
        "SSIMULACRA2 scores: 100 = identical, 90 = no visible difference at 1:1, 70 = high quality, 50 = medium. "
        "Color space chunks (gAMA/cHRM/sRGB/iCCP) are removed from the originals first, so every tool gets the same pixels and scores compare pixels only. "
        "Size ratio = output / original; smaller is better. P10 is where the worst 10% begins, so it shows the failures.",
        "",
        "## All pipelines",
        "",
        pipeline_table(rows, pipelines),
    ]
    categories = sorted({img.category for img in images})
    if len(categories) > 1:
        parts += ["", "## By category"]
        for cat in categories:
            parts += ["", f"### {cat}", "", pipeline_table([r for r in rows if r.category == cat], pipelines)]
    comps = compare_with_tinypng(rows, lossy_names(pipelines, ("pngquant",)) or lossy_names(pipelines))
    if comps:
        parts += [
            "",
            "## Open source vs TinyPNG",
            "",
            "On each image, the pipelines of one family (such as pq80, pq70… or pd80, pd85…) form a size–quality curve, "
            "and TinyPNG's result is placed on that curve (interpolating between settings):",
            "",
            "- **Size ratio at equal quality**: the bytes open source needs to reach TinyPNG's score ÷ TinyPNG's bytes. Under 100% means open source is smaller.",
            "- **Can't reach**: TinyPNG scores higher than the family's highest setting, so its palette pipelines can't match it.",
            "- **Score difference at equal size**: how much higher open source scores with as many bytes as TinyPNG (negative means lower).",
            "- **Not comparable**: TinyPNG's file is smaller than the family's smallest setting.",
            "",
            "The pq families pick from fixed settings after the fact, an upper bound on choosing settings per image; the pd families are what pngdeer finds while compressing. "
            "The category rows use every pq setting.",
            "",
            vs_tiny_table(rows, pipelines),
            "",
            "### Per image (biggest open-source losses first)",
            "",
            vs_tiny_per_image(comps),
            "",
            "## Fixed settings vs TinyPNG, per image",
            "",
            head_to_head_table(rows, pipelines),
        ]
    searched = [p for p in pipelines if p.kind == "pngdeer"]
    if searched:
        parts += [
            "",
            "## pngdeer (pdT / pdauto)",
            "",
            "`pngdeer IN.png -o OUT.png --target T` (`pdauto` is `--target auto`, the default), the same process as the command line and the browser: "
            "at each color count (2–256) it tries a few remapping variants and finds the smallest file that reaches the target SSIMULACRA2; images over 350,000 pixels are searched on a downscaled copy first, "
            "then confirmed on the original. Finally oxipng -o 2; when the original has at most 256 colors and the lossless version is only a little larger, the lossless one is used, and the original is kept when the result would be larger. "
            "Scores come from fast-ssim2 in-process, within 0.25 of the libjxl ssimulacra2 used for scoring here.",
            "",
            "Times are one pngdeer run per target (using several CPU cores). "
            "TinyPNG's times are API round trips (upload, server processing, download), counted only for images actually requested.",
            "",
            auto_table(rows, pipelines),
        ]
        curve_rows = []
        for family in sorted({p.family for p in searched if on_curve(p)}):
            comps = compare_with_tinypng(rows, {p.name for p in searched if on_curve(p) and p.family == family})
            if comps:
                curve_rows += [vs_tiny_row(f"{family} · all", comps)] + category_rows(comps, f"{family} · ")
        if curve_rows:
            parts += ["", "### Target score curves vs TinyPNG (by category)", "", md_table(VS_TINY_HEADERS, curve_rows)]
        parts += ["", "### Time (by category)", "", timing_by_category_table(rows, pipelines)]
    errors = [r for r in rows if r.note.startswith("error")]
    if errors:
        parts += ["", "## Failures", "", md_table(["Image", "Pipeline", "Reason"], [[r.image, r.pipeline, r.note[7:]] for r in errors[:50]])]
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
    parser.add_argument("--pd-args", default="", help="more pngdeer arguments for the pd and wp pipelines, e.g. '--webp-method 5'")
    parser.add_argument("--pd-preview-pixels", type=int,
                        help="pngdeer --preview-pixels for the pd pipelines; 0 searches at full size (default: pngdeer's own, as shipped)")
    parser.add_argument("--small-side", type=int, default=256,
                        help="score images whose longer side is below this enlarged to at least this size; 0 = off (default: 256)")
    parser.add_argument("-j", "--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2), help="images processed in parallel")
    args = parser.parse_args()

    pipelines = parse_pipelines(args.pipelines)
    tools = find_tools(need_pngdeer=any(p.kind in ("pngdeer", "pngdeer-webp") for p in pipelines), quant_tools={p.tool for p in pipelines if p.kind == "quant"}, webp=any(p.webp for p in pipelines))
    cfg = Config(
        tools["pngquant"], tools["oxipng"], tools["ssimulacra2"], args.oxipng_level, args.zopfli, args.pq_speed,
        tools.get("pngdeer"), args.pd_variants, args.pd_preview_pixels, tools.get("quantlab"),
        args.small_side if tools.get("pngdeer") else 0, tuple(args.pd_args.split()), tools.get("cwebp"), tools.get("dwebp"),
    )
    if args.small_side and not tools.get("pngdeer"):
        print("warning: pngdeer not found, so small images are scored at their own size", file=sys.stderr)

    tiny = None
    if any(p.kind in ("tinypng", "tinywebp") for p in pipelines):
        keys = load_api_keys()
        if keys:
            tiny = TinyPNG(keys, BENCH_DIR / ".cache" / "tinypng")
            try:
                tiny.validate()
            except TinyPNGError as e:
                if e.status != 429:
                    sys.exit(f"TinyPNG key check failed: {e}")
                print(f"warning: {e}; only cached TinyPNG results will be used", file=sys.stderr)
        else:
            print("warning: TINYPNG_API_KEY not set (env or .env), skipping tinypng", file=sys.stderr)
            pipelines = [p for p in pipelines if p.kind not in ("tinypng", "tinywebp")]
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
