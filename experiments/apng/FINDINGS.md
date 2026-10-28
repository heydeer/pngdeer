# Animated PNGs

2026-10-28. Before this, pngdeer kept only an animated PNG's first frame, silently (`codec::decode`
reads one image), and its WebP output refused animations. Now an animated PNG stays animated:
`apng::decode` draws every frame as a viewer shows it, the search runs over all frames on one
palette (`search::run_animation`), and `apng::encode` writes the frames back.

## How

- **Frames as shown.** Each frame is blended and disposed onto the canvas per the APNG spec, so the
  search scores the images people see. Checked against Pillow on 13 animations (12 below and a
  3-frame test): identical, pixel for pixel. Where they differ, on an animation of translucent
  frames blended over translucent ones, pngdeer follows the spec's formula and Pillow pastes
  through the frame's alpha (alpha 129 where the spec gives 191); browsers composite per the spec.
- **One palette.** A palette APNG has one PLTE for every frame, so the palettes are built from all
  frames at once (stacked into one tall image). Each frame is remapped on its own, then every pixel
  whose source is the same as in the frame before keeps that frame's index: dithering stays still
  where the picture does, and the changed rectangle stays small.
- **Scored per frame.** A candidate's score is its worst frame's, so every frame reaches the
  target; a frame that repeats the one before (same source, same indices) isn't scored again.
  Automatic quality is as for still images, on the worst frame of a plain 256-color palette.
- **Written as changes.** The first frame in full, each later one as the rectangle where it
  differs from the frame before (dispose: none). Inside it, pixels that stay the same become a
  fully transparent entry blended over the canvas when that is allowed (every changed pixel opaque,
  or landing on a clear pixel) and smaller. A frame that changes nothing adds its delay to the one
  before. Delays, play count and a separate default image are kept. oxipng then recompresses every
  frame (it turns off its color reductions on APNGs).
- **A preview of a few frames.** An animation over the preview budget (0.35 MP over all frames)
  and of at least 16 frames, or with frames large enough to downscale, is searched first on 8 of
  its frames, then checked on all of them as a still image's preview is checked at full size. The 8
  are the frames a plain 256-color palette serves worst (auto mode scores it anyway): evenly spaced
  frames missed the hardest ones, the checks then stepped up past the target, and files came out
  up to 9% bigger at a fixed target. Frames are downscaled only past the budget, and never below
  256 px on their longer side; full-size frames get no preview margin.
- The usual fallbacks: the input optimized losslessly if that is smaller, a lossless palette for
  animations of at most 256 colors (when it costs at most 15% more), the input if nothing beats it.

## Results

12 animated emoji from Microsoft's Fluent set (github.com/microsoft/fluentui-emoji-animated, MIT;
256 × 256, translucent, 13–73 frames), at automatic quality:

| Animation | Frames | Original | Lossless (oxipng) | pngdeer | Worst frame (target) | Colors |
|---|---:|---:|---:|---:|---:|---:|
| aerial tramway | 72 | 1456 KB | 93.5% | 23.1% | 83.2 (82.9) | 230 |
| airplane departure | 72 | 1906 KB | 93.3% | 25.1% | 86.3 (86.2) | 163 |
| alarm clock | 37 | 1357 KB | 96.0% | 23.5% | 84.4 (83.5) | 225 |
| alien monster | 63 | 1641 KB | 95.8% | 32.7% | 88.8 (88.6) | 176 |
| ambulance | 72 | 2211 KB | 94.7% | 23.7% | 83.8 (83.7) | 204 |
| anatomical heart | 13 | 480 KB | 94.0% | 27.6% | 83.3 (83.0) | 234 |
| angry face | 72 | 2628 KB | 94.3% | 30.9% | 85.2 (84.5) | 230 |
| beating heart | 35 | 888 KB | 90.5% | 32.7% | 88.8 (88.7) | 189 |
| fire | 48 | 1170 KB | 93.0% | 35.9% | 83.5 (83.4) | 163 |
| ghost | 73 | 2300 KB | 91.4% | 32.4% | 83.1 (83.0) | 204 |
| hourglass not done | 72 | 1908 KB | 93.0% | 28.9% | 81.3 (80.1) | 234 |
| party popper | 61 | 1636 KB | 96.3% | 23.2% | 81.3 (80.8) | 234 |
| **all 12** | | 19.6 MB | 93.9% | 28.2% | | |

Against TinyPNG (its API compresses APNGs: one palette of 255–256 colors, the input's frame
rectangles and dispose ops kept), on 3 of the emoji and a screen recording (36 frames of 1000 ×
640: a cursor moving over pngdeer's site, then a scroll). pngdeer's size where it matches TinyPNG's
worst frame, and (searching for an average frame instead, an experiment, before the preview)
where it matches TinyPNG's average frame:

| Animation | TinyPNG | Worst frame: TinyPNG | pngdeer's size at it | Average frame: TinyPNG | pngdeer's size at it |
|---|---:|---:|---:|---:|---:|
| fire | 470 KB | 82.5 | 84.3% | 87.3 | 89.8% |
| ghost | 757 KB | 81.7 | 94.9% | 84.8 | 99.2% |
| beating heart | 280 KB | 88.7 | 101.6% | 89.9 | 103.7% |
| screen recording | 151 KB | 88.0 | 91.3% (average 90.8 against 90.1) | 90.1 | at most 91.3% |

So about even to 16% smaller, less of a lead than on still images. TinyPNG's frames are uneven
(fire: worst 82.5, median 87.8), which a worst-frame match favours; on the average frame two of the
emoji come out about level. Deflate isn't where it differs: zopfli shrinks either file by 1% at most.

Time and memory (natively, 14 threads): with the preview, the emoji take 2.6 s on the median (3.5
s searched whole; 25% less over the 12) and the screen recording 4.9 s (16.4 s whole), for
100.1% of the whole search's bytes; up to 0.9 GB (73 frames) and 1.5 GB (the screen recording),
which the full-size checks still need. In the threaded browser build, byte-identical to the
command line: fire 3.5 s, ghost 3.8 s, the screen recording 6.8 s (26 s searched whole). The
browser takes animations up to 30 MP over all frames (web/worker.js).

## Tried and dropped

- **Temporal smoothing**: keeping the previous frame's entry where it is at most δ worse for the
  new source (as `prefer_neighbours` does with neighbours in space). It breaks the dithering's error
  diffusion: at δ = 2 the worst frame fell from 82.7 to 75.5 on fire, out of reach at any palette size.
- **Other criteria than the worst frame** (the 10th percentile, the average): compared at
  TinyPNG's level on the same measure they didn't come out ahead (89.8% / 99.2% / 103.7% on the
  average), and the worst frame is the guarantee that no frame looks worse than the target.
- **Remapping variants** at TinyPNG's average quality: dithering at 0–50% within a few percent of
  each other, heavier dithering and neighbour smoothing worse.

## Open

- Palettes: building them is most of an emoji's time (median cut and k-means over ~120,000
  colors, again for every palette size). Reusing work across sizes would change the palettes
  slightly (k-means reads the colors in the order median cut leaves them).
- WebP output still refuses animations (an animated WebP would need its own search).
