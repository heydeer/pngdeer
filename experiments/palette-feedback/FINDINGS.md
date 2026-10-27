# Where TinyPNG wins on UI screenshots, and how to close the gap with libimagequant

2026-10-23, based on the 109 public images in `bench/` (15 of them website screenshots). Scores are SSIMULACRA2; sizes are bytes after oxipng. "Size ratio at equal quality" places TinyPNG's result on the open-source method's size–quality curve and interpolates: the bytes the open-source method needs to reach the same score ÷ TinyPNG's bytes.

## Conclusions

1. **TinyPNG uses no special format**: its UI screenshots are also palette PNGs with ≤256 colors. The difference is in how the palette is allocated.
2. **libimagequant allocates the palette by "pixel count × mean squared error"**. In UI screenshots, large areas of similar colors (fine-grain noise on dark backgrounds, light gray backgrounds) cover most of the pixels, so they take most of the palette slots. Colors with few pixels that still stand out (accent-colored icons, text colors) get no slots. Result:
   - `linear@2x`: at 64 colors the blue icons turn entirely gray and the yellow icons shift in hue; compared with 96 colors, the score drops from 91.2 to 81.4. TinyPNG keeps these colors with only 39 colors (score 88.9).
   - `react-dev@2x`: at 14 colors the gray text in the search box is not in the palette and can only be "assembled" by dithering, so the text edges are covered in dotted noise. TinyPNG uses 30 colors and the text is clean.
3. **pngquant's `--quality` is also based on mean squared error**. On UI it picks too few colors (`linear@2x` at q95 has only 22 colors), and it jumps between levels (q98 is 80.3 / 536KB, q100 jumps straight to 93.8 / 852KB), with no points in between.
4. **The fix uses only libimagequant's public API** and needs no changes to the library:
   - quantize once and compute each pixel's error;
   - use `set_importance_map` to raise the weight of pixels with large errors (importance = 32 + 8 × max channel error, capped at 255), then quantize again;
   - use `set_max_colors` to search the color count directly, instead of `--quality`.

## Data

Compared with TinyPNG (for each image, the curve is taken over 9 color counts × 3 dither levels):

| Image group | Method | Reaches TinyPNG quality | Size ratio at equal quality (median) | Size ratio at equal quality (total) |
|---|---|---|---|---|
| 15 UI screenshots | Color count only | 11/15 | 100.1% | 105.6% |
| 15 UI screenshots | **+ two-pass quantization with error feedback** | **15/15** | **94.8%** | **95.9%** |
| 12 sampled from other categories | Color count only | 8/12 | 96.0% | 94.2% |
| 12 sampled from other categories | **+ two-pass quantization with error feedback** | 8/12 | **86.5%** | **85.4%** |

For comparison, pngquant in the benchmark, with the best level picked afterwards for each image, gets 101.6% (median) / 102.5% (total) on UI, and can't reach on 3 images.

The 4 hardest images:

| Image | Color count only | Error feedback (gain 8) |
|---|---|---|
| `ui/linear@2x.png` | 148% | 116% |
| `ui/react-dev@2x.png` | 124% | 87% |
| `ui/stripe@2x.png` | can't reach | 93% |
| `ui/tailwindcss@2x.png` | can't reach | 89% |

Gains 4 and 8 are about the same; 24 is too much (some images get worse). Making frequent colors fixed colors (`add_fixed_color`) is worse: `linear@2x` has about 100 colors that each cover ≥0.05% of pixels (gradient steps on the dark background), and putting them all into the palette leaves even less room for accent colors.

## Real results after wiring into bench (live search at compression time)

`bench.py`'s `liqT` method: for each image and each dither strength, binary-search liqx's color count on a log scale (two-pass error feedback, gain 8) and find the smallest file that reaches the target score. All 109 images, with targets 70/75/80/85/90/95 joined into a curve:

| Method | Size ratio at equal quality (median) | Size ratio at equal quality (total) | Can't reach | Score difference at equal size (median) |
|---|---|---|---|---|
| auto (pngquant live search) | 93.2% | 95.3% | 28 | +0.4 |
| **liq (liqx live search)** | **89.2%** | **88.2%** | **18** | **+1.0** |
| Reference: best of all pq levels, picked afterwards | 94.5% | 94.8% | 22 | +0.5 |

- Of the 91 comparable images, liq is smaller than TinyPNG on 80. By category (size ratio at equal quality, median / total): 3D 85.4% / 86.5%, photos 85.8% / 85.9%, photos with transparency 82.2% / 83.0%, website screenshots 93.7% / 95.7% (all 15 can reach), illustrations 93.6% / 95.0%, icons 92.2% / 93.8%.
- Fixed targets: `liq90` is 93.1% (median) / 96.9% (total) of TinyPNG's size, with a median score 0.6 higher; `liq85` is 83.3% / 91.7%, with a score 1.5 lower.
- The gaps on the 18 images that still can't reach are all small (most < 1 point). 6 of them are twemoji icons, because the highest target was only 95 while TinyPNG scores 95–100 on these images; the rest are 3D and illustrations at the high-quality end.
- The biggest losses are `react-dev@2x` (117%), `reinel-compass-rose` (117%) and `linear@2x` (114%). For `react-dev` the main cause is that the targets are too far apart: liq85 is 15 colors, 86.5 / 64KB; liq90 needs 123 colors, 90.1 / 200KB; TinyPNG's 88.7 falls in between, and interpolation overestimates. In the grid experiment, 32 colors already give 88.8 / 98KB.

Time (`-j 1`, one image at a time, `bench/results/timing-j1`):

| Category | auto85 | liq85 | TinyPNG (API round trip) |
|---|---|---|---|
| All (median / mean) | 1.82 / 2.75 s | **1.49 / 2.36 s** | 3.42 / 3.54 s |
| 3D / icons / photos | 0.5–1.1 s | 0.5–0.8 s | 2.0–3.9 s |
| Photos with transparency | 2.2 s | 2.2 s | 6.3 s |
| Illustrations | 5.1 s | 4.0 s | 3.7 s |
| Website screenshots | 4.1 s | 3.6 s | 2.7 s |

Each liqx quantization takes only 0.1–0.2 s (5MP), so two-pass quantization adds almost no time. Most of the time goes to SSIMULACRA2, which runs for every candidate (5MP: about 0.6 s). liq needs a median of 16 evaluations per target, fewer than auto's 19.

## Round 2: tuning, small-image rules

Tried several changes on a 24-image test subset (biased toward hard images). Size ratio at equal quality (median / total):

| Variant | Size ratio | liq85 mean time | Evaluations |
|---|---|---|---|
| Round 1 default | 93.1% / 89.1% | 4.67 s | 15.5 |
| libimagequant speed 1 | 89.5% / 86.5% | 4.65 s | 15.5 |
| Only two dither levels: 0.5 and none | 94.2% / 89.0% | 3.12 s | 11 |
| **Both together (new default)** | **88.6% / 86.3%** | **3.40 s** | **11** |
| Plus down-weighting grainy areas | 95.0% / 82.9% | 3.61 s | 11 |

Grain down-weighting improves transparent photos and website screenshots, but 3D gets clearly worse (83.5% → 98.4%), so it was not adopted. Error feedback gains 4 / 12 give no consistent improvement. On a 5MP image, zopfli spends 17 more seconds to save only 0.9%, so it was not used either.

In the blind test, a 72px light bulb icon (`twemoji-1f4a1`) was seen to be worse than TinyPNG: we merged several levels of semi-transparent edge and lost the transition colors of the stripes on the bulb base, and SSIMULACRA2 can barely measure this on such a small image (95.1 vs 95.4). Two rules were added:

- **Upscale small images before scoring**: for images whose long side is under 256, the original and all outputs (including TinyPNG's) are first upscaled with nearest neighbor to at least 256, then scored.
- **Prefer lossless for few-color images**: if the original has at most 256 colors and the lossless palette version is at most 1.15x the chosen result, use the lossless version. This triggered on all 21 few-color images.

All 109 images (liq targets 70–95 joined into a curve, compared with TinyPNG):

| | Size ratio at equal quality (median / total) | Smaller than TinyPNG | Can't reach | Score difference at equal size |
|---|---|---|---|---|
| Round 1 | 89.2% / 88.2% | 80 / 109 | 18 | +1.0 |
| **Round 2** | **88.0% / 85.7%** | **88 / 109** | **13** | **+1.6** |

- All 10 twemoji images now reach, at 94.7%–99.8% of TinyPNG's size at equal quality. Light bulb: liq90 is 32 colors, 595 B, score 90.6; TinyPNG is 615 B, 90.0 (scored at 4x upscale).
- By category (median): 3D 83.2%, photos 84.5%, transparent photos 80.4%, illustrations 92.0%, website screenshots 94.4%, icons 94.3% (icon scoring changed, so not directly comparable with round 1).
- The 13 that still can't reach: 6 3D images, 7 illustrations, most within 1 point; the largest gap is `fluent-red-apple` (3.5 points).

Time (`-j 1`, `bench/results/timing-j1-v2`), median: liq85 **1.24 s** (round 1: 1.49 s), TinyPNG API round trip 3.42 s. By category, liq85 is faster than TinyPNG everywhere: website screenshots 2.62 vs 2.70 s, illustrations 2.76 vs 3.73 s, the rest 0.5–2 s vs 2–6 s. Separately, switching the final oxipng step from `-o max` to `-o 2` adds only 0.35% to size and cuts time to about 30%; this is not yet used in bench.

## Cross-check with an independent metric (butteraugli)

liq's search itself optimizes SSIMULACRA2, so comparing with TinyPNG on SSIMULACRA2 is biased in our favor. So the round 2 outputs for all 109 images were rescored with the butteraugli that ships with jpeg-xl (lower is better; transparent images are composited on a dark and a light background, scored on each, and the worse is taken; icons are scored at original size, not upscaled):

| Metric | Method | Size ratio at equal quality (median / total) | Smaller than TinyPNG | Can't reach |
|---|---|---|---|---|
| SSIMULACRA2 (reference) | liq | 88.0% / 85.7% | 88 / 109 | 13 |
| butteraugli 3-norm | **liq** | **88.3% / 81.7%** | 83 / 109 | 12 |
| butteraugli 3-norm | auto | 94.9% / 92.5% | 62 / 109 | 29 |
| butteraugli max distance | **liq** | **84.9% / 76.3%** | 84 / 109 | 13 |
| butteraugli max distance | auto | 94.3% / 87.1% | 52 / 109 | 39 |

- Paired by size (as in the blind test, take the liq output closest in size to TinyPNG's): liq has the smaller butteraugli distance on **81** images, TinyPNG on 27, tied on 1. 3D 19 : 3, photos 11 : 1, transparent photos 15 : 1, website screenshots 12 : 3, illustrations 14 : 10, icons 10 : 9 (icons are roughly even).
- Out of 107 pairs, the two metrics agree on "which is better" in 99 (93%).
- Conclusion: the advantage does not come from gaming SSIMULACRA2. It holds with an independent metric, and is even larger on total size. But both are algorithmic metrics; a human blind test and a fresh set of images not used for tuning are still missing.

## Round 3: neighbor smoothing (`liqx --rdo`)

Idea: when two palette colors have about the same error, pick the one that matches the left or upper neighbor, so the image data compresses better. Implemented as liqx `--rdo DELTA`: after remapping, check each pixel; if the current color differs from both the left and upper neighbors, and one neighbor's color has at most DELTA more error (RGBA with premultiplied alpha, max channel difference), switch to it.

- **Turning it on globally hurts** (24-image test subset): with DELTA 1 / 2 / 4, size ratio at equal quality (total) goes from 86.2% to 102.9% / 100.1% / 112.0%, and the images that can't reach go from 4 to 14 / 16 / 19. Smoothing wipes the dither patterns back into color bands, and even all 256 colors cannot reach high quality.
- **As one variant in the search** (at each color count, try 0.5 dither, no dither, and no dither + smoothing, and take the smallest file that meets the target): the smoothing variant is picked about a third of the time. It saves only 0.5% on the test subset, a bit more on the full set:

| Metric | Method | Size ratio at equal quality (median / total) | Can't reach |
|---|---|---|---|
| SSIMULACRA2 | No smoothing | 88.0% / 85.7% | 13 |
| SSIMULACRA2 | + smoothing variant (DELTA 2) | 87.1% / 84.9% | 14 |
| butteraugli 3-norm | No smoothing | 88.3% / 81.7% | 12 |
| butteraugli 3-norm | + smoothing variant (DELTA 2) | 87.8% / 78.5% | 13 |

The gains are mainly in illustrations, transparent photos and website screenshots, and much smaller than the expected 5–15%: the quality lost to smoothing has to be bought back with more colors, and the two mostly cancel out. The median number of evaluations goes from 11 to 13. Now the default variants for liq (`--liq-variants 50,0,0r2`). Real rate–distortion optimization would put the compression cost into dithering and remapping themselves, not smooth afterwards.

## Round 4: search moves into liqx, browser prototype

- **In-process search**: the whole liq search moved into liqx (`liqx --target T`). The original is decoded once, the reference computed once, each color count quantized once and remapped per variant, and candidates are scored in memory with fast-ssim2. fast-ssim2 and libjxl's ssimulacra2, compared on 164 samples, differ by 0.000 on average and 0.22 at most; its strip mode is off by 2.4 points, so it is not used. The final oxipng step changed to `-o 2`.
- **Single-thread timing** (`bench/results/timing-j1-v4`): liq85 median **0.60 s**, mean 0.90 s (previously, with smoothing, 1.63 / 2.46 s); TinyPNG API round trip 3.42 s; website screenshots 1.41 vs 2.70 s, illustrations 1.17 vs 3.73 s. Faster than TinyPNG in every category.
- **Size** is mostly unchanged: size ratio at equal quality under SSIMULACRA2 is 89.0% / 85.8% (median / total), under butteraugli 89.3% / 79.4%. The median is about 1 percentage point worse than the previous round; that is the cost of `-o 2`.
- **Browser prototype** (`web/`): liqx split into a library, PNG I/O switched to pure Rust (png crate for decoding + our own encoder with miniz compression), compiled to WASM (554 KB). Feature-complete, with results identical to native, but:
  - Slow: single-threaded, and fast-ssim2 has no SIMD on WASM. Sample images take 1.4–18.6 s, a 5MP screenshot 74.6 s, about 5–20x native.
  - Large: the pure Rust encoder is larger than oxipng `-o 2` (median +7.8%, total +11.3%), so the browser version is only at 92.5% / 97.3% of TinyPNG. The gap comes from libdeflate-level compression and palette ordering. (Round 5 compiled oxipng into WASM, and this gap is gone.)

## Round 5: speeding up the browser version

Goal: a 5MP screenshot in under 10 s in the browser (previously 63–82 s). Two parts: make scoring itself faster, and add a coarse search on a downscaled image.

- **Scoring is more than 3x faster, with unchanged scores**. Profiling the WASM showed 64% of the time in `fmaf`: WASM has no scalar fused multiply-add instruction, so every `mul_add` in fast-ssim2's scalar path becomes a bit-by-bit software emulation in compiler-builtins. Replacing it with a plain multiply and add does not work: the recursive Gaussian filter amplifies the rounding error, and scores are off by up to 2.4. Computing in f64 instead (an f32 product is exact in f64) matches native to 4 decimal places. Two other changes: the vertical blur used to walk one column at a time (each step jumps a whole row, so the cache is useless); it now walks row by row in blocks of 64 columns. The XYB conversion reuses the result when a pixel equals the previous one (screenshots have large flat areas, palette images even more so). All three patches are in `web/wasm/vendor/fast-ssim2` and only the browser version uses them. One 5MP scoring pass: 6.5 s → 2.0 s; full search on the original: 74.6 s → about 30 s. The WASM SIMD build is still slower (it also has to emulate fused multiply-add exactly), so it is not enabled.
- **Coarse search on a downscaled image, confirm on the original** (`--preview-pixels 350000`). First run a full search on a downscaled image of at most 350k pixels (weighted-average downscaling), then:
  - Scores on the downscaled image are systematically high (median 4.5 points higher, up to 18 higher), and the bias varies with color count (react-dev: at 104 / 62 / 18 colors it is +0.5 / −0.8 / +3.4), so a single confirmation is not enough: with one confirmation, of 76 images, 29 fell below 85. Now the original is confirmed with a budget of 3 scorings: after the first, correct the target with the measured bias and find a new color count on the downscaled image (cheap); once there are original-image scores on both sides, interpolate on log(color count). The coarse-search target gets a 2-point margin; on average 2.9 original-image scorings.
  - Dithering costs much more on the original than on the downscaled image, so the downscaled image often picks the wrong variant (40/76 images differ from the full search; react-dev picked d50 with 15 colors, which is larger than d0r2 with 30 colors by 48%). Now variants are ranked by a rough encoded size on the original (no filtering, deflate level 1) and then confirmed; with budget left, the second-ranked is also tried.
  - Images whose score can't reach the target (even 256 colors is not enough) pick the variant on the original with the old rule, and the quality is exactly the same as the full search (32 images).
  - Tried but worse: point sampling instead of average downscaling (larger bias), a mosaic of small tiles from the original (the palette sees only local colors; worst case off by 137 points), a 250k-pixel preview (not faster).
- **Results** (109 images, target 85, same pipeline run natively): 76 images use the downscaled image, and none falls below 85; total size is 1.5% larger than a full search on the original, median unchanged. In the browser (Chrome, one at a time), 8 screenshots at 5MP take **5.3–8.9 s**; dropping all 8 in at once (4 workers) takes 19 s total.
- **oxipng in the browser**: with the `freestanding` feature, oxipng compiles to WASM (libdeflate built with Homebrew's clang), and its output is byte-identical to native `oxipng -o 2`. It now writes the final file; our own encoder is only used to rank candidates (the last miniz level-10 recompression was removed). Browser version total size over 109 images: **−10.1%** (median −7.1%), exactly the same as "old output run through native oxipng"; 3 images picked slightly different color counts because candidates are now ranked by level-4 size, a total difference of 326 bytes. In the few-color rule, if our encoder's lossless version, even shrunk by 25%, would not fit within the 15% margin, it is no longer passed through oxipng (measured: oxipng shrinks by at most 14%). 5MP screenshots in Chrome: **5.9–9.3 s**, of which oxipng is 0.5–1.2 s; 8 at once: 20 s. WASM went from 627 KB to 872 KB.
- **Already-compressed images** (a user dropped in a 256-color photo already compressed by something like TinyPNG, 1254², 590 KB): the web page did only lossless optimization (saving 12 bytes), while a full search on the original, at target 80, reaches 333 KB. Two causes: average downscaling smooths away the palette image's dithering and creates colors the original does not have, so on the downscaled image smoothing (d0r2) can't reach the target and is excluded, while on the original it is exactly the best (original palette + smoothing: 84.4, 333 KB; without smoothing it takes 192 colors to reach 82.8, 495 KB). Also, the original already has 256 colors, so at 256 colors it is reproduced exactly (score 100), and this jump throws off the later interpolation. Changes: when the image has at most 256 colors, the downscaled image now takes the center pixel of each block (keeping the original colors and grain); when confirmation on the original misses the target, the next step goes up to at most 3x the color count (the bias measured at a large gap overestimates the bias higher up; vercel@2x used to jump straight to 256 colors in one step and finally stopped at 77 colors; now 32 colors, versus 24 with the full search).
  - That photo: at target 80, 590 KB → **333 KB** (score 84.4); TinyPNG gives 498 KB, 82.5 on the same image. At target 85 the original file is still kept (the full search does the same).
  - Test set: the 126 TinyPNG outputs cached by bench (over 350k pixels) used as "already compressed" inputs, compared with a full search on the original: target 85 +4.1% → +1.9%, target 80 +4.5% → +1.6%; the 109 originals +1.6% → +1.3%. None falls below the target, and 5MP time is unchanged.

## Round 6: automatic quality (no level to pick)

- **TinyPNG has no fixed quality**: over the 109 images its scores range from 65.8 to 100 (median 87, photos about 81, icons about 93). It also does not "stop once 1 more point costs more than some number of bytes": on our curves, the slope where it stops ranges from 5 to 51 points per doubling. The closest match is **pngquant using all 256 colors** (`--quality 0-100`): median score difference only 1.0, spread 1.0. Also, when 256 colors is already near lossless (≥95), it uses fewer colors and stops at 89–93.
- **Rule** (`liqx --target auto`): reference score = the score of plain libimagequant at 256 colors (speed 4, Floyd–Steinberg dithering, no error feedback), but not lower than our own best 256-color result − 4 (on a few line drawings with transparency, plain quantization is more than 10 points worse than both TinyPNG and us). When the original has at most 256 colors, plain quantization reproduces it exactly, so the best non-copy result among our 256-color ones is used instead (the 256-color photo from round 5: smoothed version, 84.4). Target = min(reference score, 90). 90 is the line where 1:1 shows no visible difference; bytes spent above it are not visible.
  - Parameters tried: reference score minus 0 / 0.5 / 1, cap 90 / 91 / 92 / 95. Using "our best at 256 colors − 1" as the reference is worse (on UI our ceiling is much higher than plain quantization, so the target is set too high).
- **Results** (109 images, vs TinyPNG): full search on the original gives size 90.1%, median score 0.5 higher, and 66 images that are smaller than TinyPNG and not worse; the browser pipeline (coarse search on the downscaled image) gives 90.9%, median 0.6 higher. "Visibly worse" (below 90 and more than 1 point below TinyPNG) is only 5–6 images, mostly the ones where TinyPNG's 256 colors are already better than ours. For comparison: fixed target 85 gives 87.5% but median 1.4 lower and 55 visibly worse; fixed target 90 gives 93.9%, 1.0 higher, 4 images.
- **Matching changes in the browser pipeline**: the target is set by one plain-quantization scoring on the original (a target estimated on the downscaled image is off by ±1.5 points); when the downscaled image can't reach with the margin, retry without the margin first; if the top variant can't reach even at 256 colors, try the next one, instead of treating all variants as 256 colors; when out of confirmations, take the highest-scoring result.
- **Not yet solved**: 5MP screenshots take 11–19 s single-threaded in Node (after round 7's multithreading, 4.8–8.4 s in Chrome); on images like `react-dev@2x` and `linear@2x`, where the last few points are very expensive, TinyPNG uses 30–40 colors and stops around 89, while we go to the cap of 90, at 1.8 / 1.2x its size.

## Round 7: multithreaded WASM

- **How**: `web/wasm` gets a `threads` feature (wasm-bindgen-rayon) and a separate build, `web/pkg-mt`. It needs nightly Rust (`-Z build-std` to rebuild the standard library with atomics), `-matomics -mbulk-memory` for the C code too, and `--shared-memory --import-memory --max-memory` plus the TLS exports passed by hand (newer Rust no longer passes them automatically; without them the memory is not shared and `postMessage` fails with `Memory could not be cloned`). fast-ssim2's 9 blurs per scale are already independent tasks run in parallel with rayon, libimagequant and oxipng also have rayon paths, and the output is byte-identical to single-threaded.
- **Speed** (automatic quality, 8 screenshots at 5MP, Chrome 156, one at a time, 14 threads): **4.8–8.4 s**, vs 10.8–19.1 s single-threaded. Scoring is 2.6x faster; quantization, remapping and our own code (error feedback, smoothing, encoding) are still mostly single-threaded and are most of what remains.
- **Memory**: 5MP peaks at 1.3 GB (mostly fast-ssim2's reference pyramid and the temporary buffers for each comparison, about 800 MB), 12MP 2.7 GB, 15MP 3.4 GB, 20MP exceeds wasm32's 4 GB. Also limited the quantized images cached by the search (about 95 MB each) to the latest 3; the peak dropped only 5%, results unchanged. In the multithreaded build, a thread running out of memory hangs the whole compression, so images above 12MP go to the single-threaded build (which reports the error normally), and above 16MP are rejected.
- **Pitfalls**: a second multithreaded instance in one page hangs, so only one worker is used in multithreaded mode (a batch of 8 screenshots at 5MP takes 48 s total, slightly slower than four single-threaded workers in parallel, but a single image is more than twice as fast). In an embedded browser used during development, reloading the page while a multithreaded instance is in use hangs all later WebAssembly in the same process (even the single-threaded build); real Chrome (headless, separate profile) is fine after three reloads in a row, so the problem is that browser's. The page now runs a self-check: the multithreaded worker first compresses a small image, and falls back to single-threaded if that does not finish within 5 s.

## Round 8: the high-quality end (the 256-color ceiling)

- **Problem**: on 14 images (6 3D, 8 illustrations), TinyPNG's quality is higher than the best we can reach at 256 colors, by 0.1–3.5 points; the 5–6 "visibly worse" images under automatic quality are almost all among them.
- **Causes** (on these 14 images, tried every combination of error feedback strength × dither strength, all scored at 256 colors):
  - Our variants have no full dithering (only 50% and 0%). On 3D / illustrations with many smooth gradients, full dithering at 256 colors often scores highest (mahuri: 78.4 vs 50% at 76.3).
  - Error feedback gives palette slots to rare colors. This is key for UI, but on smooth images at 256 colors it loses: the plain palette without error feedback scores 86.6 vs 84.5 on renal-corpuscle, and 85.0 vs 83.8 on digestive-system.
  - Taking the best combination per image, of the 14, 8 can catch up with TinyPNG (previously 0); of the rest, only red-apple is far behind (84.5 vs 85.7). Its palette structure is the same as TinyPNG's (255 colors, 37 semi-transparent entries); only the mean error on opaque pixels is 1.64 vs 1.59, a gap in palette optimization itself.
- **Fix** (kept out of the regular search to avoid slowing it down): two extra candidates at 256 colors: our palette with full dithering, and the plain palette (libimagequant speed 4, i.e. the pngquant default, no error feedback) with full dithering. The plain palette is already computed for the automatic-quality reference score, so it costs no extra time. These two candidates are considered only when the regular variants at 256 colors are still more than 0.5 points short of the target, and only when worth it: for each 1 point more (counted only up to the target), at most 15% more bytes. Without this rule, tinypng@2x would pay for 1 point with 30% more bytes (larger than TinyPNG), and stripe@2x would spend 50% more in the browser.
- **Results** (automatic quality, vs TinyPNG): full search on the original, size 90.8% (previously 90.1%), median score +0.7 (previously +0.5), visibly worse 3 (previously 5); browser 91.6% (previously 90.9%), +0.7 (previously +0.6), 4 (previously 6). Of the 14 hard images, 3 catch up with TinyPNG, and the median gap shrinks from −0.9 to −0.3/−0.4. 5MP screenshots in the browser: 4.8–9.6 s (tinypng@2x needs one more scoring on the original).
- **What else could be done**: the remaining gap is in palette optimization itself (like red-apple): run a few more rounds of k-means over the whole image in a perceptual color space, or fine-tune the palette directly against SSIMULACRA2.

## Round 9: palette refinement

Tool: `tools/liqx/examples/palette_lab.rs` (deleted together with libimagequant in round 13), which compares the scores of different palettes and mapping methods at 256 colors.

- **Is the gap in the palette or the mapping?** Take the palette from TinyPNG's output and remap the original with libimagequant's dithering: on most of the 14 hard images this scores higher than TinyPNG's own output (teddy-bear 88.8 vs 87.8, mahuri 78.6 vs 77.7, potted-plant 89.1 vs 88.7), and also higher than all our palettes. So on most images TinyPNG wins on the palette, and our mapping is actually the better one. The exception is red-apple: TinyPNG's palette with our mapping gets only 84.6, about the same as our own; its 85.7 comes from the mapping (85% of the opaque pixels in its output are just the nearest color, with only light dithering).
- **Our own mapping**: nearest color, Floyd–Steinberg at different strengths (sRGB / linear light), Bayer ordered dithering. FS at 75% strength beats libimagequant by 0.6 on red-apple and 1.2 on mouse-mechanism, but the median is 1.0–1.4 points lower (libimagequant has an edge map, an error cap and k-means during mapping), so it was not adopted.
- **What makes TinyPNG's palette better**: in OKLab, the mean distance from each pixel to the nearest palette color is the same as ours, but the 99th percentile is smaller (teddy 1.71 vs 1.78, digestive 5.25 vs 5.70): it takes care of the few colors that are hardest to reproduce. Error feedback (gain 8) takes care of them too aggressively; the plain palette does not take care of them at all.
- **Refinement**: run a few rounds of k-means in OKLab over the opaque colors of the whole image, weighting each color by pixel count × distance^(p−2) (p=2 is plain k-means; larger p leans more toward hard-to-reproduce colors), leave the semi-transparent entries alone, then map with libimagequant using this fixed palette. p=3 with 5 rounds works best: mahuri 79.5 (previous best 78.1, TinyPNG 77.7), red-apple 85.3 (84.5 / 85.7), teddy-bear 88.8 (87.8 / 87.8). But no single parameter set is good everywhere (p=4 drops 29 points on a map with a flat color scheme), so it became a third 256-color candidate, chosen by score.
- **Details of wiring it into the search**: candidates are chosen by "are the extra bytes worth it", and among candidates that meet the target, the highest score wins (previously the smallest bytes, so the refined version could never be picked); the "smallest file that meets the target" step must not override the result picked at the top; the reference score's safety line is tightened from 4 points below our best to 3 points (so stripe@2x reaches 79.3, above TinyPNG, at 3% more bytes); when the plain candidate already exceeds the maximum allowed size, the full-dither and refined candidates are skipped (tinypng@2x in the browser goes from 11.5 s back to 8.6 s, same result). The refinement's nearest-color lookup prunes using a sort by lightness: same result, several times faster.
- **Results** (automatic quality, 109 images, vs TinyPNG): full search size 91.0%, median score +0.72, visibly worse 2 (round 8: 3, round 7: 5); browser 91.6%, +0.72, 3 (previously 4 / 6). Mean gap on the 14 hard images: −1.27 → −0.80 → **−0.69** (including 5 images where we stop at the cap of 90 and TinyPNG is at 91–93). 5MP screenshots in the browser: 4.8–8.6 s.
- **Remaining**: nueva-esparta (the plain palette used for the reference score is 3 points worse than both us and TinyPNG on this image, so the automatic target is set too low), potted-plant (a candidate can reach, but costs more than 16% more bytes than the smaller file, not worth it), red-apple 0.4 behind (mapping).

## Round 10: holdout validation

The parameters of the first nine rounds were all tuned on the 109 images in `bench/images`. To confirm they do not only work for this set, 71 more images were collected from the same kinds of sources, none of them identical (`bench/images-holdout`: other Twemoji / OpenMoji emoji, other Fluent 3D emoji, the remaining 12 Kodak images, the last 15 of the Commons featured SVGs, 12 Commons transparent cutouts, screenshots of 10 websites not tested before). Without changing a single parameter, TinyPNG and our method were rerun (`bench/results/holdout-20261024-214649`).

| vs TinyPNG | 109 images (tuning) | 71 images (holdout) |
|---|---|---|
| Size at equal SSIMULACRA2 quality (median / total) | 89.0% / 85.8% | **89.1% / 86.6%** |
| Can't reach TinyPNG quality | 14 (13%) | 6 (8%) |
| Score difference at equal size (median / P10) | +1.5 / −0.7 | +1.1 / −0.1 |
| Size at equal butteraugli quality (median / total) | 89.3% / 80.9% | **90.6% / 83.1%** |
| Automatic quality, full search: size / median score difference | 91.0% / +0.72 | **87.9% / +0.15** |
| Automatic quality, browser: size / median score difference | 91.6% / +0.72 | **90.4% / +0.26** |
| Automatic quality: visibly worse (below 90 and more than 1 point lower) | 2 / 3 | 2 / 3 |
| Automatic quality: butteraugli distance / TinyPNG (median, lower is better) | | 0.96 (browser 0.94) |

- The size advantage at equal quality carries over almost unchanged, on both metrics, and every category is smaller than TinyPNG (equal SSIMULACRA2: 80–98%).
- On the holdout set, automatic quality saves more size and its quality lead shrinks (median +0.15 to +0.26), but stays positive. The largest score differences are icons: we stop at the cap of 90, TinyPNG reaches 93–94, invisible at 1:1. The one that is really much worse is a transparent cutout of a sword (83.8 vs 86.1, with a file a third smaller).
- Conclusion: the parameters are not overfit to the tuning set; "10–15% smaller than TinyPNG at equal quality" holds on unseen images too.

## Round 11: comparison with other open-source quantizers

`bench.py` gained `<tool>N` / `<tool>N-d0` methods: ImageMagick (`-colors`, PNG8), FFmpeg (palettegen / paletteuse), Pillow (median cut; fast octree when there is transparency), exoquant (k-means), quantette (Wu + k-means, Oklab), NeuQuant (color_quant), quantizr (imgproxy); the last four are Rust libraries, called through `tools/quantlab`. Each tool runs 16 / 32 / 64 / 128 / 256 colors × dither / no dither, pngquant runs 5 levels × dither / no dither, and all go through the same `oxipng -o 2` (`bench/results/tools-main-20261024-222124`, `bench/results/tools-holdout-20261024-222124`).

These tools have no "target quality", so each tool's 10 outputs are merged into one size–quality frontier (equivalent to picking the best setting for each image afterwards on its behalf, which is generous to them), and two things are measured: whether it can reach TinyPNG's quality ("can reach"), and how much higher the score is at TinyPNG's file size (median). ImageMagick, FFmpeg and quantette palette PNGs have only fully transparent / opaque, so a second pass looks only at opaque images. Computed with `experiments/palette-feedback/tool_rank.py`:

| Tool | All 109 | Holdout 71 | Opaque 31 | Holdout opaque 24 |
|---|---|---|---|---|
| **Us (liq)** | **+1.6** (98/109) | **+1.1** (65/71) | **+1.8** (31/31) | **+1.3** (24/24) |
| pngquant | +0.3 (87) | 0.0 (53) | −0.2 (25) | −0.6 (18) |
| quantizr | −1.0 (53) | −0.1 (39) | +0.4 (22) | +0.5 (19) |
| quantette | −9.6 (21) | −7.6 (18) | +1.1 (20) | +1.3 (17) |
| exoquant | −5.1 (24) | −3.7 (19) | −2.4 (13) | −1.8 (12) |
| FFmpeg | −8.8 (15) | −7.0 (16) | −1.2 (14) | −0.1 (15) |
| NeuQuant | −9.8 (5) | −6.5 (4) | −8.4 (2) | −6.4 (2) |
| ImageMagick | −14.5 (8) | −11.3 (3) | −3.5 (7) | −3.2 (3) |
| Pillow | −19.5 (4) | −16.8 (4) | −19.9 (3) | −18.8 (2) |

(Numbers: median SSIMULACRA2 score difference at the same size as TinyPNG; in parentheses: number of images that reach TinyPNG quality.)

- We are first in all four columns, and the only one that almost always reaches TinyPNG quality. Next are pngquant and quantizr, roughly even with TinyPNG; the other tools are far behind, losing mainly on transparency (only on/off), dithering and palette allocation.
- Worth noting: on opaque images only, quantette (Wu initialization + k-means in Oklab) is very close; on the holdout set its median equals ours (+1.3), it just reaches TinyPNG on fewer images (17/24 vs 24/24). It does not support semi-transparency, but its palette method (Oklab, k-means) matches the refinement idea from round 9, so it could be one more 256-color candidate for opaque images (round 12).
- The comparison itself favors the other tools: for each image they get the best of 10 settings picked afterwards, while our liq curve has 6 targets, each a real result found by the search at compression time.

## Round 12: quantette palette as a candidate for opaque images

In round 11, quantette was about as good as us on opaque images. Its palette (Wu initialization + k-means in Oklab) starts from a different place than libimagequant's and may be better on some images. liqx gained a fourth palette source, `Source::Kmeans` (`quantize::kmeans_palette`): quantette produces the palette, which is passed to libimagequant as fixed colors for remapping (the dithering is also libimagequant's). It is not used if even one pixel in the image is not fully opaque (quantette does not handle semi-transparency). Used in two places:

- **Regular variant**: tried at every color count alongside d50 / d0 / d0r2, with labels starting with `q` (`q0` is no dither). Three ways of adding it were compared under automatic quality with full search; total size change (109 images / 71 images): `q0` −1.39% / −1.35%, `q50` −0.85% / −1.24%, `q0` + `q50` −0.99% / −1.53%. `q0` is consistent on both sets and needs fewer evaluations, so the default variants became `50,0,0r2,q0`.
- **Extra candidate at 256 colors**: the quantette palette with full dithering (`q100`), chosen by the same 15%-per-point rule as the full-dither, plain and refined candidates. Picked only on mahuri.

Browser version: k-means on the original is not cheap; the slowest image, tinypng@2x, went from under 10 s to 10.5 s. Two restrictions: the 256-color fallback (`at_top`) no longer considers `q` variants (the full-dither `q100` candidate stays); during confirmation on the original, a `q` variant ranked below first is no longer confirmed upward (it often climbs all the way to 256 colors and still can't reach, with a k-means pass on the original each time). tinypng@2x is back to 9.4 s; the second restriction affects only stripe@2x (1.1% larger).

| vs TinyPNG | 109 images | 71 images (holdout) |
|---|---|---|
| Automatic quality, full search: total size | 91.0% → **89.6%** | 87.9% → **86.7%** |
| Of which opaque images (31 / 24) | 94.7% → **91.5%** | 90.3% → **88.0%** |
| Automatic quality, browser: total size | 91.6% → **91.0%** | 90.4% → **89.2%** |
| Of which opaque images | 94.9% → **93.3%** | 93.8% → **91.4%** |
| Automatic quality: median score difference (full / browser) | +0.72 / +0.72, unchanged | +0.15 → +0.26 / +0.26, unchanged |
| Automatic quality: visibly worse | 2 / 3, unchanged | 2 / 3, unchanged |
| Size at equal SSIMULACRA2 quality (median / total) | 89.0% / 85.8% → 88.6% / 84.9%\* | 89.1% / 86.6% → **87.9% / 84.3%** |
| Size at equal butteraugli quality (median / total) | 89.3% / 80.9% → 90.2% / 80.8%\* | 90.6% / 83.1% → **89.1% / 82.4%** |
| Can't reach TinyPNG quality (SSIMULACRA2) | 14 → 11\* | 6 → 6 |

\* The baseline for the two equal-quality rows on the 109 images is `bench/results/20261023-222009`, which predates rounds 8 and 9, so not all of the difference comes from this round; the holdout baseline (`bench/results/holdout-20261024-214649`) differs from this round only by `q0` and `q100`. New results are in `bench/results/q0-main-20261024-232434` and `bench/results/q0-holdout-20261024-232434`.

- The images that pick `q` are almost all opaque photos (of the 24 Kodak images, 17), followed by a few @1x website screenshots (hackernews, python-docs, djangoproject, github-docs), mozilla@2x, stripe@2x and a terrain map illustration. None falls below the target because of it; with full search only kodim17 got 1.1% larger (probably because candidates are ranked by level-4 deflate size, and the order flips after oxipng).
- The browser version gets only half the gain on the 109 images (almost all of it on the holdout set). It is not caused by the second restriction (removing it changes only stripe@2x); most likely it is the ranking on the downscaled image: confirmation on the original only gives one chance to the first and second ranked, so `q0` can't be picked when it ranks low (kodim21 and the terrain map, with `q0` under full search, are 15–17% smaller, but the browser version did not pick it).
- Cost: total native search time is 6–12% higher; browser version, 5MP screenshots, multithreaded 4.9–9.4 s (previously 4.8–8.4), single-threaded 11–21 s; the single-threaded WASM build is 970 KB. Peak memory for a 12MP image 2743 → 2789 MiB; the existing pixel limits do not need to change.
- quantette and palette are both MIT / Apache-2.0.

## Round 13: dropping libimagequant

libimagequant is GPL-3.0 (or a commercial license), the only copyleft among liqx's dependencies: oxipng is MIT, libdeflate is MIT / Apache-2.0, fast-ssim2 is BSD-2, quantette and palette are MIT / Apache-2.0. Releasing under the MIT license means replacing it. This round first measured what replacing it would cost; compression actually improved instead of getting worse, so it was removed outright.

`tools/liqx/src/permissive.rs` is our own palette generation and mapping, using only textbook methods and no reference to libimagequant's code: weighted median cut (each box is split where it most reduces squared error) plus weighted k-means to build the palette; mapping uses nearest color or serpentine Floyd–Steinberg, with weaker dithering at edges and in texture. During the comparison it lived alongside libimagequant (`liqx --backend own`), and the search, automatic quality and the four palette sources needed no changes (quantette's palette also goes through this mapping).

Tuning tool `tools/liqx/examples/backend_lab.rs` (it and `reference_lab.rs` below both need libimagequant as a reference, and were deleted along with it): at 16 / 64 / 256 colors and three dither levels, cross {libimagequant, own} palettes × {libimagequant, own} mappings, to see separately how much the palette and the mapping each lose. Tuned only on 26 images from `bench/images` (18 sampled across categories, plus 8 hard images at the high-quality end); the holdout set was not used. Problems found, in order:

- **Color space**: distances were first computed in gamma sRGB, and nearest-color mapping alone was 2.2 points below libimagequant (kodim23: 7.7 lower). After switching to Oklab it was 0.2 higher instead, and the palette at 256 colors was also better than libimagequant's.
- **Full dithering**: with an error cap of 48 it was 2.4 points below libimagequant and 11% larger; with a cap of 6 and no diffusion of errors below 1.5 it is even (256 colors: −0.03 points, +3% bytes).
- **Error feedback**: gain 8 (tuned for libimagequant) hurts k-means: at 16 colors it is 4 points lower; gain 1 is best at 64 / 256 colors.
- **Semi-transparency**: Oklab values multiplied by alpha do not describe what the composited result looks like; the 8 hard images (all with transparency, mostly line drawings on transparent backgrounds) were 0.6–1 points behind. Changed to "the color composited over dark gray 24 and light gray 232, and the six Oklab coordinates of the two results" (exactly the two backgrounds the benchmark uses to score transparent images; pure black and white collapsed by 21 points on one image). Then semi-transparent pixels were excluded from dithering (the error leaks into neighbors along anti-aliased edges), and full dithering went from −1.2 points to +0.3 points.
- Final, 26 images, all own vs all libimagequant: 256 colors +0.45 to +0.80 points, 64 colors +0.83 to +2.35 points, with similar or fewer bytes. Dithering at 16 colors is poor on a few complex illustrations, but there every method is below 60 and the search never goes there.

The automatic-quality reference score used to be the score of libimagequant's plain palette (speed 4, full dithering), which happened to track TinyPNG's quality closely. It is now our own plain palette, half dithering, minus 1.4 points (`REFERENCE`, chosen with `examples/reference_lab.rs` on both image sets): correlation with TinyPNG's score 0.96 / 0.92, versus 0.94 / 0.96 for libimagequant.

Results of the final version (after removing libimagequant and speeding up), `bench/results/nolq-main-20261025-102740` and `bench/results/nolq-holdout-20261025-102740`, compared with round 12:

| vs TinyPNG | 109 images: libimagequant → own | 71 images (holdout): libimagequant → own |
|---|---|---|
| Size at equal SSIMULACRA2 quality (median / total) | 88.6% / 84.9% → **84.4% / 81.1%** | 87.9% / 84.3% → **81.6% / 77.7%** |
| Can't reach TinyPNG quality | 11 → **6** | 6 → **4** |
| Score difference at equal size (median / P10) | +1.6 / −0.6 → +1.9 / −0.0 | +1.1 / −0.1 → +1.8 / +0.1 |
| Size at equal butteraugli quality (median / total) | 90.2% / 80.8% → **86.3%** / 81.8% | 89.1% / 82.4% → **85.6% / 79.1%** |
| Can't reach under butteraugli | 10 → 14 | 8 → 8 |
| Automatic quality, full search: total size / median score difference | 89.6% / +0.72 → **88.2%** / +0.56 | 86.7% / +0.26 → **81.8% / +0.47** |
| Automatic quality, browser: total size / median score difference | 91.0% / +0.72 → **89.3% / +0.78** | 89.2% / +0.26 → **84.1% / +0.62** |
| Automatic quality: visibly worse (full / browser) | 2 / 3 → 3 / 3 | 2 / 3 → 3 / 3 |

- No loss in compression; it is better: 4–6 percentage points smaller at equal quality, with a larger gain on the holdout set, on both metrics. The exceptions: under butteraugli, on the 109 images, the number that can't reach TinyPNG grew by 4 (under SSIMULACRA2 it fell by 5), and under full search "visibly worse" grew by 1. The visibly worse images are now educational line drawings on transparent backgrounds (bicycle, mouse-mechanism, renal-corpuscle; college-of-arms and fortepian in the holdout set), now the category furthest from TinyPNG.
- **Speed-up**: the first version was single-threaded and built the histogram from all colors in the original. On a 5MP screenshot one palette (including the two error-feedback passes) took 0.8–1.2 s and one dithered mapping 0.4–0.7 s; in the browser stripe@2x and tinypng@2x took 14 s. Three changes, none of whose results depend on thread count (fixed chunks, merged in order): when the histogram has more than 2^18 colors, merge by dropping the low bits of r, g, b (weighted average, alpha untouched; the first attempt merged down to 2^16 colors including alpha, and the browser version got 1.3% larger); k-means nearest-color assignment and undithered mapping run in parallel chunks; dithering runs in parallel strips of 64 rows, each strip starting from zero error, and pixels with no carried error look up a per-color cache. Native: palette 0.26 s, dithered mapping 65–94 ms; total search time is on par with the libimagequant version (automatic quality, 109 images: 337 s vs 369 s).
- **Browser**: 8 screenshots at 5MP, multithreaded 3.6–8.9 s (libimagequant version 4.9–9.4 s); single-threaded 8–23 s (11–21 s; stripe@2x went from 18 s to 23 s). Single-threaded WASM build 928 KB (970 KB), multithreaded 1.36 MB. Memory actually went down: 12MP peak 2789 → 2350 MiB, 15MP 3425 → 2934 MiB, so the pixel limits stay; a 12MP image finishes in 31.6 s in the multithreaded build.
- **Browser and command line byte-identical**: after removing libimagequant, of 24 sampled images, 4 had browser output that differed from `liqx --finish --preview-pixels 350000` by a few bytes up to 1%. Threads were not the cause (results did not change with parallelism off); it was math functions: `cbrt` and `powf` in the Oklab conversion use the system math library natively and Rust's port of musl in WASM, which differ in the last bit and occasionally change a k-means decision. After switching to the `libm` crate (pure Rust, which fast-ssim2 already uses) and writing `powi(3)` as multiplication, all 180 images in both sets are byte-identical between native and WASM, and so is the multithreaded Chrome build on the 8 screenshots. The downscaled-search size also moved into liqx (`PREVIEW_PIXELS`); `liqx --finish` uses it by default and the browser no longer passes it, so the command line without arguments matches the browser (for a full search on the original use `--preview-pixels 0`; bench's search mode still defaults to full search).
- **Remaining time** (Chrome multithreaded, 8 screenshots at 5MP): SSIMULACRA2 scoring 55%, palette 22% (of which quantette's k-means on opaque images is single-threaded), oxipng 10%, reference image 5%, fast encoding 4%, mapping 4%. Dropping the quantette candidate (`q0`) makes the search 20–40% faster on opaque images, but under full search these images get 1.8% larger (a single Kodak photo up to 17% larger); substituting our own plain palette (`p0`) does not make up for it, since Wu initialization really is better on photos. quantette's multithreaded mode makes results depend on thread count, so it is off.
- **Removed**: the `imagequant` dependency (`RGBA` now comes from the `rgb` crate), `liqx --quality` and `--backend`, and the three experiment programs that need libimagequant as a reference (`palette_lab`, `backend_lab`, `reference_lab`). The default error feedback gain changed from 8 to 1 (`bench.py --liq-feedback` updated to match). liqx now has 67 dependency crates, all licensed MIT, Apache-2.0, BSD-2-Clause or Zlib (`Cargo.toml` declares `license = "MIT OR Apache-2.0"`).

## Round 14: cleanup for open source

Goals: code quality, as few dependencies as possible, as fast as possible, the same results from the command line and the browser, and no leftover code from old approaches.

- **Rename, structure**: liqx (libimagequant + x) is renamed pngdeer and moved to the repository root, forming one Cargo workspace with `web/wasm` and `tools/quantlab`: one Cargo.lock, and both the native and browser versions use `vendor/fast-ssim2`. Modules are split by responsibility: `color` (Oklab and two-background compositing), `palette`, `remap`, `search`, `codec`, `par` (parallelism with fixed chunks).
- **One command-line mode**: `pngdeer IN -o OUT [--target auto|T] [--preview-pixels N] [--variants SPEC]`, which is the browser pipeline, plus two small helpers for bench (`--score`, `--upscale`). Removed the bench-only "search only" mode, the old plain quantization mode, and the parameters only tuned in experiments (`--speed`, `--feedback`, `--grain-weight`, `--fixed`, `--importance`, `--dither-mask`, `--exact`, `--small-side`, `--auto-below`, `--auto-cap`, `--verify-steps`, `--preview-margin`, `--finish`); they are now constants in the code. Output for the 180 images in both sets is byte-identical before and after the cleanup.
- **bench tests what is actually released**: `pdT` and `pdauto` call the command line directly, and the duplicated finishing logic in Python (oxipng, lossless for few-color images, fall back if larger than the original) is deleted; the pngquant auto-search baseline `autoT` is deleted too (round 1's conclusions are recorded above). bench.py went from 1132 lines to 930. The blind test now recognizes `pd`; old runs still open with `--challenger liq --results ...`.
- **quantette replaced by our own Wu quantizer**: Wu (Graphics Gems II, 1991) bins in Oklab, then refines with online k-means (MacQueen, learning rate count^-1/2, fixed pseudo-random sampling); used only for opaque images. With bins over the Oklab range that sRGB can cover, 32×16×16 bins, it ties with quantette (55 opaque images: full search on the original +0.01%, downscaled search −0.15%); with 32³ bins over the image's own color range, Kodak photos get 14–17% larger. Dependencies went from 64 crates to 46. The variant label changed from `q0` to `w0`.
- **Removed the `q100` extra candidate** (quantette palette with full dithering): in about 1080 runs over both sets it was picked only 7 times.
- **Refined palette** (`p100k`) now uses the shared histogram, nearest-color lookup and parallel code: it used to iterate over a HashMap, so the order of floating-point accumulation could differ between runs.
- **Results unchanged** (full search on the original, vs round 13): size at equal SSIMULACRA2 quality, 109 images 84.4% / 81.1% → 84.5% / 80.9%, 71 images 81.6% / 77.7% → 82.2% / 77.8%; can't reach TinyPNG is still 6 and 4 images; butteraugli 109 images 86.3% / 81.8% → 86.3% / 81.5%, 71 images 85.6% / 79.1% → 85.0% / 78.7%. Released default settings (downscaled search): equal quality 84.6% / 81.6% and 83.3% / 79.0%, automatic quality total size 89.0% and 84.2%, median score 0.7 and 0.6 higher than TinyPNG (`bench/results/final-main-20261025-120607`, `final-holdout-20261025-120607`, `full-main-…`, `full-holdout-…`).
- **Size and speed**: single-threaded WASM build 970 KB → 839 KB, multithreaded 1.36 → 1.24 MB. 8 screenshots at 5MP in Chrome: multithreaded 3.5–7.7 s (round 13: 3.6–8.9), single-threaded 7.8–20.3 s; the 180 images are still byte-identical between native and WASM.
- **Quality**: clippy (`-D warnings`) and rustfmt are clean, and `rustfmt.toml` sets the line width; unit tests (color round trip, exact nearest color) and end-to-end tests (reaching a fixed target, same input gives the same bytes, lossless for few-color images, variant parsing); GitHub Actions runs these three. `LICENSE` (MIT); `web/notices.py` generates the third-party license list for the browser version, linked from the page. The published fast-ssim2 crate has no LICENSE file, so `vendor/fast-ssim2/LICENSE` was added from its repository.

## Round 15: always dither smooth gradients

A user compared two images with TinyPNG: by eye ours were worse, but the scores said ours were better:

- **Oil-painting-style portrait** (1254²): the default picked the undithered `w0`, 656 KB, 81.8; TinyPNG 877 KB, 79.2. Zoomed in, the skin breaks into flat patches with contour lines between them, and the left side of the lower lip turns purple; TinyPNG is grainy but the shading is continuous. On the face alone, SSIMULACRA2 gives us 80.5 and TinyPNG 74.6, and butteraugli also prefers us (4.0 vs 5.0). **Both metrics prefer flat patches over dither grain**, so the search always picked no dithering.
- **Wooden deer** (1448×1086, RGBA): after cutout the whole deer has alpha 240–254, with not one pixel fully opaque. Only fully opaque pixels used to be dithered, so not a single pixel in the image was dithered: the chosen `d50` and `d0` differed by only a few hundred bytes. The large backlit shadow under the belly mapped to lighter, more orange colors, and the shadow got lighter.
- Tried measuring "how far the local average color drifts" (blocks from 4×4 to 16×16, distance between the block's average color and the original's, counting only blocks above a threshold): TinyPNG drifts more than us on both images, yet the eye prefers it. What is visible is the **contours** of flat patches, not a shift in average color; this measure did not help and was removed.

Two changes, both in `src/remap.rs`:

- **Semi-transparent pixels are dithered too**, as long as their alpha differs from each of the four neighbors by at most 16 (`ALPHA_STEP`): soft shadows and cutouts with alpha in the 250s throughout both qualify; anti-aliased edges, where alpha jumps, are still not dithered, so the original concern, "error seeping out along line-drawing edges", is still prevented.
- **Smooth gradients get at least half-strength dithering in every variant** (`SMOOTH_DITHER`): a pixel counts as a smooth gradient if there is change within 2 pixels of it but no neighboring difference exceeds `FADE_FROM` (12). 78% of the portrait's pixels count as smooth. The undithered variants (`d0`, `d0r2`, `w0`) still do not dither flat areas, text, edges and strong texture, so their size advantage stays there.

Results:

- Portrait 656 → 670 KB (+2%, still `w0`, score 82.4); the patch edges are broken up by grain. Deer 292 → 298 KB (+2%); the depth of the shadow is back. For both images the browser version and the command line still give the same size.
- Default output by category (109 images / 71 images): 3D renders +8.2% / +10.1% (almost all smooth gradients; the largest increase is fluent-money-bag, +21%, where the banding on the gradient is gone, 8.3 → 10.0 KB, still smaller than TinyPNG's 11.1 KB), photos with transparency +3.5% / +0.8%, photos +1.3% / −0.9%, icons and illustrations 0% to +1%, UI −2.8% / −0.2%; about two in five images did not change size.
- Size at equal SSIMULACRA2 quality: 109 images 84.6% / 81.6% → 88.8% / 83.2%, 71 images 83.3% / 79.0% → 84.5% / 79.7%; can't reach goes from 6 and 4 images to 4 and 4. butteraugli (3-norm, before and after recomputed with the same interpolation method): 86.9% / 82.2% → 90.3% / 84.2%, 86.1% / 79.7% → 87.3% / 80.4%. Automatic quality total size 89.0% → 90.0%, 84.2% → 84.4%; median score +0.6 and +0.5 above TinyPNG. Against other open-source tools (tool_rank.py): +1.9 / +1.7 points at TinyPNG's size, reaching TinyPNG quality on 105/109 and 67/71 images (previously 103 and 67). `bench/results/smooth-main-20261025-142520`, `smooth-holdout-20261025-142520`.
- Speed: one image at a time, new vs old version per image is between −15% and +20%, with no consistent direction.

On this issue both metrics disagree with the eye, so the trade-offs in this round could only be decided by looking at images. Future tuning of dither-related parameters should also look at images first and scores second.

## Round 16: no dead zone in smooth gradients

After round 15, a user saw that the gradient on the 3D rocket in the samples (fluent-rocket, 256²) was still patchy. Zoomed in, the smooth gradients are patches of undithered flat color with grain only at the borders: pixels with error below `DEAD_ZONE` (1.5) do not pass on error, which saves bytes in flat areas but leaves islands of flat color in gradients. TinyPNG dithers fully, with even grain.

- No dead zone in smooth gradients anymore (pixels selected by `smooth_map` always pass on error); flat areas, edges and texture are unchanged. Rocket 8853 → 9036 bytes, the patches are mostly gone (TinyPNG 9771).
- Also tried raising the strength in smooth gradients to 0.75: rocket 9571 bytes, as even as TinyPNG, but the default output on both sets is another 3–7% larger than with only the dead zone removed, and large background gradients in website screenshots are the most expensive (react-dev@2x +37%, kotlinlang@2x +41%, svelte@2x +40%, compared with strength 0.5 with the dead zone). The patches come from the dead zone, not the strength, so it stays at 0.5.
- Compared with round 15 (109 images / 71 images): size at equal SSIMULACRA2 quality 88.8% / 83.2% → 92.2% / 84.8%, 84.5% / 79.7% → 88.0% / 81.1%; can't reach TinyPNG quality 4 → 3 images, 4 → 4 images; butteraugli (3-norm) 90.3% / 84.2% → 93.8% / 86.3%, 87.3% / 80.4% → 91.0% / 83.3%; automatic quality total size 90.0% → 92.6%, 84.4% → 87.2%, median score +0.7 and +0.8 above TinyPNG. By category: 3D renders +7.3% / +9.8%, illustrations +4.4%, photos with transparency +3.0% / +1.7%, UI +3.0% / +7.4%, photos and icons about unchanged. Against other open-source tools: +2.0 / +1.7 points at TinyPNG's size, reaching TinyPNG quality on 106/109 and 67/71 images. `bench/results/smooth50nodz-main-20261025-222913`, `smooth50nodz-holdout-20261025-222913` (strength 0.75 is `smooth75-*-20261025-221831`).
- The browser version and the command line still give the same size on the 6 sample images.

## Round 17: large images in the browser

A user's 35.9 MP PNG was refused: the browser version took at most 16 MP (12 MP threaded), because scoring peaked at about 260 bytes a pixel and wasm32 has 4 GB.

- **Where the memory went** (dhat, 4 MP, one thread, 893 MB at the peak): fast-ssim2's cached reference 68 bytes a pixel, each comparison's planes 110, copies of the image 26. Two things doubled it: an opaque image went in as RGBA, so the reference and every comparison were made twice, over two backgrounds that give opaque pixels the same values; and the threaded blurs cloned their outputs, so every blurred plane existed twice.
- **Tiles and strips don't work**: SSIMULACRA2 is dominated by the worst regions, so averaging independent tiles scores up to 24 points too high and the worst tile up to 20 too low (16–21 MP images, 1024–4096 px tiles). fast-ssim2's strips drift +0.05 to +1.6 points (256 rows, 96 of overlap) and still +0.004 to +0.42 (2048 rows, 512): its blur is recursive, so no overlap makes them exact.
- **What changed**, with scores identical to the bit:
  - Opaque images go to fast-ssim2 as RGBX: one reference stack and one comparison instead of two.
  - fast-ssim2 no longer clones the blurred planes, allocates planes it doesn't use, or copies the image for padding (`vendor/fast-ssim2/PATCHES.md`, change 4).
  - Above 8 MP scored (`LEAN_PIXELS`), a lean reference keeps only the source. Each check linearizes both images, walks their scales together and, per channel, runs the five blurs one after another, then that channel's maps: at most five blurred planes are alive, and no pyramid is kept.
- **Memory and time** (single-threaded WebAssembly in Node, the same V8 as Chrome; memory is how far WebAssembly memory grew, which never shrinks):

  | Image | Before | After |
  |---|---|---|
  | kodim01 (0.4 MP) | 3.1 s, 91 MB | 2.6 s, 56 MB |
  | stripe@2x (5.2 MP) | 16.6 s, 1075 MB | 13.1 s, 643 MB |
  | sun poster at 4 MP | 13.4 s, 831 MB | 10.7 s, 497 MB |
  | sun poster at 16 MP | refused | 37 s, 1.22 GB |
  | 4 screenshots, 21 MP | refused | 41 s, 1.58 GB |
  | cut-out with alpha, 24 MP | refused | 97 s, 2.07 GB |
  | 6 screenshots, 36 MP | refused | 73 s, 2.75 GB |
  | cut-out with alpha, 40 MP | refused | 164 s, 3.45 GB |

  Large images take about 80 bytes a pixel, 86 with transparency; the threaded build about the same (36 MP 2.78 GB, 40 MP with alpha 3.48 GB). The browser now takes up to 40 MP in either build. Opaque images up to 8 MP are also 16–21% faster, scored once instead of twice.
- **Checked**: on all 180 benchmark images, at automatic quality and at target 85, the files and reports are byte-identical to before, both as shipped and with the lean reference forced on for every image. The browser builds and the command line give the same bytes on the 4–40 MP test images.
- A lean check runs its blurs on one thread even in the threaded build (in parallel each blur needs its own scratch plane, 16 bytes a pixel more), so above 8 MP scoring is no faster threaded.

## Round 18: threads for large images and palettes

Speed only: on all 180 benchmark images, at automatic quality and at target 85, the files and reports are byte-identical to 0.1.2.

- **Blur**: fast-ssim2's scalar blur (what the browser runs) now does its horizontal pass a row per thread and its vertical pass a 256-column range per thread. A recursive Gaussian treats each row, then each column, on its own, so the scores are bit-identical; no plane is added, and products of two planes are formed a row at a time in the output plane, which the vertical pass overwrites anyway, instead of in a plane of their own (`vendor/fast-ssim2/PATCHES.md`, change 5). This undoes round 17's last point: above 8 MP the threaded build's checks are no longer on one thread.
- **Palette**: k-means assigns colors to centers in parallel, then adds them up in the old order; plain color counts are made in 16 parts and merged (whole numbers below 2^24, which f32 adds exactly in any order). Weighted counts, and builds without threads, keep the single pass. Median cut stays serial: sorting in parallel would change the order of ties.
- **Threaded browser build** (headless Chrome, 14 threads):

  | Image | Before | After |
  |---|---|---|
  | kodim01 (0.4 MP) | 1.6 s, 110 MB | 1.3 s, 101 MB |
  | stripe@2x (5.2 MP) | 5.7 s, 984 MB | 5.0 s, 859 MB |
  | sun poster at 16 MP | 22.3 s, 1.25 GB | 14.8 s, 1.25 GB |
  | cut-out with alpha, 24 MP | 77 s, 2.10 GB | 57 s, 2.10 GB |
  | 6 screenshots, 36 MP | 48.8 s, 2.78 GB | 27.3 s, 2.78 GB |

- **Command line** (14 threads, best of 3): kodim01 1.26 → 0.99 s, stripe@2x 3.66 → 3.36 s, sun poster at 4 MP 3.39 → 3.04 s, cup-of-tea 2.11 → 1.79 s, 4 screenshots at 21 MP 9.11 → 8.84 s. On one thread, and in the single-threaded browser build, times are unchanged. That build's WebAssembly memory grows 1–2% further (kodim01 56 → 58 MB, stripe@2x 643 → 654 MB, sun poster at 4 MP 497 → 506 MB; unchanged at 21 MP), though the most live data at once is the same (dhat on the scalar scorer: stripe@2x 818 MB, sun poster at 16 MP 2527 MB, before and after) and each comparison makes 108 fewer allocations: the allocator packs the planes differently.

## Still unsolved

- **Educational line drawings on transparent backgrounds**: the category furthest from TinyPNG since round 13. bicycle-diagram, even at 256 colors, can't reach TinyPNG's 90.0; mouse-mechanism, renal-corpuscle, college-of-arms and fortepian are 1–2 points lower under automatic quality. Anti-aliased semi-transparent edges are not dithered (since round 15, semi-transparent pixels with smooth alpha are dithered), and there may still be room in palette allocation for flat patches and thin lines. (The former worst case, `linear@2x`: size at equal quality 116% → 107% (round 12) → 93%; react-dev@2x 123% → 100%; both solved in round 13.)
- **3D / illustrations at the high-quality end**: rounds 8 and 9 added three 256-color candidates: full dithering, plain palette and refined palette (round 12 also added the quantette palette for opaque images), and the mean gap on the hard images shrank from −1.27 to −0.69; for what remains, see the last item of round 9 (those are libimagequant-version numbers).
- **Neither metric sees banding**: round 15 works around it with the "always dither smooth gradients" rule, but the search still has no signal that quantifies banding and contour lines (such as Netflix's CAMBI). What counts as "smooth" is a threshold; calibrating it with a blind test would be more reliable.
- **Search on large images**: since round 2 the native version is no slower than TinyPNG, and since round 5 the browser version handles 5MP in under 10 s; the bottleneck is still SSIMULACRA2, and the coarse search on a downscaled image costs 1.5% in size. The native version can also use `--preview-pixels`, but bench does not enable it. Since round 18 the threaded browser build also splits each blur across threads above 8 MP.

## Reproducing

```bash
cargo build --release
python3 bench/bench.py
python3 bench/bench.py --images bench/images-holdout
python3 experiments/palette-feedback/butteraugli_check.py bench/results/latest tinypng,pd70,pd75,pd80,pd85,pd90,pd95
```

- `butteraugli_check.py`: rescores the outputs of one bench run with butteraugli and writes `<run>/butteraugli.csv`. Requires Pillow and numpy.
- `tool_rank.py`: the round 11 comparison; merges each tool's outputs into a size–quality frontier and compares with TinyPNG.
- `ui_diag.py`: the diagnostics from rounds 1 and 2; compares the palettes, flat-color reproduction and error locations of TinyPNG and pngquant outputs of the same size.

The scripts used in earlier rounds, `ui_exp.py` and `liqexp.py` (which drove liqx's old quantization mode), and `palette_lab`, `backend_lab`, `reference_lab` (which need libimagequant as a reference) have all been deleted; their results remain in the rounds above. Numbers and names before round 14 (`liqx`, `liqT`, `q0`) refer to the versions of that time.

The run folders named above (`bench/results/...`) stayed local, and the website screenshots (`ui/`) are not in the repository, so a rerun from it covers 94 of the 109 images and 61 of the 71. The `tinypng` pipeline needs a TinyPNG API key (see `.env.example`).
