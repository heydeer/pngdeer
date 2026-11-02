#!/bin/sh
# Vercel's build (vercel.json), on Amazon Linux 2023 as root: installs what web/build.sh needs,
# builds both WebAssembly packages, and copies the site into dist/ with the sample images' links
# resolved (they point into bench/images/, outside the site).
set -e
cd "$(dirname "$0")/.."

# clang and llvm-ar that target wasm32, for the C code (libdeflate, libwebp, mozjpeg, libaom): wasi-sdk's,
# LLVM 23 as on a Mac with Homebrew's. Amazon Linux's own, clang 15, builds libaom's SIMD (through SIMDe)
# into code that writes other AVIF files than the command line (build.sh checks).
wasi=34
curl --proto '=https' -sSfL "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-$wasi/wasi-sdk-$wasi.0-x86_64-linux.tar.gz" | tar -xz -C /opt
# The image has rustup in /rust, set to an older Rust: use current stable, as CI does.
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
  PATH="$HOME/.cargo/bin:$PATH"
fi
rustup toolchain install stable --profile minimal --target wasm32-unknown-unknown
rustup default stable
rustup toolchain install nightly --profile minimal --component rust-src --target wasm32-unknown-unknown
# wasm-bindgen-cli's release build, at the version web/wasm/Cargo.toml pins, next to cargo
version=$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' web/wasm/Cargo.toml)
tmp=$(mktemp -d)
curl --proto '=https' -sSfL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$version/wasm-bindgen-$version-x86_64-unknown-linux-musl.tar.gz" | tar -xz -C "$tmp"
cp "$tmp"/*/wasm-bindgen "$(dirname "$(command -v cargo)")/"

LLVM_BIN=/opt/wasi-sdk-$wasi.0-x86_64-linux/bin sh web/build.sh

rm -rf dist
mkdir dist
cp -RL web/index.html web/worker.js web/THIRD_PARTY_LICENSES.txt web/brand web/samples web/pkg web/pkg-mt web/pkg-avif web/pkg-avif-mt dist/
