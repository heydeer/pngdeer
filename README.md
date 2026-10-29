# pngdeer

**Smaller images. Same picture.** pngdeer compresses PNG images into smaller PNG or WebP files, with no quality slider: it scores candidate images with a perceptual metric and keeps the smallest file that still looks right. For PNG output, the median image comes out **8–12% smaller than with TinyPNG** at the same quality, the best result of any open-source PNG compressor we've measured.

**[Try it in your browser →](https://heydeer.ai/open-source/pngdeer)** Nothing is uploaded: images are compressed on your device.

[![pngdeer in the browser](.github/pngdeer.png)](https://heydeer.ai/open-source/pngdeer)

```bash
brew install heydeer/tap/pngdeer
pngdeer input.png -o output.png
pngdeer input.png -o output.webp
```

- **PNG or WebP output**: keep PNG or write WebP, with automatic quality in either format.
- **Picks the quality for you**: about what TinyPNG would give the image, never above 90 on SSIMULACRA2's scale, which is indistinguishable at 1:1.
- **Same bytes everywhere**: the command line (macOS and Linux) and the browser (WebAssembly, on every core) run the same Rust code and write identical files.
- **Big images too**: up to 40 megapixels in the browser, more on the command line.
- **Animated PNGs stay animated**: every frame on one palette and at the target quality, timing kept; 28% of the original size on 12 animated emoji, about even to 16% smaller than TinyPNG ([notes](experiments/apng/FINDINGS.md)).
- **Free and open source**: MIT.

## Leaderboard

### PNG

We ran pngdeer, TinyPNG and eight open-source quantizers on 180 PNGs: icons, UI screenshots, illustrations, photos, cut-outs and 3D renders. Every output was scored against its original with [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2), a perceptual metric where 90 means indistinguishable at 1:1. TinyPNG is the yardstick:

- **Points**: median SSIMULACRA2 points above (+) or below (−) TinyPNG when a tool's file is as big as TinyPNG's.
- **Reaches**: images where a tool can match TinyPNG's quality at all.

Each cell gives the held-out set (71 images) / the tuning set (109 images):

| Compressor | Points | Reaches |
|---|---:|---:|
| **pngdeer** | **+1.7 / +2.0** | **67 / 106** |
| TinyPNG (the yardstick) | 0 | 71 / 109 |
| pngquant | 0.0 / +0.3 | 53 / 87 |
| quantizr | −0.1 / −1.0 | 39 / 53 |
| exoquant | −3.7 / −5.1 | 19 / 24 |
| NeuQuant | −6.5 / −9.8 | 4 / 5 |
| FFmpeg | −7.0 / −8.8 | 16 / 15 |
| quantette | −7.6 / −9.6 | 18 / 21 |
| ImageMagick | −11.3 / −14.5 | 3 / 8 |
| Pillow | −16.8 / −19.5 | 4 / 4 |

At TinyPNG's quality, pngdeer's files are 12% smaller on the held-out set and 8% smaller on the tuning set (median image).

- **Held out**: pngdeer's parameters were tuned on the 109 images; the other 71 were collected afterwards and never used for tuning.
- **Generous to the others**: each open-source tool gets its best of ten settings per image, picked after the fact. pngdeer's results are what you get, searched at compression time.
- **A second opinion**: scored with butteraugli instead, which the search never looks at, pngdeer's files are still 6–9% smaller than TinyPNG's at the same quality.
- TinyPNG ran through its API in October 2026. [bench/](bench/) reruns everything; the website screenshots in the sets aren't ours to share, so a rerun covers 155 of the 180 images.

### WebP

pngdeer also writes WebP (an output name ending in `.webp`, or the switch in the browser): the smaller of its palette image stored losslessly and the lowest lossy quality that keeps the target. Against TinyPNG's WebP on 120 of the images, the size each tool needs to match TinyPNG's quality (median image / in total), and how many images it can bring up to that quality:

| Compressor | Size at TinyPNG's quality | Reaches |
|---|---:|---:|
| **pngdeer** | **95.0% / 93.9%** | **118** |
| TinyPNG (the yardstick) | 100% | 120 |
| cwebp, best of 27 settings | 97.9% / 110.2% | 120 |
| pngquant + cwebp, best of 10 | 135.0% / 212.1% | 115 |

- **cwebp** is libwebp's own encoder, the library Squoosh, sharp and Pillow encode with. It gets its best of 27 settings per image, picked after the fact: 13 qualities with and without the auto filter, and lossless. **pngquant + cwebp** stores pngquant's palette image as lossless WebP, TinyPNG's palette route from open-source parts.
- At TinyPNG's file size, on the 40 images where all three can be compared, pngdeer scores 2.6 SSIMULACRA2 points above TinyPNG, pngquant + cwebp 1.9, cwebp −0.3.
- Out of the box, cwebp's default quality makes files 6% bigger than TinyPNG's on the median image, and slightly worse. pngdeer's automatic quality makes them 5% smaller, as good.
- WebP's settings were chosen on these 120 images. On the other 60, against a stand-in that matches TinyPNG's WebP on these (same size and score, median), pngdeer is 4% smaller. The [WebP notes](experiments/webp/FINDINGS.md) have the details.

## How it works

- **Searches the palette**: palette sizes from 2 to 256 colors, each tried a few ways (with and without Floyd–Steinberg dithering, with neighbour smoothing, with a second palette builder), and the smallest file that reaches the target wins.
- **Scores what you'd see**: SSIMULACRA2, measured in-process. Colors are matched in Oklab, smooth gradients are always dithered, and transparent pixels are judged over light and dark backgrounds.
- **Stays fast on big images**: large images are searched on a downscaled preview and checked at full size.

The [research notes](experiments/palette-feedback/FINDINGS.md) follow every step, from the first comparison with TinyPNG to today's defaults.

## Command line

```bash
pngdeer input.png -o output.png              # automatic quality
pngdeer input.png -o output.png --target 85  # a fixed SSIMULACRA2 quality
pngdeer input.png -o output.webp             # WebP, automatic quality
pngdeer --help
```

The output filename selects WebP when it ends in `.webp`; otherwise it selects PNG. Use `--format png` or `--format webp` to set the format explicitly.

Input images must be PNG. Animated PNGs stay animated with PNG output; APNG-to-WebP conversion and WebP input are not supported.

From source: `cargo build --release`, and the binary is `target/release/pngdeer`.

## Browser

The [web app](web/) runs the same code compiled to WebAssembly. To run it locally ([web/build.sh](web/build.sh) lists the toolchain it needs):

```bash
sh web/build.sh
python3 web/serve.py   # http://localhost:8770
```

## License

[MIT](LICENSE). Dependencies keep their own licenses ([web/THIRD_PARTY_LICENSES.txt](web/THIRD_PARTY_LICENSES.txt)), as do the test and sample images (the `SOURCES.md` next to them).
