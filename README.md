# pngdeer

Lossy PNG compression that picks the quality for each image. The command line and the browser run the same Rust code and write the same bytes.

## Command line

```bash
brew install heydeer/tap/pngdeer
pngdeer input.png -o output.png
```

Add `--target 85` for a fixed SSIMULACRA2 quality instead of the automatic one; `--help` lists the rest. From source: `cargo build --release`.

## Browser

The [web app](web/) compresses on the device with WebAssembly; nothing is uploaded. [web/build.sh](web/build.sh) lists the toolchain it needs.

```bash
sh web/build.sh
python3 web/serve.py   # http://localhost:8770
```

## More

[Benchmarks](bench/) and [research notes](experiments/palette-feedback/FINDINGS.md).

## License

[MIT](LICENSE). Dependencies keep their own licenses ([web/THIRD_PARTY_LICENSES.txt](web/THIRD_PARTY_LICENSES.txt)), as do the test and sample images (the `SOURCES.md` next to them).
