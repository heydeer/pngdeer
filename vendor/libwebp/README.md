# libwebp 1.6.0, built for pngdeer

pngdeer's WebP output (`pngdeer::compress_webp`, `src/webp.rs`) encodes and decodes with libwebp,
Google's WebP library (BSD-3-Clause with a patent grant: `libwebp/COPYING`, `libwebp/PATENTS`). This
crate builds it from source so that every target, the browser's WebAssembly included, writes the
same bytes for the same input, as pngdeer's PNG output does.

## What is here

- `libwebp/`: from the libwebp v1.6.0 release (github.com/webmproject/libwebp, tag v1.6.0; the
  tarball's SHA-256 is 93a852c2b3efafee3723efd4636de855b46f9fe1efddd607e1f42f60fc8f2136), unmodified:
  `src/dec`, `src/dsp`, `src/enc`, `src/utils`, the public headers pngdeer's code includes from
  `src/webp`, `sharpyuv`, and AUTHORS, COPYING and PATENTS. Left out: the SIMD files (`*_sse2.c`,
  `*_sse41.c`, `*_avx2.c`, `*_neon.c`, `*_msa.c`, `*_mips*.c`, never built here), mux, demux, the
  tools and the build files. Added: an empty `src/webp/config.h` (below).
- `glue/pngdeer_webp.c`: entry points with plain arguments (encode, decode, free, and a source
  picture converted to YUV once), so the Rust side declares none of libwebp's structs.
- `src/lib.rs`: the Rust API (`encode_lossy`, `encode_lossless`, `encode_near_lossless`, `decode`;
  `Source`, an image converted to YUV once for any number of lossy encodes, from any number of
  threads; `Tuning`, WebPConfig's finer settings), and the C library functions below. libwebp, built
  without threads, sets up its function tables on first use without a lock, so the first call of
  any of these runs one encode and decode of each kind (`warm_up`) before any could run on several
  threads.
- `glue/pngdeer.h`, `glue/libc/`, `glue/libc.c`: see below.

## How it is built (build.rs)

The same way on every target, for the same output everywhere:

1. **No SIMD.** `HAVE_CONFIG_H` is defined and `config.h` defines none of `WEBP_HAVE_SSE2`, `_SSE41`,
   `_AVX2` or `_NEON`, so x86, ARM and WebAssembly all run libwebp's plain C code. (It also leaves out
   threads, which libwebp needs `WEBP_USE_THREAD` for.)
2. **No fused multiply-add.** `-ffp-contract=off`: clang fuses `a * b + c` on targets with FMA, such
   as arm64, and a fused result can differ in the last bit.
3. **One libm and one qsort.** `glue/pngdeer.h`, included ahead of every file, sends libwebp's
   inexact math functions (log, log2, log10, exp, pow, logf, expf) to Rust's `libm` crate, which
   gives the same bits on every target, and qsort to a stable sort (`src/lib.rs`): system math
   libraries differ in the last bits, and qsort implementations order equal elements differently.
   sqrt, floor, ceil, fabs and round are exact operations and stay as they are.
4. **wasm32 has no C library.** For it, `glue/libc/` holds the few headers libwebp includes
   (declarations only; exact math functions as wasm instructions), malloc, calloc, realloc and free
   are Rust's allocator (`src/lib.rs`), and `glue/libc.c` has abort, getenv and abs. memcpy and
   friends come from Rust's compiler-builtins. `-ffreestanding` keeps clang from expecting more.

Checked: libwebp's output hashes the same natively (arm64) and as wasm32 (lossy at three
qualities and their decodes, lossless at three levels), and pngdeer's `tests/webp.rs` pins the
bytes of four whole compressions, which a wasm32 build reproduces and the release workflow runs on
all four release targets.
Plain C turned out to give the same files as Homebrew's cwebp 1.6.0 with its NEON code (lossy and
lossless, byte for byte on the files compared).
