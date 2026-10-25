#!/bin/sh
# Vercel's build (vercel.json), on Amazon Linux 2023 as root: installs what web/build.sh needs,
# builds both WebAssembly packages, and copies the site into dist/ with the sample images' links
# resolved (they point into bench/images/, outside the site).
set -e
cd "$(dirname "$0")/.."

# clang and llvm-ar that target wasm32, for oxipng's libdeflate
dnf install -y -q clang llvm
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --target wasm32-unknown-unknown
"$HOME/.cargo/bin/rustup" toolchain install nightly --profile minimal --component rust-src --target wasm32-unknown-unknown
# wasm-bindgen-cli's release build, at the version web/wasm/Cargo.toml pins
version=$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' web/wasm/Cargo.toml)
tmp=$(mktemp -d)
curl --proto '=https' -sSfL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$version/wasm-bindgen-$version-x86_64-unknown-linux-musl.tar.gz" | tar -xz -C "$tmp"
cp "$tmp"/*/wasm-bindgen "$HOME/.cargo/bin/"

LLVM_BIN=/usr/bin sh web/build.sh

rm -rf dist
mkdir dist
cp -RL web/index.html web/worker.js web/THIRD_PARTY_LICENSES.txt web/brand web/samples web/pkg web/pkg-mt dist/
