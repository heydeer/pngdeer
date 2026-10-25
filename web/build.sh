#!/bin/sh
# Builds the WebAssembly module and its JS glue from web/wasm, twice: web/pkg single-threaded (works
# anywhere) and web/pkg-mt with a rayon thread pool (needs a cross-origin isolated page: see
# serve.py). Needs rustup's stable and nightly toolchains (nightly with rust-src) with the
# wasm32-unknown-unknown target, and wasm-bindgen-cli at the same version as the wasm-bindgen crate
# in web/wasm/Cargo.toml (see README).
set -e
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"  # rustup's cargo, ahead of a system cargo without wasm32
# oxipng's deflate (libdeflate) is C, so it needs a clang that targets wasm32: Apple's doesn't,
# Homebrew's llvm does (brew install llvm). Set LLVM_BIN to use another one.
LLVM_BIN="${LLVM_BIN:-$(brew --prefix llvm 2>/dev/null)/bin}"
export CC_wasm32_unknown_unknown="$LLVM_BIN/clang" AR_wasm32_unknown_unknown="$LLVM_BIN/llvm-ar"
if [ ! -x "$CC_wasm32_unknown_unknown" ]; then
  echo "no wasm32 clang at $CC_wasm32_unknown_unknown: brew install llvm, or set LLVM_BIN" >&2
  exit 1
fi
# No +simd128: fast-ssim2's wasm SIMD path emulates exact fused multiply-add lane by lane and
# measured ~1.8x slower than its scalar one (one 5 MP score: 6.5 s vs 3.6 s in Node), and scoring
# is most of the time. The scalar path is patched for wasm in vendor/fast-ssim2.
cargo build --release --target wasm32-unknown-unknown -p pngdeer-wasm
wasm-bindgen --target web --no-typescript --out-dir web/pkg target/wasm32-unknown-unknown/release/pngdeer_wasm.wasm

# Threads: shared memory needs atomics in std (rebuilt with build-std, hence nightly) and in the C
# code, an imported shared memory (wasm32's 4 GiB ceiling: a 5 MP screenshot peaks at 1.3 GB), and
# the TLS exports wasm-bindgen's thread start-up uses.
LINK="--shared-memory --import-memory --max-memory=4294967296 --export=__wasm_init_tls --export=__tls_size --export=__tls_align --export=__tls_base"
RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals $(for a in $LINK; do printf -- '-C link-arg=%s ' "$a"; done)" \
CFLAGS_wasm32_unknown_unknown="-matomics -mbulk-memory" \
  cargo +nightly build --release --target wasm32-unknown-unknown -Z build-std=panic_abort,std \
  -p pngdeer-wasm --features threads --target-dir target-mt
wasm-bindgen --target web --no-typescript --out-dir web/pkg-mt target-mt/wasm32-unknown-unknown/release/pngdeer_wasm.wasm
ls -l web/pkg web/pkg-mt
