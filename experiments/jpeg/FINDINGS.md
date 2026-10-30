# JPEG to smaller JPEG

2026-10-30. pngdeer now takes JPEG photos too: `pngdeer::compress_jpeg` (src/jpeg.rs) decodes the
photo, re-encodes it at the lowest quality whose SSIMULACRA2 score against it reaches the target,
and never returns more bytes than it was given. On 60 photos, at TinyPNG's quality, its files are
10% smaller than TinyPNG's on the median photo and 13% smaller in total; at its automatic quality,
which aims at about TinyPNG's, 18% and 16% smaller, scoring about the same (median +0.2 points).

## Photos and method

- **60 photos from Wikimedia Commons** (`sources.json`: page and license of each): 30 of Commons'
  resized copies (`web/`: 0.6–6.1 MP, quality 87, 4:2:0, 0.2–2.5 MB), the kind of JPEG a website
  serves, and 30 camera originals (`orig/`: 6.1–15.6 MP, quality 91–100, mostly 4:4:4, 1.3–13.8 MB).
- **TinyPNG** through its API, 30 October 2026 (bench.py's client, `bench/.cache/tinypng`).
- **Scores** against the photo, both decoded as pngdeer decodes them (zune-jpeg), with pngdeer's
  `--score`. Browsers decode with libjpeg-turbo: on TinyPNG's 60 files its pixels score within 0.2
  points of zune-jpeg's (mean 0.05).
- `compare.py` runs everything below and summarizes it; frontiers and interpolation are bench.py's,
  as in the PNG and WebP leaderboards.

## Round 1: the encoder

Every candidate on the 30 web photos and 12 originals (42), each as a sweep of qualities, compared at
each photo's TinyPNG score: bytes needed there over mozjpeg with its table 2 at 4:2:0 (median, and
the middle half of the photos):

| Encoder and settings | Bytes |
|---|---|
| **mozjpeg 4.1.5, quantization table 2, 4:2:0** | **1.000** |
| jpegli, 4:2:0 | 0.997 (0.96–1.03) |
| mozjpeg's defaults (table 3) | 1.051 (1.03–1.08) |
| jpegli's defaults (4:4:4) | 1.068 (1.03–1.17) |
| mozjpeg's defaults at 4:4:4 | 1.137 (1.10–1.18) |
| mozjpeg, table 2, 4:4:4 | 1.146 (1.09–1.22) |
| mozjpeg tuned for SSIM | 1.186 (1.08–1.25) |
| libjpeg-turbo | 1.234 (1.17–1.37) |
| mozjpeg, table 1 | 1.362 (1.20–1.55) |

mozjpeg with table 2 (which cjpeg's usage describes as tuned for MS-SSIM) and 4:2:0 ties jpegli at
4:2:0, and jpegli is C++, which the browser build can't link. pngdeer keeps mozjpeg's progressive
scans and trellis quantization (its defaults), and writes baseline-compatible 8-bit tables, which
every decoder reads.

**Same bytes everywhere.** The encoder is vendored and built as plain C (vendor/mozjpeg/README.md):
no SIMD, no fused multiply-add, math functions from Rust's libm, no environment lookups. Built that
way it writes cjpeg's own files byte for byte (cjpeg from the same tag with `-ffp-contract=off`, 12
photos at three qualities), and the same files natively and in WebAssembly. tests/jpeg.rs pins
three whole compressions; macOS (clang), Linux arm64 (GCC 12) and the wasm build reproduce them.

**Decoding** is zune-jpeg's, without its SIMD code: plain Rust, so the same pixels on every target,
and its errors come back as errors (libjpeg's decoder ends on a longjmp, which the browser build
can't make). A file cut short in its scans decodes as viewers show it, gray below what arrived.
CMYK JPEGs are refused.

## Round 2: what TinyPNG does

- **Quality by size, not content.** Its quantization tables aren't libjpeg's scaled ones, but the
  nearest standard quality says enough: about 62 on every camera original (62–66, one 72), 65 on
  the 5.5 MP web photos, 69–81 at 2–2.5 MP, 85–87 at 0.6–1.1 MP, and never above the photo's own
  (the 0.6 MP photo stays at its 87, 93% of its bytes). Its scores spread wide as a result: 49–86,
  median 66.9 (65.8 on the originals, 68.3 on the web photos).
- **4:4:4 stays 4:4:4** (21 of the originals), the rest 4:2:0; always progressive.
- **All metadata goes**, the ICC profile too: a photo in Display P3 or Adobe RGB then shows in the
  wrong colors. pngdeer keeps an ICC profile unless it is sRGB's (what viewers assume anyway) and
  the EXIF orientation (a phone photo would otherwise show sideways), and drops the rest of EXIF
  (camera, place, time), which is mostly bytes and sometimes private.
- It kept 18% of the 60 photos' bytes in total.

## Round 3: automatic quality

pngdeer's PNG and WebP output aim at TinyPNG's quality through a reference file whose score tracks
TinyPNG's per image. For JPEG no reference does: mozjpeg's file at any quality from 50 to 85, with
libjpeg's table or table 2, scores with a correlation of −0.11 to +0.13 with TinyPNG's file, since
TinyPNG goes by size. Models of TinyPNG's score per photo, each fitted on the other 59 photos and
tried on the one left out (error: model minus TinyPNG's score):

| Model | Mean error | Median error | Within 3 points | Web / originals (median) |
|---|---|---|---|---|
| a constant (66.9) | 4.73 | 3.24 | 25 | −1.6 / +1.2 |
| **a − b ln(megapixels)** | **4.33** | 2.89 | 30 | **+0.4 / −1.2** |
| ln(megapixels) and the score at table 0, quality 60 | 4.34 | 2.38 | 32 | +0.9 / −1.8 |
| ln(megapixels) and the input's bits per pixel | 4.44 | 3.11 | 29 | +0.4 / −1.2 |
| table 2, quality 70's score, plus an offset | 4.83 | 2.30 | 37 | −2.4 / +0.6 |
| table 0, quality 65's score, plus an offset | 5.34 | 2.86 | 30 | −3.6 / +1.1 |

The reference files come closest on the typical photo but miss the most by far on others, and
lean by kind (2–4 points low on the web photos). The size model has the smallest mean error and
treats both kinds alike, and adding a reference to it buys nothing. So the automatic target is
**72.3 − 3.2 ln(megapixels)**, at most 90 as for PNG: 74.5 at 0.5 MP, 70.1 at 2 MP, 67.2 at 5 MP,
64.4 at 12 MP. TinyPNG's choice by size is a sensible one: a large photo is seen scaled down, which
hides what its compression did. It also costs nothing to compute, where a reference file cost an
encode and a scoring. The rest of TinyPNG's spread isn't in anything measured here; pngdeer lands
within 3 points of TinyPNG's score on half the photos.

## Round 4: against TinyPNG

| On the 60 photos, against TinyPNG | Bytes (median photo / total) | Points (median) |
|---|---|---|
| **pngdeer, at TinyPNG's score** | **89.5% / 86.7%** (all 60 reached) | |
| **pngdeer, automatic quality** | **82.4% / 84.2%** | +0.2 |
| mozjpeg at its default quality (75) | 107.0% / 105.6% | +5.0 |

- **At TinyPNG's score** (its score as the target), pngdeer's file is smaller on 57 of the 60
  photos (from 45% to 112% of TinyPNG's; middle half 82–96%): 85% on the median original, 94% on
  the median web photo, which TinyPNG compressed less to begin with.
- **Automatic quality**: smaller than TinyPNG's file on 52 photos (middle half 75–93% of its
  bytes), within −2.7 to +3.1 points of its score on the middle half. The originals come out at
  78% of TinyPNG's size, 0.8 points lower; the web photos at 93%, 0.7 points higher. Qualities
  40–82, median 63.5 (the search starts there); every photo reached its target, none came back
  as the original. 2.3 s per photo (median) on a 14-core Mac.
- **The open-source encoders**, each given its best quality per photo after the fact (a sweep,
  pooled into one frontier; pngdeer's own encoder settings measured the same way for comparison):

  | At TinyPNG's quality | Bytes (median / total) | Reached | Points at TinyPNG's size |
  |---|---|---|---|
  | pngdeer's settings (mozjpeg, table 2, 4:2:0), qualities 50–85 | 87.6% / 86.4% | 59 | +3.3 |
  | jpegli, defaults and 4:2:0, qualities 40–90 | 89.2% / 87.2% | 60 | +3.1 |
  | mozjpeg's defaults, qualities 40–90 | 94.2% / 89.1% | 59 | +1.8 |

  Out of the box, mozjpeg's default quality is 5 points above TinyPNG at 7% more bytes.

## Speed and memory

A round's 3 qualities are encoded and decoded at once, then scored as many at once as fit in 8 MP
(`SCORED_AT_ONCE`): scoring three 12 MP files at once peaked at 2.7 GB, one at a time at 1.3 GB,
and one at a time is about 25% slower, so only large photos take turns. In the browser a 40 MP JPEG
peaks at 3.0 GB of wasm32's 4 (79 bytes a pixel, as for PNG), so the browser's 40 MP limit holds for
JPEG too; the threaded build compressed one in 42 s, to the same bytes as the command line.

## Reproduce

```bash
python3 experiments/jpeg/compare.py run PHOTOS TINY results.csv --cjpeg CJPEG --cjpegli CJPEGLI
python3 experiments/jpeg/compare.py summary results.csv
```

PHOTOS holds `web/` and `orig/` (the files of `sources.json`), TINY TinyPNG's file for each as
KIND-NAME (bench.py's `TinyPNG.compress` fetches and caches them: 60 compressions). CJPEG is
mozjpeg 4.1.5's cjpeg, CJPEGLI jpegli's cjpegli (libjxl 031a0077f579).
