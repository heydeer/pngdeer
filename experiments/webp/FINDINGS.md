# WebP output: findings

pngdeer's WebP output (`pngdeer --format webp`, `pngdeer::compress_webp`, src/webp.rs): per image,
the smaller of two WebP files that keep the quality the PNG search settled on. Measured on the
benchmark's 180 images (bench/images, 109, and bench/images-holdout, 71) with bench.py and
compare.py (both in this repository; results stay in the ignored bench/results/).

## Round 1: phase one (core and command line)

- **Encoder: libwebp 1.6.0**, vendored and built for every target alike (vendor/libwebp/README.md):
  BSD-licensed, the reference encoder (Chrome, cwebp). The pure-Rust zenwebp is AGPL or commercial,
  which doesn't fit pngdeer's MIT license.
- **Same bytes everywhere.** Plain C only (no SSE/NEON), no fused multiply-add, libwebp's log, pow
  and friends from Rust's libm crate and its qsort a stable sort. The wasm32 build writes the same
  files as the native one (libwebp alone, and all of `compress_webp` on tests/webp.rs's four pinned
  cases), and the native files matched Homebrew's cwebp 1.6.0 with its NEON code byte for byte.
- **Two candidates, because neither wins everywhere.** A first look (11 images, scores by the
  ssimulacra2 tool, lossy quality bisected with cwebp -m 6 -sharp_yuv to match the PNG's score):
  pngdeer's own palette image as lossless WebP was always 2–31% smaller than its PNG, while lossy
  WebP was 11–70% smaller on photos, 3D, illustrations and 2× screenshots but 129–170% bigger on
  flat icons and 1× UI, or never reached the score. So both are made and the smaller one that
  reaches the target ships:
  - lossless: the PNG's pixels (the palette, or the source where the PNG kept it), so the PNG's
    score; `-z 3` and `-z 6`, the smaller kept;
  - lossy: the lowest quality (integer, 0–100) whose file reaches the target, at `-m 6`, sharp YUV,
    the alpha plane lossless.
- **The target is the PNG's**: the automatic one (TinyPNG's quality, as for PNG) or `--target`. A
  WebP file is then the same quality as pngdeer's PNG of that image, in fewer bytes.
- **Settings tried** (wpauto with each, all 180 images; sizes against the default's):
  - method 5 instead of 6: 1.0% bigger in total (photos 1.8%), no faster;
  - sharp YUV off: on sun-poster and stripe@2x no quality reaches the target at all;
  - lossless `-z 9` where the lossy file isn't 10% smaller: where lossless won it saved 1.6% in
    total (0.3% on the median image) under 1 MP, 2.3% over (2.8% median), for about 20 times the
    time (15 s on a 5 MP screenshot). Off by default; `--webp-lossless-level 9` for experiments.
  - alpha filtering 0, 1 or 2: the same bytes.
- **Speed.**
  - With translucent pixels and `-m 6`, libwebp compresses the alpha plane as hard as `-z 9`
    (alpha_enc.c: quality 100, method 6), on every try: 23 s for cup-of-tea's search. The VP8 data
    doesn't depend on how the alpha plane is stored (checked with cwebp -alpha_method 0 and 1), so
    the search's tries store it uncompressed, are sized without it, and only the file that ships
    compresses it: same files, 2.8 s.
  - The quality search aims each try where the two closest scores say the target is crossed (a
    0.5-point-per-step guess from the first try at 90), halving when that stalls: 3–5 tries on
    most images instead of 7–8. Scores aren't quite monotonic in quality, so on a few images it
    lands on another quality that reaches the target (stripe@2x q86 instead of q88, 12% smaller;
    figma@2x q70 instead of q55, 11% bigger); totals didn't move.
- **Misses.** Where the palette can't reach the target (a fixed `--target` beyond it), the lossy
  file that comes closest ships if it scores higher than the palette (on a 96×80 test image: 79.4
  against 28.3 for a target of 80).
- **Colors.** WebP carries color information only as an ICC profile: an iCCP chunk becomes the
  file's ICCP (with a VP8X header), sRGB needs none, and cICP or non-sRGB gAMA/cHRM without a
  profile are refused, as are animated PNGs, rather than written in the wrong colors.

### Results

All 180 images at the automatic target (wpauto, as shipped: no `-z 9`), against pngdeer's PNG
(pdauto) at the same target and against cwebp (`-m 6 -sharp_yuv -alpha_q 100` at qualities 50–100,
and `-lossless -z 9`), every file scored by the ssimulacra2 tool after dwebp:

| Images | n | WebP / PNG, median | WebP / PNG, total | P10 – P90 | cwebp / WebP at the same score, median |
|---|---|---|---|---|---|
| all | 180 | 77.9% | 59.5% | 38.9% – 90.8% | 106.3% (total 119.8%) |
| lossy won | 89 | 48.8% | 45.9% | 31.0% – 71.8% | |
| lossless won | 91 | 87.2% | 91.8% | 78.5% – 92.2% | |
| photos | 24 | 47.9% | 49.1% | 38.9% – 59.6% | 101.0% |
| photos with alpha | 28 | 43.4% | 47.9% | 26.0% – 88.5% | 102.1% |
| 3D | 34 | 60.5% | 61.1% | 43.1% – 85.3% | 103.1% |
| UI | 25 | 78.6% | 68.6% | 38.8% – 92.2% | 126.3% |
| icons | 30 | 86.8% | 85.5% | 79.9% – 90.5% | 124.8% |
| illustrations | 39 | 89.0% | 79.1% | 53.8% – 93.1% | 162.8% |

- WebP scores the same as the PNG (median difference +0.00; +0.32 on photos, where the lossy
  file lands just above the target).
- "cwebp at the same score" gives cwebp the best of its eleven settings for each image, picked
  after the fact (log size interpolated in score, as bench.py does against TinyPNG), which cwebp
  alone can't do. It needs about the same bytes where the lossy file wins (photos, 3D: the same
  encoder), and a quarter to two thirds more where pngdeer's palette does (UI, icons,
  illustrations). Against one setting for all, cwebp -q 75 is smaller (wpauto 124% of it on the
  median image) but scores 9.8 points lower.
- Time (seven images at a time on 14 cores): 2.6 s per image on the median (PNG 1.7 s), 3.5 s
  mean, 12 s at most. With lossless `-z 9` where lossless might win it was 3.6, 4.8 and 25 s for
  0.7% less in total; before the alpha and quality-search changes, 5.7, 7.5 and 41 s.

### Open

- The browser build (phase two): the same code compiles to wasm32 and writes the same bytes, but
  WebP takes about twice as long as PNG on the command line, and the browser is slower still.
- Qualities in steps finer than 1 could land closer to the target; libwebp takes a float.

## Round 2: TinyPNG's WebP

TinyPNG converts to WebP on request (a shrink, then a conversion of its result: two compressions,
checked on the account's count). With 249 compressions left in the month, 120 images: all 71
held-out ones and 49 of the 109 tuning images, spread over the categories as the set is (every
`n/k`-th by name). Against them, pngdeer's WebP at targets 40–98 and cwebp at qualities 10–100 and
lossless, each read at TinyPNG's score and at its size as bench.py places TinyPNG's PNG on the PNG
tools' curves.

- **TinyPNG's WebP is a lower quality than its PNG**: 80.0 on the median image (P10 67.4) against
  87.2 for its PNG, 6.0 points lower per image on the median, in 41% of its PNG's bytes. It is
  lossless WebP on 38 images and lossy on 82 (50 of them with an alpha plane).
- **At the same quality pngdeer's WebP is about 3% smaller:**

  | Images | n | pngdeer's bytes at TinyPNG's score / TinyPNG's, median | total | pngdeer's score at TinyPNG's size, difference (median) | P10 |
  |---|---|---|---|---|---|
  | all | 120 | 96.9% | 96.5% | +0.9 | -1.3 |
  | 3D | 22 | 101.2% | 96.6% | -0.3 | -3.8 |
  | icons | 19 | 95.2% | 90.1% | +2.9 | +0.1 |
  | illustrations | 26 | 93.9% | 98.6% | +1.1 | -1.3 |
  | photos | 17 | 96.0% | 94.2% | +1.4 | +0.6 |
  | photos with alpha | 19 | 98.9% | 94.9% | +0.3 | -1.2 |
  | UI | 17 | 99.6% | 96.8% | +0.1 | -1.6 |

  Two illustrations are left out: TinyPNG's lossless WebP scores 88.5 and 92.2 on them, above
  anything pngdeer makes (87.5, 91.9): a palette of 256 colors can't get there, nor lossy WebP
  (thin lines in color), and pngdeer has no lossless candidate with more colors.
- cwebp at its best quality for each image, picked after the fact, needs 102% of TinyPNG's bytes
  on the median image and 115% in total: TinyPNG's WebP is about libwebp at a quality picked per
  image, with a lossless (likely palette) file where that is smaller, which is what pngdeer does;
  pngdeer's edge is its palette and hitting the target more closely.
- As each tool ships, at its own quality: pngdeer's WebP (automatic: the PNG's target, about
  TinyPNG's PNG quality) is 136% of TinyPNG's WebP on the median image and scores 7.9 points
  higher. TinyPNG's WebP is 44% of pngdeer's PNG, at 7.7 points lower.

## Round 3: automatic quality from TinyPNG's WebP

Round 2 left pngdeer's WebP at its PNG's quality, which TinyPNG's WebP undercuts by 6 points per
image. Calibrated here on the 120 images with TinyPNG's WebP, and checked on the other 60.

- **How TinyPNG picks it.** Lossless on 38 of the 120 (all icons, 12 illustrations, 7 UI), scoring
  exactly its PNG's score: its palette PNG, stored losslessly. Lossy on 82, and there its score
  follows a fixed libwebp quality, not the PNG's: pngdeer's PNG target predicts it poorly (median
  9.8 points above, P10 5.2, P90 18.4, correlation 0.47), cwebp at one quality for every image well
  (quality 70 with `-m 6 -sharp_yuv`: 0.15 points under on the median image, correlation 0.94, the
  same bytes, 1.001× on the median). Quality 70 holds on both halves: −0.17 on the 34 lossy tuning
  images, −0.05 on the 48 held-out ones (60: −2.9 and −2.6; 75: +0.9 and +1.6).
- **The rule** (src/webp.rs): with automatic quality, a lossy file at quality 70 is weighed
  against the lossless file of pngdeer's PNG, as TinyPNG would. Where the lossless file is smaller,
  the PNG's target stays (TinyPNG ships its palette there). Otherwise the lossy file's score is the
  target (at most 90): the lossy search finds the lowest quality reaching it (sometimes below 70:
  stripe@2x reaches it at 61, 9% smaller), and a palette made for that lower target competes too.
- **When the palette search runs again**: it won on 16 of the 113 images whose target moved, 5–44%
  smaller than the lossy file (3D icons, diagrams, UI), always where the first palette's lossless
  file was at most 2.5 times the lossy one. Running it only up to 3 times (PALETTE_REACH) skips it on
  53 of the 113 with the same files, and brings the median time from 5.1 s back to 2.9 s.
- **A stand-in for TinyPNG** on the 60 images without its WebP (experiments/webp/tiny_emulate.py,
  nothing fetched): its cached PNG losslessly, or cwebp `-q 70 -m 6 -sharp_yuv` of the source,
  whichever is smaller. On the 120 it picks the same kind 119 times, at 100.0% of TinyPNG's bytes
  (median and total, P10 95.4%, P90 102.5%) and the same score (median +0.00). Its lossy files are
  pngdeer's quality-70 reference itself, so on photos it can only show a draw; the real TinyPNG's
  lossy files came out about 5% bigger than pngdeer's.

Results (as shipped; scores by the ssimulacra2 tool):

| Images | n | Size, median | Size, total | Score difference, median |
|---|---|---|---|---|
| **120 calibration images, against TinyPNG's WebP** | 120 | 96.4% | 96.1% | −0.15 |
| 3D | 22 | 101.2% | 95.2% | −0.36 |
| icons | 19 | 91.4% | 81.1% | −1.64 |
| illustrations | 26 | 97.5% | 96.1% | +1.00 |
| photos | 17 | 94.6% | 95.1% | −0.27 |
| photos with alpha | 19 | 97.4% | 98.4% | −0.16 |
| UI | 17 | 94.6% | 94.5% | −0.16 |
| **the other 60, against the stand-in** | 60 | 99.7% | 96.8% | +0.00 |
| 3D | 12 | 100.0% | 96.8% | +0.00 |
| icons | 11 | 94.7% | 83.2% | −0.94 |
| illustrations | 13 | 95.9% | 94.4% | +0.00 |
| photos | 7 | 100.0% | 99.7% | +0.00 |
| photos with alpha | 9 | 100.0% | 99.8% | +0.00 |
| UI | 8 | 98.4% | 97.2% | +0.09 |

- Icons score 1–1.6 points under TinyPNG's: both are lossless palettes there, at pngdeer's PNG
  target, which sits that far under TinyPNG's PNG score on icons (as for PNG output).
- Against pngdeer's own output, all 180: 45.6% of its PNG on the median image (34.5% in total) at
  6.3 points lower, and 74.3% of round 1's WebP (58.0% in total) at 6.4 lower; icons, most
  illustrations and UI keep the PNG's quality (lossless won), photos drop 17 points (to TinyPNG's
  WebP quality). Median score 80.4 (the stand-in's 79.8, the PNG's 88.1). Lossy won 97 images,
  lossless 83.
- Time: 2.9 s per image on the median (round 1: 2.6 s; PNG 1.7 s), 4.4 s mean, 14.5 s at most.
- `--target` is unchanged: a given target is the WebP's too. `--webp-reference none` keeps round 1's
  automatic quality (the PNG's).

## Round 4: widening the lead

Three ideas, measured with tools/webplab: for each image and libwebp setting, the bytes of the
smallest lossy file that reaches the image's target, scored as pngdeer scores; the target is
TinyPNG's WebP's score (its file re-scored by pngdeer). Summaries by experiments/webp/lab_report.py,
on the 90 of the 120 images where lossy WebP reaches that score at all.

- **libwebp's finer settings**, one at a time (bytes against libwebp's defaults, median / total):
  auto filter (`-af`, the deblocking filter adjusted per segment) 96.6% / 93.5%, the best of all,
  and the best setting on 45 of the 90 images; filter sharpness 3 or 6 98.6% / 96%; the picture
  and photo presets 98.5% / 96%; filter strength 0 100% / 95.8%; spatial noise shaping 75 or 100
  about 99–100%; method 5 100.8% / 99.6%; no sharp YUV 100.3% / 102.4%; one segment 105% / 109%.
  The best of all 23 for each image, picked after the fact, 95.9% / 92.1%.
- **Combined with the auto filter** (with quarter-step qualities): auto filter alone 97.0% / 93.8%;
  with filter sharpness 3, 6 or 7 97.0% / 93.2%; with noise shaping 75 and sharpness 6 96.5% /
  93.6%; the best of twelve combinations per image 95.5% / 91.8%; the better of the auto filter
  with and without sharpness 6 per image 96.5% / 93.0%, for twice the search.
- **Finer quality steps**: quarter steps below the lowest whole quality that reaches the target
  saved 0.0% on the median image and 0.3–0.5% in total, for 1.5 more files scored per image.
- **Beyond 256 colors**: a near-lossless file (levels 0–80) reached the target in fewer bytes than
  what pngdeer ships on 2 of the 120 images (7.6% and 0.3% smaller). Elsewhere a palette file is
  smaller (libwebp skips near-lossless for images that fit a palette). On the two illustrations
  where TinyPNG's lossless file scores above anything pngdeer makes, a lossless file of the source
  is 2.7 times TinyPNG's: its palette, not the encoder, is what pngdeer lacks there.
- **Adopted**: the auto filter for the lossy search (`WebpOptions::autofilter`, `--webp-autofilter`).
  The quality-70 reference keeps libwebp's defaults, since it stands for TinyPNG's file, and stays
  a candidate: the auto filter loses on some images (kodim01: 1.2% bigger), so pngdeer's WebP is
  never bigger than that file. The rest gained under 1% each for more time, and were left out.

Results, all 180 images (as shipped):

| Against | n | Size, median | Size, total | Score difference, median |
|---|---|---|---|---|
| TinyPNG's WebP (the 120) | 120 | 94.6% | 93.8% | +0.07 |
| 3D | 22 | 97.1% | 92.3% | +0.10 |
| icons | 19 | 91.4% | 81.1% | −1.64 |
| illustrations | 26 | 94.7% | 94.3% | +1.14 |
| photos | 17 | 93.7% | 93.4% | +0.07 |
| photos with alpha | 19 | 96.1% | 97.1% | +0.12 |
| UI | 17 | 89.6% | 89.5% | −0.16 |
| the stand-in (the other 60) | 60 | 96.6% | 94.2% | +0.11 |
| round 3's WebP (all 180) | 180 | 100.0% | 97.5% | +0.00 |

- Against TinyPNG's WebP, from 96.4% / 96.1% at −0.15 points in round 3 to 94.6% / 93.8% at
  +0.07. Icons are unchanged (lossless, at pngdeer's PNG target, as in round 3).
- Time: 3.0 s per image on the median (round 3: 2.9 s), 5.4 s mean (4.4 s), 21 s at most
  (14.5 s): auto-filtered files take longer to encode, and the reference no longer starts the
  search's files where the settings differ.

## Round 5: speed, and the browser

Where a WebP image's time went (one lossy try on 1–5 MP images, natively): encoding 0.5–1.2 s at
method 6 with sharp YUV and the auto filter (0.2–0.4 s without the auto filter), of which sharp YUV
0.04–0.13 s; decoding 0.02 s; scoring 0.03–0.14 s (on all cores). Encoding is single-threaded, so:

- **Rounds of tries**: the lossy search encodes three qualities at once, four apart around its
  estimate (the reference's quality in automatic mode), then the three left inside the range they
  narrow it to: two rounds and six files on the median image, against five to nine tries one after
  another. The qualities a round tries don't depend on threads, so neither do the files (the
  single-threaded browser build writes the same ones).
- **One YUV conversion per image** (vendor/libwebp's `Source`): the files are the same as encoding
  the RGBA each time (tested), without converting it again.
- **One alpha compression per image**: the compressed alpha plane is the same at every quality and
  setting, so it is made once, alongside the PNG search, and put into the file that ships; that is
  libwebp's own file (tested). Before, the reference and the winner each compressed it (3 s on a
  1.6 MP cut-out).
- **Alpha effort 5 over 8 MP.** At method 6 libwebp compresses the alpha plane with its slowest
  lossless search (as cwebp -z 9): 13 s natively on a 24 MP cut-out, where it no longer runs
  alongside the PNG search, for files 0.25% smaller than at 5. Below 8 MP it stays at 6: on the
  benchmark's 95 images with an alpha channel, 5 took 250 → 196 s, mostly where lossless won
  anyway, for 0.6% more bytes in total, 1.1% on the 59 lossy files, up to 11% on one.
- **Alongside**: the lossy source with the first PNG search, the lossy search with the second PNG
  search, on images up to 8 MP (above, for memory, they take turns; a round's files are encoded at
  once at any size, since encoding never overlaps a scoring).
- **Tried and dropped: a preview.** Searching a downscaled preview (as the PNG search does) to aim
  the full-size rounds: the lowest quality reaching the target on the preview was 3 below the
  full-size one on the median image, but −6 to +11 (P10–P90), and on all 180 images the runs took
  457 s with it against 428 s without, in as many rounds.

All 180 images, one at a time on 14 cores (as the browser runs them): 718 s → 428 s (median
2.19 → 1.73 s, mean 3.99 → 2.38 s, at most 18.2 → 9.9 s). 169 files are byte-identical; 10 are
0.1–4% smaller and 1 is 1.5% bigger, where scores aren't monotonic in quality and the rounds land on
another quality reaching the target (all reach it); 99.98% of the bytes in total. Large images on
the command line: 16 MP 30.7 → 21.1 s, a 24 MP cut-out 60.8 → 37.7 s (47.8 s at alpha effort 6),
36 MP 66.0 → 42.6 s, with the same peak memory as their PNG (the PNG search's).

**The browser** (web/: a PNG / WebP switch; each file keeps its result in each format, so switching
back is instant):

- Byte-identical to the command line: the five samples in both browser builds (single-threaded in
  Node, threaded in headless Chrome on the page itself), and 16–36 MP images in the threaded one.
- Threaded, on the page: the samples in 2–7 s each (single-threaded in Node: 4.5–17 s).
- Threaded, on the page, 16–36 MP: 34–78 s, 1.5–1.8 times the command line's time. A 24 MP
  cut-out took 164 s, 4 times: fast-ssim2 linearized each translucent pixel with eight f64
  `mul_add`s, software fma calls in wasm, so its scoring was 8 times as slow as natively (43 s of
  its PNG search). Computed once per alpha and channel byte instead (vendor/fast-ssim2/PATCHES.md,
  6; the same bits), it takes 67 s, and its PNG output 23 s instead of 56 s.
- libwebp adds about 0.45 MB to each build (1.34 MB single-threaded, 1.92 MB threaded).

## Round 6: a leaderboard

The PNG leaderboard's method (README; experiments/palette-feedback/tool_rank.py) with TinyPNG's WebP
as the yardstick: per tool, all its settings form one size–quality frontier per image. Runs
`bench/results/wlb-{main,holdout}-20261028-091328`, summarized by experiments/webp/leaderboard.py.
TinyPNG's real WebP for the 120 images it was fetched for (WebP's settings were chosen on these),
the stand-in for the other 60.

The contenders. Every open-source WebP tool we know of encodes with libwebp (Squoosh, sharp,
Pillow, ImageMagick, FFmpeg), so they differ in settings, which cwebp covers:

- **pngdeer**: 12 targets (40–98) and automatic.
- **cwebp**: 27 settings, best per image picked after the fact: -q 10–100 (13 steps) at -m 6
  -sharp_yuv, the same with -af (pngdeer's lossy encoder settings), and -lossless -z 9.
- **pngquant + cwebp**: pngquant at 60–100, with and without dithering (as in the PNG
  leaderboard), stored as lossless WebP (-z 9): TinyPNG's palette route from open-source parts.

Against TinyPNG's WebP (120 images):

| Tool | Size at TinyPNG's quality, median / total | Points | Reaches |
|---|---:|---:|---:|
| **pngdeer** | **95.0% / 93.9%** | **+1.6** (all 120) | 118 |
| TinyPNG (yardstick) | 100% | 0 | 120 |
| cwebp, best of 27 | 97.9% / 110.2% | +0.9 (102) | 120 |
| pngquant + cwebp, best of 10 | 135.0% / 212.1% | +1.1 (58) | 115 |

- Points (score at TinyPNG's file size) exist only where TinyPNG's file is no smaller than a
  tool's smallest, so the medians cover different images: pngquant + cwebp's 58 are mostly where
  TinyPNG went lossless itself. On the 40 images all three cover: pngdeer +2.6, pngquant + cwebp
  +1.9, cwebp −0.3. Size at TinyPNG's quality covers nearly every image.
- By category (median size at TinyPNG's quality, pngdeer / cwebp / pngquant + cwebp): 3D 97 / 100 /
  142%, icons 95 / 118 / 96%, illustrations 92 / 97 / 100%, photos 92 / 93 / 317%, cut-outs 97 / 97 /
  229%, UI 96 / 98 / 105%. pngdeer is under TinyPNG in every category; cwebp loses on icons, where a
  palette wins, and pngquant everywhere else.
- pngdeer can't reach 2 (and 2 of the 60): line drawings where TinyPNG's palette scores 0.2–1.0
  above pngdeer's best palette and lossy can't get there; pngdeer has no truecolor lossless WebP.
- One setting, as tools ship: cwebp's default (-q 75) is 105.8% of TinyPNG's size (111.8% in
  total) at −0.32 points, bigger and worse on 64 of the 120; -q 80 (sharp's and Pillow's default
  quality, at method 4 as here) 124.9% at +2.23. pngdeer's automatic quality: 94.6% / 93.8% at
  +0.07, bigger and worse on 1.

Against the stand-in (the other 60): pngdeer 95.8% / 92.0%, +1.2, reaches 58; cwebp 99.7% /
111.8%, +0.4 (50), 60; pngquant + cwebp 115.9% / 181.5%, +1.9 (34), 57; on the 24 images all
cover, +3.2 / +0.1 / +2.2. cwebp -q 75: 110.4% at −1.89; pngdeer automatic 95.9% / 94.1% at +0.13.
The cwebp files are byte-identical to round 1's.
