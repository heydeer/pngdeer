# libaom 3.15.2's AV1 encoder, built for pngdeer

pngdeer's AVIF output (`pngdeer::compress_avif`, `src/avif.rs`) encodes with libaom, the Alliance
for Open Media's AV1 library (BSD-2-Clause with AOM's patent license: `libaom/LICENSE`,
`libaom/PATENTS`). This crate builds its encoder from source so that every target writes the same
bytes for the same input, as pngdeer's other outputs do. Decoding isn't needed: the encoder's own
copy of the frame, with its loop filters applied, is what every AV1 decoder outputs, and pngdeer
scores that (experiments/avif/FINDINGS.md, rounds 5 to 9).

## What is here

- `libaom/`: from the libaom 3.15.2 release tarball (storage.googleapis.com/aom-releases; its
  SHA-256 is 67bb54b245f33ed98600e08269e6139986e48114e14042474ddd8885801dfddc) with
  `experiments/avif/libaom/pngdeer.patch` applied: the C files libaom's CMake compiles into the
  encoder library for the configurations below, the headers they include, the two third-party
  libraries among them (`third_party/fastfeat`, BSD-3-Clause, and `third_party/vector`, MIT, with
  their licenses), and AUTHORS, LICENSE and PATENTS. Left out: the decoder, the x86 and other SIMD
  code, the tools, tests and examples, and the build files.
- `generic/config/` and `arm64/config/`: the headers libaom's CMake generates (`aom_config.h`, the
  run-time CPU dispatch headers `aom_dsp_rtcd.h`, `av1_rtcd.h` and `aom_scale_rtcd.h`, and
  `aom_version.h`), for `-DAOM_TARGET_CPU=generic` and for `-DAOM_TARGET_CPU=arm64
  -DCONFIG_EXCLUDE_SIMD_MISMATCH=1 -DENABLE_NEON_DOTPROD=0 -DENABLE_NEON_I8MM=0 -DENABLE_SVE=0
  -DENABLE_SVE2=0`, both with `-DCONFIG_AV1_DECODER=0 -DCONFIG_MULTITHREAD=0
  -DCONFIG_RUNTIME_CPU_DETECT=0 -DCONFIG_WEBM_IO=0`, the dispatch from pngdeer.patch's
  `*_rtcd_defs.pl`. Then set by hand for every target: `HAVE_PTHREAD_H`, `HAVE_UNISTD_H` and
  `CONFIG_OS_SUPPORT` (only the encoder's timers, which change nothing) to 0.
- `glue/pngdeer_aom.c`: one entry point with plain arguments that encodes a still image (its color,
  or an alpha plane) the way libavif's `codec_aom.c` does, and hands back the frame as decoded;
  `glue/aom_config.c` stands in for CMake's.
- `src/lib.rs`: the Rust API (`encode`, `Options`), which checks what libaom would refuse, and sets
  up libaom's tables once (`av1_initialize_enc`, which libaom runs through an unlocked `aom_once` in
  a build without threads) before encoders can run on several threads.

The C library functions libaom calls are `vendor/clib`'s, shared with `vendor/libwebp` and
`vendor/mozjpeg`.

## How it is built (build.rs)

The same bytes on every target:

1. **SIMD only where it agrees with the C.** 64-bit ARM gets libaom's NEON code with
   `CONFIG_EXCLUDE_SIMD_MISMATCH` (which leaves out the floating-point neural networks' SIMD) and
   pngdeer.patch, which leaves out `aom_hadamard_16x16`'s and `av1_quantize_fp`'s: from speed 8,
   all-intra calls these 8-bit functions on 10-bit coefficients, and their SIMD truncates where the
   C doesn't (a libaom bug). Every other target runs the plain C code.
2. **No fused multiply-add.** `-ffp-contract=off`: compilers fuse `a * b + c` on targets with FMA,
   such as arm64, and libaom's rate control and neural networks work in floating point.
3. **One libm and one qsort.** `vendor/clib/include/pngdeer_clib.h`, included ahead of every file,
   sends the inexact math functions (libaom calls `exp`, `exp2`, `log`, `log1p`, `log1pf`, `log2`,
   `log10`, `pow`, `expf`, `tanh` and `cos`) to Rust's `libm` crate, and `qsort` to a stable sort.
4. **No threads.** `CONFIG_MULTITHREAD=0`; pngdeer encodes several files at once instead, each on
   one thread.
5. **Optimized in any profile.** libaom at `-O0` is tens of times slower, and its bytes don't
   depend on the level.

Checked: with the encoder settings libavif and aomenc use for stills (all-intra, tune=iq), the frame
data is byte for byte that of aomenc 3.15.2 built as plain C by its CMake (which wrote the same
bytes on macOS arm64, Linux arm64 and x86-64, and wasm32 in experiments/avif round 5): 38 encodes of
6 images at speeds 4, 6 and 8, 8- and 10-bit, from both this crate's plain C and its NEON build,
which also matched on a 12 MP photo at speeds 6 and 8, where libaom's own NEON differs. pngdeer's
`tests/avif.rs` pins the bytes of three whole compressions, which macOS arm64 (clang, NEON), Linux
arm64 (GCC 12, NEON) and Linux x86-64 (GCC 12, plain C) reproduce.
