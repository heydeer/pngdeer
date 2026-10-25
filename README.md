# pngdeer

Lossy PNG compression with automatic quality selection. Runs on the command line and in the browser using the same Rust core.

## CLI

```bash
brew install heydeer/tap/pngdeer
pngdeer input.png -o output.png
```

To build from source, run `cargo build --release`; the binary is `target/release/pngdeer`.

Quality is automatic by default. Add `--target 85` to set a SSIMULACRA2 quality target. Use `--help` for more options.

## Browser

The [web app](web/) compresses images locally with WebAssembly. Images are never uploaded.

Building requires Rust stable and nightly with the `wasm32-unknown-unknown` target, nightly's `rust-src`, `wasm-bindgen-cli` matching [web/wasm/Cargo.toml](web/wasm/Cargo.toml), and LLVM with WebAssembly support. See [web/build.sh](web/build.sh) for details.

```bash
sh web/build.sh
python3 web/serve.py
```

Open [localhost:8770](http://localhost:8770/).

## Development

See [bench/](bench/) for benchmarks and [research notes](experiments/palette-feedback/FINDINGS.md) for implementation details.

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). See [third-party licenses](web/THIRD_PARTY_LICENSES.txt) for dependencies.
