# AVIF: measured, then built

2026-10-30. Should slimdeer write AVIF? Measured before building anything (rounds 1 to 8, no product
code changed): how small AVIF files are at the same quality as slimdeer's JPEG and WebP and TinyPNG's
AVIF, which encoder to use, how fast, and whether the same bytes come out everywhere. Round 9 put it
on the command line.

**In short:** with libaom, AVIF photos are 24% smaller than slimdeer's own JPEG at the same score and
19% smaller than TinyPNG's AVIF at TinyPNG's score. rav1e, the encoder that is easy to ship (pure
Rust, same bytes natively and in wasm, checked), gives only half that: 13% under slimdeer's JPEG and
about TinyPNG's AVIF's size (bigger on camera originals). On the PNG benchmark AVIF beats slimdeer's
WebP on 3D renders and photos, with or without alpha, on about half the screenshots and
illustrations, and loses badly on icons. Against TinyPNG's own AVIF, libaom wins on every one of
the 54 images tried (30 photos, 24 PNGs of every category) when it may use slower settings (speed 4,
speed 2 for one icon) and pick among a few per image; at its default speed it loses on 6 photos and
9 PNGs. libaom is also the fast one: at speed 8, 1.3% bigger than at its default, it encodes a 12 MP
photo in 0.8 s natively, where rav1e at speed 6 takes 19 s in wasm. Built as plain C, libaom writes
the same bytes on macOS, Linux arm64 and x86-64 and in wasm (82 encodes each), runs in wasm at a
quarter to a third of its NEON speed (a 12 MP photo: 1.8 s at speed 8, 7.7 s at 6, 78 s at 4), and
adds 1.1 MB gzipped to the download. With a small patch its SIMD writes those same bytes too, in
round 7: natively 2–4 times faster, and in wasm, through SIMDe from its NEON code, 1.4–2.2 times
faster (the 12 MP photo at speed 4: 38 s) at the same 1.1 MB gzipped. Run automatically, with
fixed rules (round 8: a speed-8 search, a speed-4 final encode, four settings tried on small images),
it is smaller and no worse than TinyPNG's AVIF at TinyPNG's score on 52 of the 54 images (81% of its
bytes, median and total); on the other two it is 100.1%, and 105.0% but 1.4 points better, one
quality step short of TinyPNG's score. TinyPNG writes every AVIF at one quantizer, so its
quality can't be predicted image by image; an automatic target aimed at it on the median gives files
21–24% smaller, as good on the median, and one at slimdeer's own quality files about TinyPNG's size,
7.6 points better. slimdeer's AVIF will aim at TinyPNG's AVIF's quality. Round 9 built it into the
command line (libaom vendored, NEON on ARM, scored exactly as Chrome shows the file, the same bytes
on macOS and Linux, ARM and x86): at each image's TinyPNG score its files are 80% of TinyPNG's AVIF
(median and total), smaller and no worse on 52 of the 54, and at its automatic target 79% / 76%,
0.7 points better on the median, in 9 s on the median image.

## Setup

- **Images:** experiments/jpeg's photos, the first 15 of each kind (15 web copies, 0.6–6.1 MP; 15
  camera originals, 6.1–15.6 MP); and bench/images, the PNG leaderboard's 109 images.
- **libaom 3.15.2** through avifenc (libavif 1.4.2, Homebrew; NEON), one thread, speed 6 (libavif's
  default) unless noted. libaom tunes still images for image quality by default (`tune=iq`).
- **rav1e 0.8.1** through ravif 0.13.0 (tools/aviflab), one thread, speed 6. ravif writes 10-bit
  4:4:4 only, with rav1e's psychovisual tuning.
- **TinyPNG's AVIF** for the 30 photos (60 compressions: a shrink, then a conversion; bench.py's new
  `convert_avif`): all 10-bit 4:2:0, without ICC profile or EXIF.
- **Scores:** each file decoded with avifdec (dav1d) to 8-bit, then slimdeer's `--score` against the
  photo (photos) or bench.py's scoring (PNGs). Comparisons are on frontiers as in experiments/jpeg:
  each encoder sweeps qualities, and the bytes it needs for a score are interpolated.

## Round 1: photos, against slimdeer's JPEG and WebP

At the score of slimdeer's JPEG at automatic quality (median 67.0), the bytes each AVIF setting needs
against that JPEG's (30 photos, all reached):

| Encoder, chroma, depth | Median photo / total | Web / originals (median) | Points at the JPEG's size |
|---|---|---|---|
| **libaom 4:4:4 10-bit** | **76.4% / 77.6%** | 80.8% / 75.0% | +6.8 |
| libaom 4:2:0 10-bit | 77.2% / 78.4% | 81.7% / 74.8% | +6.5 |
| libaom 4:4:4 8-bit | 77.7% / 79.2% | 81.3% / 75.9% | +6.3 |
| libaom 4:2:0 8-bit | 78.5% / 80.1% | 82.4% / 75.5% | +6.2 |
| rav1e (4:4:4 10-bit) | 86.5% / 88.8% | 89.1% / 82.6% | +3.7 |

Against slimdeer's WebP at its own automatic score: libaom 4:4:4 10-bit 75.5% / 75.6%, rav1e 86.2% /
86.5%. So libaom AVIF saves about a quarter of the bytes that slimdeer's JPEG or WebP needs, more on
camera originals than on web copies. rav1e saves about half as much. Chroma and depth matter little
for libaom; 4:4:4 at 10 bits is best by a point or two.

## Round 2: against TinyPNG's AVIF

**TinyPNG's AVIF** is 59.8% of its JPEG's bytes on the median photo (71.6% in total), and 3.7
points worse (median score 62.3, from 47 to 76). Unlike its JPEG, it doesn't aim lower on larger
photos: the camera originals score higher (median 64.5) than the web copies (60.1).

| At TinyPNG's AVIF's score | Median photo / total | Web / originals (median) | Points at its size |
|---|---|---|---|
| **libaom 4:4:4 10-bit** | **80.8% / 82.3%** | 79.0% / 90.5% | +5.1 |
| libaom 4:2:0 8-bit | 82.9% / 84.6% | 80.6% / 94.3% | +4.3 |
| rav1e | 97.9% / 95.6% | 96.0% / 105.3% | +1.0 |

Photo by photo, libaom at speed 6 is smaller than TinyPNG's AVIF on 24 of the 30 (the best of its
four chroma and depth settings for each photo: still 24), and up to 7.6% bigger on the other six,
five of them camera originals; rav1e is smaller on 16. On those six, speed 4 wins every one (89.4–
99.0% of TinyPNG's bytes; 4:2:0 10-bit), where it saved only 2.6% on the median web photo (round 6).
Encoding TinyPNG's own shrunk JPEG instead of the photo, in case TinyPNG converts that, costs 58–117%
more, and `tune=ssim` doesn't help (99.7–111.6%). So beating TinyPNG's AVIF on every photo needs
libaom's slower settings, at least for the final encode.

At slimdeer's automatic target as it stands (JPEG's), libaom's AVIF would be about the size of
TinyPNG's (104.7% median, 89.1% total) and 6.7 points better. An AVIF output would need its own
automatic target, fitted to TinyPNG's AVIF as experiments/jpeg fitted JPEG's.

## Round 3: the PNG benchmark

At TinyPNG's PNG score per image (its PNG files from earlier runs: bench/results smooth-main), the
bytes libaom's AVIF needs against slimdeer's WebP frontier (targets 40–98; the smaller of its palette
as lossless WebP and lossy WebP: bench/results wlb-main):

| Category | AVIF reaches | AVIF / slimdeer WebP (median) | AVIF smaller in | The smaller of the two, in total |
|---|---|---|---|---|
| 3d | 22/22 | 76.8% | 22/22 | 75.2% |
| icons | 10/20 | 225.9% | 0/10 | 100.0% |
| illustrations | 22/24 | 86.4% | 11/20 | 84.3% |
| photos | 12/12 | 83.1% | 12/12 | 84.9% |
| photos-alpha | 16/16 | 70.8% | 15/16 | 73.5% |
| ui | 14/15 | 83.1% | 8/14 | 74.4% |
| **all** | **96/109** | **80.7%** | **68/94** | **79.1%** |

Icons are the exception. They are palette PNGs already (64–250 colors) that TinyPNG barely changes
(scores 91–100; one comes back as it was), so matching TinyPNG's score means nearly lossless. This
sweep encoded them as avifenc does an 8-bit PNG by default, in 8-bit YCbCr, which can't get there:
the conversion alone moves almost every pixel by a level (quality 100, "lossless", scores 94 on
twemoji-1f308), and at quality 95 the 72-pixel emoji, which bench.py scores enlarged 4 times, stay
at 86–93. Half never reached TinyPNG's score. Alpha isn't the limit (lossless alpha moves the
score by less than 0.1). On three icons tried by hand, 10 bits lift quality 95 to 92–95 and RGB
coded losslessly (`--cicp 1/13/0`) scores 100, so the score is reachable, but at 2.6–3.7 times
TinyPNG's bytes: transforms code hard edges of flat color worse than a palette and deflate, and
every file carries 430 bytes of AVIF boxes plus a separate alpha image (435–654 bytes here), as much
as a whole 72-pixel PNG. libaom's screen-content tools (`tune-content=screen`: palettes of up to 8
colors per block) helped one of the three. rav1e reaches no icon at all and is worse than slimdeer's
WebP on illustrations and screenshots (105% and 115% median). An AVIF output for PNGs would need a
way out where AVIF loses, as JPEG output hands back the photo when it can't do better.

## Round 3b: PNGs against TinyPNG's AVIF

The fair opponent for an AVIF output is TinyPNG's AVIF, not its PNG. Fetched for 24 of the PNGs,
4 per category spread over each (`PICK=4`: 2 openmoji and 2 twemoji icons; 48 compressions). TinyPNG
writes 8-bit 4:4:4 with alpha for transparent images, 10-bit 4:2:0 otherwise, and aims well below its
PNG: 38.6% of the PNG's bytes and 7.4 points lower on the median image (icons: 129% of the bytes and
10 points lower, so its AVIF of an icon is bigger and worse than its PNG).

At TinyPNG's AVIF's score, the bytes each setting needs against it (median, and the images where
it's smaller):

| Category | libaom s6 (8-bit) | s6 10-bit | **s4 10-bit** | s6 10-bit screen | s6 4:2:0 10-bit | rav1e s6 | Best libaom per image |
|---|---|---|---|---|---|---|---|
| 3d | 92.9%, 4/4 | 92.1%, 4/4 | 82.4%, 4/4 | 85.2%, 4/4 | 92.1%, 4/4 | 94.8%, 3/4 | 81.6%, 4/4 |
| icons | 105.9%, 1/4 | 109.5%, 1/4 | 97.0%, 2/4 | 109.5%, 1/4 | 111.2%, 1/4 | 127.2%, 1/4 | 97.0%, 2/4 |
| illustrations | 99.1%, 2/4 | 100.7%, 2/4 | 91.6%, 4/4 | 100.7%, 2/4 | 120.9%, 1/4 | 143.0%, 0/4 | 91.6%, 4/4 |
| photos | 85.2%, 4/4 | 83.2%, 4/4 | 78.8%, 4/4 | 87.0%, 4/4 | 84.7%, 4/4 | 97.0%, 3/4 | 78.8%, 4/4 |
| photos-alpha | 91.6%, 3/4 | 90.5%, 4/4 | 84.4%, 4/4 | 94.2%, 2/4 | 92.9%, 2/4 | 105.5%, 2/4 | 84.4%, 4/4 |
| ui | 101.7%, 1/4 | 105.9%, 1/4 | 84.5%, 4/4 | 105.9%, 1/4 | 115.6%, 1/4 | 174.0%, 0/4 | 84.5%, 4/4 |
| **all** | 93.4%, 15/24 | 93.3%, 16/24 | **84.7%, 22/24** | 90.6%, 14/24 | 92.1%, 13/24 | 108.9%, 9/24 | **84.2%, 22/24** |

Speed 4 matters far more here than on web photos (round 6): it is the best setting on 20 of the 24.
Lossless RGB (identity matrix) needs 2.9 (icons) to 19.5 (photos) times TinyPNG's AVIF's bytes on
the median image of a category: TinyPNG's AVIF is far from lossless. The two icons lost on the sweep's interpolated curves (openmoji-1f308 101.0%, twemoji-1f600
102.4%) were searched again at every second quality with seven settings, keeping the smallest file
that reaches TinyPNG's score, as slimdeer's search would:

| Icon (TinyPNG's AVIF) | s6 8-bit | s4 8-bit | s4 10-bit | s4 10-bit screen | s2 10-bit | s4 4:2:0 10-bit |
|---|---|---|---|---|---|---|
| openmoji-1f308 (13,672 bytes, 88.7) | 110.0% | 100.7% | **99.7%** | 100.3% | 102.0% | not reached |
| twemoji-1f600 (1,326 bytes, 79.6) | 104.1% | 101.4% | 101.6% | 101.6% | **93.6%** | 113.1% |
| openmoji-1f4a1 (6,198 bytes, 88.8) | 112.0% | **92.8%** | 93.7% | 93.7% | 93.3% | 133.5% |
| twemoji-1f308 (1,511 bytes, 49.6) | 94.0% | 85.3% | 89.1% | 80.6% | 85.1% | **80.1%** |

So every image tried can be beaten, but not by one setting, and on icons only just (by 0.3% on
openmoji-1f308). No setting wins everywhere: 4:2:0 wins one icon and fails another, speed 2 wins one
icon and loses another to speed 4. A search over a few settings per image is cheap for small images
and expensive for large ones (speed 4 is 4.7 times speed 6's time, round 6).

## Round 4: scoring as browsers decode

AV1 decoding is exact, but the conversion to RGB isn't specified. Chrome 156 (createImageBitmap,
headless; experiments/avif/chrome.mjs) and avifdec differ by at most 2 levels per channel on 47–87%
of the pixels of 12 files (libaom 4:4:4 10-bit and 4:2:0 8-bit, rav1e, TinyPNG's; 3 photos), and
Chrome's pixels score 0.0–0.4 points lower. A real build should score the file as Chrome converts
it (libavif with libyuv's fixed-point conversion), not as avifdec does.

## Round 5: the same bytes everywhere

- **rav1e:** identical files on macOS arm64 (with rav1e's assembly and without), Linux arm64 (glibc,
  in Docker) and wasm32 (node): 3 photos at qualities 40 and 70. rav1e is integer arithmetic almost
  throughout; it calls the platform's libm in a few places (`exp` for the rate-distortion lambda),
  which a patched rav1e would route to the libm crate, as slimdeer does for its C code.
- **libaom 3.15.2, built as plain C** (`AOM_TARGET_CPU=generic`, `-ffp-contract=off`, one thread,
  libaom/): with Apple clang on macOS arm64, GCC 12 on Linux arm64 and x86-64 (glibc, Docker) and
  LLVM 23 for wasm32 (wasi-libc, run in node), aomenc's all-intra encodes as libavif makes them
  (tune=iq) came out identical on all four, 82 encodes each: 6 images (3 photos, 2 icons, a
  screenshot) at speeds 4, 6 and 8 and cq-levels 20 and 40, 10-bit 4:4:4, and 4 of them at 8-bit
  4:4:4 and 10-bit 4:2:0. Homebrew's NEON build agrees with them on 87 of 93 encodes compared; the
  6 that differ are all the 12 MP photo at speeds 8 and 9: some NEON code at the fast speeds isn't
  bit-exact with libaom's C, which is why slimdeer builds its C without SIMD (round 7 found the
  function, a libaom bug, and patched it out). The encoder calls libm's `exp`, `log`, `log2`,
  `log1p`, `pow`, `exp2`, `tanh`, `expf`, `log10` and `cos`, several on the all-intra path
  (allintra_vis.c, ml.c's neural networks, rd.c, intra_mode_search.c). Apple's, glibc's and
  wasi-libc's agreed on every encode, but a vendored libaom would route them to Rust's libm, as
  vendor/clib does for libwebp and mozjpeg, so they agree by construction.
- **Size:** the encoder alone is 4.0 MB of wasm, 1.13 MB gzipped (slimdeer's whole wasm is 0.61 MB
  gzipped today); libaom's decoder, for scoring, adds about 70 KB gzipped, as it shares most of its
  code with the encoder. Loading it only when AVIF is asked for would keep it off everyone else's
  download.

## Round 6: speed

One encode at quality 60, one thread, nothing else running (`compare.py speed`; libaom with its
NEON code, rav1e without assembly as wasm runs it). slimdeer's whole JPEG and WebP compression, which
uses every core, for scale:

| Photo | libaom s4 / s6 / s8 / s9 | rav1e s6, native / wasm | rav1e s10, wasm | slimdeer JPEG / WebP (whole) |
|---|---|---|---|---|
| 1.1 MP | 2.3 / 0.5 / 0.2 / 0.1 s | 1.4 / 2.0 s | 0.7 s | 0.6 / 1.2 s |
| 6.1 MP | 7.8 / 1.6 / 0.7 / 0.3 s | 5.2 / 8.1 s | 2.3 s | 1.3 / 4.7 s |
| 12.2 MP | 18.6 / 2.2 / 0.8 / 0.5 s | 12.0 / 18.6 s | 5.4 s | 2.8 / 14.3 s |

A search like JPEG output's (about 9 encodes, 3 at a time) at rav1e's speed 6 would take about a
minute on a 12 MP photo in the browser's threaded build, three in the single-threaded one, before
scoring. At speed 6 libaom with NEON is 3–5 times faster than rav1e without assembly.

libaom as plain C in wasm (node 24, one thread, cq-level 30, 10-bit 4:4:4), against its NEON build:

| Photo | Speed 4: NEON / wasm | Speed 6 | Speed 8 |
|---|---|---|---|
| 1.1 MP | 3.6 / 10.6 s | 0.7 / 1.9 s | 0.3 / 0.7 s |
| 6.1 MP | 7.7 / 25.3 s | 1.6 / 4.7 s | 0.6 / 1.7 s |
| 12.2 MP | 17.9 / 78.2 s | 1.9 / 7.7 s | 0.6 / 1.8 s |

wasm is 2.5–4.4 times slower than NEON; at speed 6 it is as fast as rav1e in wasm on the 1 MP
photo and 2.4 times faster on the 12 MP one. A
search at speed 8 (about 9 encodes, 3 at a time) takes about 6 s on a 12 MP photo in the threaded
browser build; a final encode at speed 4, which beat TinyPNG's AVIF on every image tried, takes
over a minute at 12 MP and 25 s at 6 MP, but under a second on icons and small screenshots.

What speed costs, on the 15 web photos at the score of slimdeer's JPEG, against the same encoder at
speed 6:

| Setting | Bytes (median / total) | Encode time (median) |
|---|---|---|
| libaom speed 4 | 97.4% / 96.8% | 468% |
| **libaom speed 8** | **101.3% / 101.6%** | **35%** |
| libaom speed 9 | 107.2% / 106.9% | 13% |
| rav1e speed 8 | 100.7% / 100.6% | 79% |
| rav1e speed 10 | 112.5% / 115.1% (13 photos reach the score) | 25% |

libaom at speed 8 keeps nearly all of its lead in a third of the time; rav1e only gets fast at
speed 10, where these web photos come out no smaller than slimdeer's JPEG.

## Round 7: SIMD with the same bytes

2026-10-31. Can the browser build be smaller and faster, with every platform still writing the same
bytes?

- **libaom's SIMD is meant to match its C.** Built with `CONFIG_EXCLUDE_SIMD_MISMATCH=1`, which
  leaves out the SIMD of a few floating-point neural networks, its SSE, AVX2 and NEON encodes are
  checked against its C ones by libaom's own test (test/av1_c_vs_simd_encode.sh), for its good and
  real-time presets, not all-intra. Built that way for arm64 (NEON, no SVE), 80 of the 82 encodes
  matched plain C; the 12 MP photo at speed 8 didn't.
- **The two functions that don't share one libaom bug.** From speed 8 all-intra switches to a fast
  real-time-style search that, for 10-bit content too, calls the 8-bit Hadamard transform and
  quantizer (`aom_hadamard_16x16`, `av1_quantize_fp`) on coefficients that don't fit 16 bits. Their
  C clamps or adds in 32 bits; the NEON quantizer truncates to 16 bits, and the SSE2 and AVX2
  Hadamard add in 16 bits. Found by bisection: on arm64 by rebuilding with rtcd functions sent back
  to C, on x86 with a switch that does it at run time (libaom/forcec.py, bisect.py).
  libaom/slimdeer.patch drops the SIMD of just those two. With it, against plain C: NEON on macOS
  82 of 82 identical; x86-64 (GCC 12, Docker, run-time CPU detection) 82 of 82 at each of SSE2,
  SSE4.1 and SSE4.2 (under Rosetta, which has no AVX) and at AVX2 (under QEMU's `-cpu max`, which
  reports AVX2 but not AVX-512); and the wasm build below 82 of 82, and also with screen-content
  tuning (speeds 4, 6 and 8, 10- and 8-bit), lossless and monochrome (how an alpha plane is coded).
  AVX-512 wasn't tried; libaom 3.15.2 has AVX-512 code only through Highway, which is off by
  default.
- **wasm SIMD from libaom's NEON.** libaom has none for wasm, but SIMDe implements NEON's
  intrinsics on wasm's SIMD128, exactly for the integer operations libaom's NEON uses.
  libaom/simd-wasm.py recompiles an arm64 build's files for wasm with SIMDe's arm_neon.h in place
  of the real one. All of its NEON: the same bytes, 1.5–1.8 times faster than plain C, but 6.5 MB,
  1.59 MB gzipped.
- **Then no bigger than plain C**, three ways:
  - Only 172 of libaom's 1,486 NEON functions ran in 10-bit encodes of photos, icons and
    screenshots (CPU profiles in node: libaom/profile.sh, hot.py; simd.json); the others go back to
    C, which writes the same bytes. SIMDe's NEON transforms alone are 1 MB of wasm, and the 8-bit
    half goes unused if slimdeer always encodes 10-bit, which it should anyway (round 1; icons need
    it, round 3).
  - Files that took under 0.1% of every encode's time (files.json) are built at -Os, the rest at
    -O2, which ran as fast as -O3 here and is smaller.
  - `SLIMDEER_STILL` (in slimdeer.patch): the encoder's function tables leave out the sub-pixel,
    compound, masked and OBMC functions only inter frames use, and the 8- and 12-bit sets of the
    high bit depth ones, so the linker drops them: 1 MB of plain C, but only 8% gzipped, as they
    are macro-made and compress well.

  The result is 3.2 MB, 1.11 MB gzipped and 0.90 MB with brotli, against plain C's 4.0, 1.13 and
  0.89 MB, and still writes plain C's bytes (82 of 82):

  | Photo | Speed 4: plain C / SIMD | Speed 6 | Speed 8 |
  |---|---|---|---|
  | 1.1 MP | 11.0 / 6.3 s | 1.95 / 1.27 s | 0.75 / 0.54 s |
  | 6.1 MP | 26.3 / 15.7 s | 6.1 / 3.5 s | 2.1 / 1.5 s |
  | 12.2 MP | 83.6 / 38.5 s | 7.8 / 4.0 s | 1.8 / 1.2 s |

  (wasm in node 24, one thread, cq-level 30, 10-bit 4:4:4, node's start-up included; plain C and
  SIMD timed in turn while other work ran, so plain C's are a little above round 6's.)
- **Natively** the patched libaom with NEON is 2.2–4.0 times faster than as plain C, with the same
  bytes, so the command line could use libaom's SIMD (NEON, and SSE2 to AVX2 picked at run time)
  if a check like hashes.sh guards it:

  | Photo | Speed 4: plain C / NEON | Speed 6 | Speed 8 |
  |---|---|---|---|
  | 1.1 MP | 8.1 / 3.2 s | 1.42 / 0.65 s | 0.54 / 0.27 s |
  | 6.1 MP | 19.6 / 6.8 s | 3.7 / 1.35 s | 1.22 / 0.58 s |
  | 12.2 MP | 59.2 / 14.9 s | 6.0 / 1.6 s | 1.32 / 0.54 s |

- **What didn't help:** speed 5 (98.0% of speed 6's bytes on the web photos, against speed 4's
  97.4%, in about speed 4's time); wasm-opt `-Oz` (nothing over `-O3` once cold files are `-Os`);
  dropping the files only inter frames need, as the encoder calls into them even for a still (to set
  up, or to check a flag and return), so they need call-site patches, and what can go without them
  saves 6% gzipped; dropping the 8-bit-only code (about 100 KB left before compression). In the
  SIMD build most of
  the time left is C that libaom has no SIMD for: trellis quantization (`av1_optimize_txb`),
  quantizing with quantization matrices (tune=iq turns them on and libaom's SIMD quantizers don't
  take them), and the gradient histograms that prune intra modes.

## Round 8: automatic, end to end

2026-10-31. The wins so far were at TinyPNG's score with settings picked per image after the fact.
Can an automatic AVIF output, with fixed rules, beat TinyPNG's AVIF? (auto.py; avifenc with libaom's
NEON code natively, one thread, 10 images at a time.)

- **TinyPNG's AVIF is one quantizer.** Every one of the 54 files (its alpha planes too) has
  base_q_idx 104 (quantizer 26, libavif's quality 59), quantization matrices at level 8, chroma
  delta-q +2, perceptual delta-q and CDEF off, which libaom all-intra writes with tune=ssim,
  `enable-qm`, `enable-chroma-deltaq` and `deltaq-mode=3`. So its score follows the image, from 47
  to 89. That reference encode isn't TinyPNG's, though: its files are 14% bigger than TinyPNG's on
  the median photo, scoring 6 points higher, and 5% bigger on PNGs, scoring about the same.
- **Its score can't be predicted image by image.** Each image predicted from a line through the
  others, the error is 6.7 points rms on photos for a constant, 6.9 for slimdeer's JPEG target (size
  doesn't matter here, round 2), 6.0 for the reference's score, and 7.1 even from TinyPNG's own JPEG
  of the photo (so its AVIF isn't just a conversion of that); on PNGs 10.0, 9.2 for PNG output's
  target, 5.9 at best (that target and the reference's score, their distances from 100 added). An
  automatic target can only match TinyPNG's AVIF on the median, as JPEG output's matches TinyPNG's
  JPEG. On the median TinyPNG's AVIF is 6.3 points below slimdeer's target as it stands on photos
  (JPEG output's) and 9.0 below on PNGs (PNG output's).
- **The rules.** Every setting is 10-bit 4:4:4, tune=iq, alpha at the color's quality. Over 0.5 MP:
  the lowest quality whose speed-8 file reaches the target, then the lowest at speed 4 from there
  (the speed-4 file is 14% smaller than the speed-8 one at the same target, median, and one quality
  step lower). Up to 0.5 MP, where encodes are cheap: the smallest of four settings, each at its
  lowest quality (speed 4; speed 4 with screen tuning; speed 4 at 8 bits; speed 2), which kept speed
  2 on 8 to 11 of the 13 small images. 9 encodes on the median image (8 for photos, 16–17 for PNGs).
- **Results** against TinyPNG's AVIF, three targets:

  | Target | Bytes (median / total) | Smaller | Score (median) | Smaller and not worse |
  |---|---|---|---|---|
  | **Each image's TinyPNG score** (the leaderboards' comparison) | **81.3% / 80.9%** | 52/54 | +0.6 | **52/54** |
  | About TinyPNG's AVIF's quality: photos 62.3 (TinyPNG's median), PNGs PNG output's target − 9.0 | 79.3% / 75.8% | 46/54 | +0.9 | 21/54 |
  | slimdeer's target as it stands | 107.0% / 89.1% | 24/54 | +7.6 | 17/54 |

  At TinyPNG's score: web photos 79.0%, camera originals 87.4%, PNGs 83.0%, icons 92.9% (all 4
  smaller). The two that aren't smaller are an illustration (100.1%) and a camera original (105.0%,
  but 1.4 points better: one quality step lower misses the score; libavif's 100 qualities map to
  libaom's 64 quantizers). Four more are within 3% (97.9–99.7%). Each image's constants above came
  from the other images only.

  At about TinyPNG's AVIF's quality the automatic output is 21–24% smaller on the whole and as good
  on the median, but image by image it is better or worse than TinyPNG's by as much as TinyPNG's own
  quality wanders (median +3.9 points on the web photos, −1.7 on the originals, −2.7 on icons). At
  slimdeer's own quality it is about TinyPNG's size (smaller on camera originals, bigger on PNGs) and
  7.6 points better.
- **Time:** the median encode work was 17 s for a web photo, 61 s for a camera original (up to 83 s
  at 12 MP), 13 s for a PNG, one thread, contended, nearly all of it in the 3 or so speed-4 encodes
  of a large photo. As the final quality is the speed-8 search's less one step on the median image,
  a cheaper final (`FINAL=margin`): encode at speed 4 one step below the speed-8 quality, step up
  if that misses the target, and bracket downward only if it beats the target by more than a step's
  worth of points (by the speed-8 files' slope). On all 41 images over 0.5 MP it found exactly the
  files the full bracketing did, in 6 encodes instead of 8 on photos (one or two at speed 4): 10 s
  for a web photo, 35 s for a camera original (up to 69 s), median. Taking the first step up
  without the downward check (`FINAL=step`) is as good on photos but up to 17% bigger on
  illustrations, cut-outs and screenshots, where speed 8's real-time-style search (round 7) lands
  2 to 10 steps above where speed 4 could. In the browser, with SIMD (round 7), a camera original
  would take about a minute.
- **Decided:** slimdeer's AVIF aims at about TinyPNG's AVIF's quality, the `tinypng` target: a
  photo's target 62.3, a PNG's PNG output's target less 9.0 (the medians on these 54 images). Its
  files are 21–24% smaller than TinyPNG's AVIF and as good on the median; at TinyPNG's quality,
  image by image, 19% smaller.

## Round 9: in the product

2026-11-02. AVIF output on the command line: `slimdeer IN -o OUT.avif` (or `--format avif`), from a
PNG or a JPEG photo, by round 8's rules (src/avif.rs; libaom in vendor/libaom). The browser build
doesn't have it yet.

- **libaom vendored** as libwebp and mozjpeg are: 3.15.2 with slimdeer.patch, the encoder only, its
  CMake's config and dispatch headers checked in (plain C, and NEON for 64-bit ARM), built by
  build.rs without threads or fused multiply-add, its math functions and qsort sent to vendor/clib.
  Its frame data is byte for byte that of aomenc built as plain C by libaom's own CMake: 38 encodes
  from the plain C build and 52 from the NEON one (6 images, speeds 4 to 8, 8- and 10-bit; the 12
  MP photo at speeds 6 and 8, where unpatched NEON differs, among them).
- **No decoder.** libaom's own copy of the frame, with its loop filters applied, is what every AV1
  decoder outputs, so slimdeer scores that: libavif skips applying the last filter
  (AV1E_SET_SKIP_POSTPROC_FILTERING), which leaves the bitstream as it is, so slimdeer doesn't. It is
  converted to RGB as Chrome converts it (libyuv's fixed-point BT.601, 10-bit chroma cut to 8 bits
  first). Against Chrome 156's own decode (chrome.mjs) of 6 files, 8- and 10-bit, opaque and
  translucent: every one of 5.8 million opaque pixels identical, every alpha value identical, and
  the reports' scores Chrome's to 0.001 on opaque images. Translucent pixels differ only by Chrome's
  canvas, which stores them premultiplied (0.01–0.06 points, the reports' lower).
- **The file.** slimdeer writes its own container: the boxes HEIF, MIAF and AVIF require (with av1C
  from each item's sequence header, colr nclx, the ICC profile's colr prof if any, and alpha as an
  auxiliary item). Without avifenc's item names and optional brand, and without the temporal
  delimiters AVIF leaves out, it takes 266 bytes for an opaque image and 416 with alpha, against
  avifenc's 275 and 430.
- **avifenc tiles automatically.** The product's first files differed from round 8's by up to
  ±19% image by image (one web photo 19% bigger at the same quality, two screenshots 14–17%
  smaller), though only 0.1% in total. avifenc 1.4.2 splits an image into as many tiles of at least
  512 × 512 as fit, up to 8, by default (as if for 8 threads, whatever `-j` says), and tune=iq's
  choices change with the tiling, not just at the tile boundaries: on that photo 8 tiles made a 16%
  smaller file at the same quantizer. Tiled the same way, slimdeer reproduces round 8 (its files
  100.0% of round 8's on the median image, 100.6% in total, the rest one quality step either side,
  as Chrome's scoring and a target of 62.3 rather than 62.1 or 62.5 move the line); but untiled, at
  the same target and the same median score, they are 0.5% smaller in total, and 3.6% on the PNGs.
  slimdeer doesn't tile.
- **Against TinyPNG's AVIF**, both scored as Chrome shows them (TinyPNG's files decoded by Chrome
  and scored by slimdeer: product.py `tiny`; median −0.1 points from round 8's avifdec scores):
  - At each image's TinyPNG score as the target, the leaderboards' comparison: 79.7% of TinyPNG's
    bytes on the median image, 80.5% in total, every target reached, smaller and no worse on 52 of
    the 54 (round 8: 81.3% / 80.9%, 52). The other two are a camera original (104.6%) and an
    illustration (100.8%). Web photos 78.7%, camera originals 87.3%, PNGs 79.1%, icons 89.9%
    (median).
  - At the automatic target (photos 62.3, PNGs their PNG target less 9): 79.3% / 75.9%, smaller on
    48, and 0.7 points better on the median (web photos +4.0, camera originals −0.7, PNGs +0.1);
    round 8's was 79.3% / 75.8%.
- **Time**, the whole compression at the automatic target, one image at a time, with NEON on a
  14-core Mac (the same 54 files as the plain C build, byte for byte): 8.8 s on the median image;
  web photos 8.3 s, camera originals 18.4 s (up to 95 s, for a photo whose speed-4 file beat the
  target by more than a step and was searched downward: 19 encodes), PNGs 5.9 s (up to 22 s). Plain
  C took about three times as long. Nearly all of it is speed 4, one thread per encode: libaom is
  built without threads, for the same bytes, so slimdeer encodes a few qualities at once instead.
- **A floor for PNGs.** Where a palette does badly, PNG output's target is low: 30 on a smooth
  gradient, which would make a poor AVIF, though TinyPNG's, at its one quantizer, isn't. A PNG's
  target is at least a photo's (62.3); round 8's PNG targets were all above it.
- **The same bytes everywhere**: tests/avif.rs pins three whole compressions (a small icon and a
  small photo-like image, both with transparency, searched over the four small settings; a 0.54 MP
  photo-like one, searched at speed 8 and finished at speed 4), and macOS arm64 (clang, NEON), Linux
  arm64 (GCC 12, NEON) and Linux x86-64 (GCC 12, plain C) reproduce them; the timing run above wrote
  the plain C build's 54 files with NEON.
- **Still to do** (round 10 did the browser): the browser (wasm32-unknown-unknown has no C library for libaom's stdio and
  `setjmp`, which vendor/clib would stand in for; round 7's SIMDe build of the NEON code; 1.1 MB
  more to load, only for AVIF); x86's SIMD on the command line (SSE2 to AVX2, chosen at run time,
  checked in round 7); a way out where AVIF loses to slimdeer's WebP, icons above all (round 3); and
  two cheap wins round 8 didn't try: monochrome AV1 for gray images, and colors under fully
  transparent pixels, which cost bits for nothing.

## Round 10: in the browser, a cheaper small-image search, and the leaderboard

2026-11-02.

- **The browser build.** AVIF is a WebAssembly build of its own (web/pkg-avif, web/pkg-avif-mt),
  everything the other one does and libaom, which the page moves its workers onto when AVIF is first
  asked for (a second threaded build beside the first hangs). libaom builds for wasm32-unknown-unknown
  with vendor/clib standing in for a C library: its stdio stubbed (messages, statistics and option
  files slimdeer never uses), and `setjmp` returning 0 with `longjmp` aborting, as libaom jumps only on
  an error that slimdeer's checks leave to running out of memory. Its SIMD is round 7's: the 171 NEON
  functions 10-bit encodes spend time in, through SIMDe on SIMD128, the hot files at -O2 and the
  rest at -Os. 1.80 MB gzipped (the threaded build 1.92 MB), against 0.62 MB without AVIF. It writes
  the command line's files byte for byte (twemoji, a cut-out, a 1.1 MP JPEG photo, the page's five
  samples). One thread of wasm SIMD is 2.8 times slower than one of NEON (a 0.38 MP icon, 27
  encodes: 51 s against 18 s), and SIMD makes wasm about 1.4 times faster than plain C (a 1.1 MP
  photo). In headless Chrome on the page, the threaded build: the 0.38 MP icon in 13 s, a 0.39 MP photo in
  17 s, the 1.1 MP photo in 28 s, a 5.2 MP screenshot in 57 s, all the command line's files. A file over 24 MP is refused in the browser,
  where encodes run at once only while their pixels fit in 24 MP: an encode takes 35–50 bytes a
  pixel, the rest about 70 (a 21.8 MP photo peaked at 2.6 GB), and wasm has 4 GB. That photo took
  2.5 minutes there, a 24 MP one 3, both the command line's files. The first
  deployment wrote other files than the command line: Vercel built it with Amazon Linux's clang 15,
  which compiles the SIMD (SIMDe's NEON on SIMD128) differently; its plain C, and LLVM 23's SIMD
  (wasi-sdk 34, now on Vercel too), write the command line's files. web/check-avif.mjs now refuses
  a build that doesn't.
- **Small images, searched near each other.** The four small settings were each searched alone, 9
  to 15 encodes each, because the first guess, a photo's, is far off on icons and renders. But the
  others land within 6 steps of 10-bit speed 4's answer (round 8's 13 small images), so it is now
  searched first and the others in rounds of three neighbouring qualities from one step below its
  answer. On the benchmark's 89 small images: the same files but for 6, which are 0.5–8% smaller
  (99.93% of the bytes in total), with 45% fewer encodes (2,219 against 4,049) and 35% less time on
  the command line. The pinned tests' files didn't change.
- **Tried: speed 6 where speed 8 misleads.** Where speed 4's first file beats the target by more than a
  step, its search goes down from speed 8's quality, which can be far above speed 4's on screenshots
  (5 MP slimdeer@2x: 75 against 49, 24 encodes, 35 s). Searching there at speed 6 first, then near its
  answer at speed 4, wrote the same files on all 105 large images tried, and slimdeer@2x in 14 s, but
  it took 22% more encodes and more CPU on most of the others, where speed 8 is only a few steps off.
  Left out.
- **The leaderboard** (leaderboard.py; the site's Research tab, the README's table), on round 8's 54
  images, each tool's files pooled into one size-quality frontier per image, every file decoded by
  Chrome and scored by slimdeer, against TinyPNG's AVIF (scored the same way):

  | Tool | Saved at TinyPNG's quality (median / total) | Reaches | Points at TinyPNG's size |
  |---|---|---|---|
  | **slimdeer** (its runs at TinyPNG's score −6 to +6, and automatic) | **22.7% / 22.1%** | 54 | +5.9 |
  | avifenc (best of 33: speed 6, qualities 30–90, 4:4:4 8- and 10-bit, 4:2:0 10-bit) | 10.7% / 16.7% | 54 | +3.5 |
  | rav1e (ravif, speed 6, best of 11 qualities) | −2.0% / 2.7% | 54 | +0.4 |

  Photos: slimdeer 22.6%, avifenc 18.8%, rav1e 2.9%; PNGs: 22.7%, 6.3%, −13.3% (median). avifenc at
  its defaults (quality 60, 4:4:4 8-bit) makes files 32% bigger than TinyPNG's on the median image,
  8.8 points better. All tools encode the photos from the upright PNG without its color profile:
  a profile in the file has Chrome show it color-managed, not as the photo's own pixels, which
  TinyPNG's, without one, are scored against (slimdeer's files from the JPEGs, which keep a profile
  that isn't sRGB's, lost 9–14 points that way on three camera originals).

## Round 11: libaom's threads

2026-11-03.

- **Two threads or twenty, the same file.** libaom writes different files with one thread and with
  several, but with row multithreading, any number from 2 writes one file: aomenc 3.15.2
  (Homebrew, NEON) on an icon, a screenshot, an 8-bit web photo and a 12 MP photo, at speeds 2, 4
  and 6, with and without screen content tools, wrote the same file at 2, 5 and 12 threads (24 of
  24). libaom hands rows out to its workers as they ask for them, and what a worker learns in one
  row doesn't carry to the next, so which worker does which row doesn't matter. The 12 MP photo at
  speed 4 and a fixed quantizer: 0.26% bigger than with one thread, in 2.7 s at 14 threads against
  17.4 s at one.
- **Built in.** libaom is now built with `CONFIG_MULTITHREAD`, and every encoder gets at least 2
  workers, as many as rayon has threads. libaom's own pthreads are replaced by its worker
  interface on rayon (vendor/libaom/src/workers.rs): a worker is a job that whichever of rayon's
  threads is free takes, or the encoder's own thread when it waits for one that nobody has
  started. Encoders running at once (a small image's settings, a large one's qualities) never wait
  for each other's threads, and in the browser, which has no threads to give libaom, the jobs run
  on rayon's Web Workers. Without rayon (the browser's single-threaded build) they run one after
  another, the first doing all the rows. On wasm32, vendor/clib's `pthread.h` gives libaom futex
  locks on wasm's atomic wait and notify, or none in the single-threaded build. A worker costs about
  0.5 MB on a 21.8 MP photo (2.62 GB at 1, 2.64 GB at 32).
- **The same bytes.** libaom's own pthreads at 2, 3, 4, 8 and 16 threads, rayon's pools of 1, 2, 4
  and 14 threads, and the jobs one after another wrote the same files on five images (an icon, a
  photo, a cut-out, a screenshot, an illustration); the threaded command line and the one without
  threads, the same files on all 109 of the benchmark's images, and on 18 of them at targets 100,
  95 and 40 (54 files more). Images too small for a second worker don't change (row
  multithreading gets at most half the superblock columns: a 72 × 72 icon has one); every
  256 × 256 render does.
  tests/avif.rs's 900 × 600 photo is re-pinned (1,463 bytes, was 1,376), and a new test writes it
  with rayon pools of 1, 2 and 5 threads; macOS arm64, Linux arm64 (GCC 12, NEON) and Linux x86-64
  (GCC 12, plain C) pass. In the browser, the single-threaded build wrote the command line's files
  (web/check-avif.mjs now pins a 256 × 256 icon too, for several workers run one after another), and
  the threaded build, in headless Chrome, the command line's files byte for byte (six images, the
  five above and a 256 × 256 render).
- **Files.** At a fixed quantizer the multithreaded encoder's files are a little bigger (0.26% on
  the 12 MP photo), and the search's answers move both ways: the 21.8 MP photo's quality 41 came out
  at 284 KB, 0.03 points short of the target, so the search took 42 (328 KB), where one thread's 41
  had reached it at 294 KB. On the leaderboard's 54 images (leaderboard.py, slimdeer's files made
  again): at TinyPNG's score, 81.1% / 81.2% of TinyPNG's size (was 79.6% / 80.4%), smaller and no
  worse on 53 (52); at the automatic target 79.8% / 76.1% (79.3% / 75.9%), 0.5 points better on the
  median (0.7); pooled, 22.6% / 21.9% saved at TinyPNG's quality (22.7% / 22.1%; photos 22.5%, PNGs
  22.7%) and 5.7 points at its size (5.9), which the README and the site now give.
- **Time**, the whole compression, on a 14-core Mac (the command line, libaom with one thread
  against threads; the threaded browser build in headless Chrome, round 10 against now):

  | Image | Command line | Browser |
  |---|---|---|
  | 0.38 MP icon (openmoji) | 4.4 s → 3.7 s | 13 s → 11 s |
  | 0.39 MP photo (kodim01) | 8.4 s → 4.3 s | 17 s → 10 s |
  | 1.1 MP web photo | 10.6 s → 3.9 s | 28 s → 11 s |
  | 5.2 MP screenshot (slimdeer@2x, 24 encodes) | 21.8 s → 14.0 s | 57 s → 40 s |
  | 21.8 MP photo | 37.9 s → 19.2 s | 91 s → 45 s |
  | 24 MP photo | 65.5 s → 17.1 s | about 3 minutes → 49 s |

  Small images gain least: their settings already ran at once, and only the scout's rounds and the
  last stragglers waited on one thread. The 24 MP browser time before is round 10's, before the
  second speed-4 encode waited to be needed. The single-threaded browser build takes about as long
  as before (a 256 × 256 icon: 9.7 s of CPU against 9.5 s).

## What it would take (after round 8)

Round 9 did the scoring, the automatic target and the search, on the command line; not the way out
to WebP, nor the browser.


Either encoder would also need:

- a decoder in slimdeer to score what browsers show (AV1 decoding is exact: rav1d, dav1d's Rust port,
  or libaom's own) and Chrome's conversion to RGB (round 4);
- the automatic target round 8 decided on: about TinyPNG's AVIF's quality, 6–9 points below
  slimdeer's JPEG and PNG output (a photo 62.3, a PNG its PNG target less 9.0);
- a way out where AVIF loses to slimdeer's own WebP, icons above all (round 3);
- round 8's search (a speed-8 search, then one or two speed-4 encodes, `FINAL=margin`; four
  settings on images up to 0.5 MP), and for camera originals in the browser, still about a minute,
  perhaps a search on a preview as PNG output does.

**libaom** is the one worth shipping: 24% under slimdeer's JPEG, 19% under TinyPNG's AVIF, and fast;
with speed 4 and a few settings per image it beat TinyPNG's AVIF on all 54 images tried. Built as
plain C it writes the same bytes everywhere checked (round 5), so it fits slimdeer's rule; with
slimdeer.patch so does its SIMD (round 7), which would make the command line 2–4 times faster and
the browser 1.4–2.2 times, the browser through SIMDe and a build step like simd-wasm.py's. The costs
are 1.1 MB more to download (loaded only for AVIF), speed 4's time on large photos in the browser
(38 s at 12 MP with SIMD), and vendoring a large C library the way libwebp and mozjpeg are:
libaom's CMake generates config and run-time CPU dispatch (RTCD) headers that would be generated
once and checked in (for wasm, the NEON choices of simd.json), slimdeer.patch would be carried, a
check like hashes.sh on every platform would guard the SIMD, and libaom reports errors with
`setjmp`/`longjmp`, which wasm32-unknown-unknown has no C library for (the WASI build here used wasm
exception handling, which current browsers support; a patch that aborts instead is the other way).
The speed-8 bug is worth reporting to libaom.

**rav1e** is cheap to ship (pure Rust, the same bytes already shown, 0.4–0.7 MB gzipped in wasm
against slimdeer's 0.6 MB today), but its files are only 13% under slimdeer's JPEG and no smaller than
TinyPNG's AVIF on camera originals, so slimdeer couldn't say it beats TinyPNG with it; and at a usable
speed it gives most of that up.

## Reproduce

```bash
cargo build --release -p slimdeer -p aviflab
cargo build --release -p aviflab --lib --target wasm32-unknown-unknown
python3 experiments/avif/compare.py tiny PHOTOS TINYAVIF
python3 experiments/avif/compare.py photos PHOTOS TINYAVIF avif.csv
python3 experiments/avif/compare.py summary avif.csv JPEG.csv WEBP.csv
python3 experiments/avif/compare.py pngs bench/images TINYPNGS avif-png.csv
python3 experiments/avif/compare.py summary-pngs avif-png.csv bench/results/wlb-main-20261028-091328/results.csv bench/results/smooth-main-20261025-142520/results.csv
PICK=4 python3 experiments/avif/compare.py tiny-pngs bench/images TINYPNGS
PICK=4 python3 experiments/avif/compare.py pngs bench/images TINYPNGS avif-png.csv aom-s6-10bit aom-s4-10bit aom-s6-10bit-screen aom-s6-420-10bit aom-lossless
python3 experiments/avif/compare.py summary-tiny-pngs avif-png.csv bench/results/smooth-main-20261025-142520/results.csv
python3 experiments/avif/compare.py speed PHOTO...
python3 experiments/avif/auto.py refs PHOTOS bench/images TINYAVIF TINYPNGS refs.csv
python3 experiments/avif/auto.py fit refs.csv
[FINAL=step|margin] python3 experiments/avif/auto.py run PHOTOS bench/images TINYAVIF TINYPNGS refs.csv auto-TARGET.csv at-tinypng|tinypng|slimdeer
python3 experiments/avif/auto.py summary auto-TARGET.csv
python3 experiments/avif/product.py tiny PHOTOS bench/images TINYAVIF TINYPNGS tiny.csv
python3 experiments/avif/product.py run PHOTOS bench/images auto-tinypng-margin.csv product.csv
python3 experiments/avif/product.py run PHOTOS bench/images auto-tinypng-margin.csv at-tinypng.csv tiny.csv
python3 experiments/avif/product.py summary product.csv
python3 experiments/avif/leaderboard.py sweep PHOTOS bench/images tiny.csv leaderboard.csv
python3 experiments/avif/leaderboard.py summary tiny.csv leaderboard.csv
node experiments/avif/chrome.mjs FILE.avif...
experiments/avif/libaom/hashes.sh INPUTS AOMENC                       # once per build; diff the output
```

libaom/ builds and checks libaom 3.15.2 (its release tarball): natively with `cmake LIBAOM
-DAOM_TARGET_CPU=generic -DCONFIG_MULTITHREAD=0 -DCONFIG_RUNTIME_CPU_DETECT=0 -DCONFIG_WEBM_IO=0
-DCMAKE_C_FLAGS=-ffp-contract=off` and `make aomenc`; for wasm the same with
`-DCMAKE_TOOLCHAIN_FILE=experiments/avif/libaom/wasi.cmake` (Homebrew's llvm, lld, wasi-libc and
wasi-runtimes), run as `node experiments/avif/libaom/wasi.mjs aomenc ...`; on Linux with linux.sh in
Docker. INPUTS holds Y4M files made with ffmpeg (`-pix_fmt yuv444p10le -strict -1`, and yuv444p,
yuv420p10le) in in/, in8/ and in420/.

Round 7's builds are of libaom with slimdeer.patch applied (`patch -p1`). NEON: `cmake LIBAOM
-DAOM_TARGET_CPU=arm64 -DCONFIG_RUNTIME_CPU_DETECT=0 -DCONFIG_MULTITHREAD=0 -DCONFIG_WEBM_IO=0
-DCONFIG_AV1_DECODER=0 -DCONFIG_EXCLUDE_SIMD_MISMATCH=1 -DENABLE_NEON_DOTPROD=0 -DENABLE_NEON_I8MM=0
-DENABLE_SVE=0 -DENABLE_SVE2=0 -DCMAKE_EXPORT_COMPILE_COMMANDS=ON -DCMAKE_C_FLAGS=-ffp-contract=off`
and `make aomenc`; that tree is NATIVE for `python3 experiments/avif/libaom/simd-wasm.py NATIVE OUT`
(SIMDe from `brew install simde`, wasm-opt from binaryen), whose OUT/aomenc.opt.wasm is the SIMD wasm
build. simd.json and files.json came from `simd-wasm.py NATIVE OUT --all-simd --all-o3`, `profile.sh
INPUTS OUT/aomenc.wasm PROFILES` and `hot.py OUT DIR PROFILES/*.cpuprofile` (profiles sample, so a
rerun moves a few rarely run functions in or out). x86-64: linux-simd.sh in Docker, then
qemu-avx2.sh; forcec.py and bisect.py find a function that differs.

PHOTOS is experiments/jpeg's (`web/` and `orig/`), JPEG.csv and WEBP.csv its compare.py results.
avifenc and avifdec are libavif 1.4.2's (`brew install libavif`); the wasm build needs rustup's
cargo, as web/build.sh says.
