# pngdeer

**Smaller PNGs. Same picture.** pngdeer is a lossy PNG compressor with no quality slider: for each image it searches palette sizes from 2 to 256 colors, with and without dithering, scores each result with a perceptual metric, and keeps the smallest file that still looks right. At the same quality, the median image comes out **8–12% smaller than with TinyPNG**, the best result of any open-source compressor we've measured.

**[Try it in your browser →](https://heydeer.ai/open-source/pngdeer/)** Nothing is uploaded: images are compressed on your device.

[![pngdeer in the browser](.github/pngdeer.png)](https://heydeer.ai/open-source/pngdeer/)

```bash
brew install heydeer/tap/pngdeer
pngdeer input.png -o output.png
```

- **Picks the quality for you**: about what TinyPNG would give the image, never above 90 on SSIMULACRA2's scale, which is indistinguishable at 1:1.
- **Same bytes everywhere**: the command line (macOS and Linux) and the browser (WebAssembly, on every core) run the same Rust code and write identical files.
- **Big images too**: up to 40 megapixels in the browser, more on the command line.
- **Free and open source**: MIT.

## Leaderboard

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

## How it works

- **Searches the palette**: palette sizes from 2 to 256 colors, each tried a few ways (with and without Floyd–Steinberg dithering, with neighbour smoothing, with a second palette builder), and the smallest file that reaches the target wins.
- **Scores what you'd see**: SSIMULACRA2, measured in-process. Colors are matched in Oklab, smooth gradients are always dithered, and transparent pixels are judged over light and dark backgrounds.
- **Stays fast on big images**: large images are searched on a downscaled preview and checked at full size.

The [research notes](experiments/palette-feedback/FINDINGS.md) follow every step, from the first comparison with TinyPNG to today's defaults.

## Command line

```bash
pngdeer input.png -o output.png              # automatic quality
pngdeer input.png -o output.png --target 85  # a fixed SSIMULACRA2 quality
pngdeer --help
```

From source: `cargo build --release`, and the binary is `target/release/pngdeer`.

## Browser

The [web app](web/) runs the same code compiled to WebAssembly. To run it locally ([web/build.sh](web/build.sh) lists the toolchain it needs):

```bash
sh web/build.sh
python3 web/serve.py   # http://localhost:8770
```

## License

[MIT](LICENSE). Dependencies keep their own licenses ([web/THIRD_PARTY_LICENSES.txt](web/THIRD_PARTY_LICENSES.txt)), as do the test and sample images (the `SOURCES.md` next to them).
