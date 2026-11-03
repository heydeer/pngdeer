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
- `generic/config/`, `arm64/config/` and `wasm/config/`: the headers libaom's CMake generates
  (`aom_config.h`, the run-time CPU dispatch headers `aom_dsp_rtcd.h`, `av1_rtcd.h` and
  `aom_scale_rtcd.h`, and `aom_version.h`), for `-DAOM_TARGET_CPU=generic` and for `-DAOM_TARGET_CPU=arm64
  -DCONFIG_EXCLUDE_SIMD_MISMATCH=1 -DENABLE_NEON_DOTPROD=0 -DENABLE_NEON_I8MM=0 -DENABLE_SVE=0
  -DENABLE_SVE2=0`, both with `-DCONFIG_AV1_DECODER=0 -DCONFIG_MULTITHREAD=0
  -DCONFIG_RUNTIME_CPU_DETECT=0 -DCONFIG_WEBM_IO=0`, the dispatch from pngdeer.patch's
  `*_rtcd_defs.pl`. Then set by hand for every target: `CONFIG_MULTITHREAD` to 1, and
  `HAVE_PTHREAD_H` (only `aom_once`'s lock), `HAVE_UNISTD_H` and `CONFIG_OS_SUPPORT` (only the
  encoder's timers, which change nothing) to 0. `wasm/config` is
  arm64's with ARM's CRC32 off and every NEON function back in C but the 171 that round 7 of
  experiments/avif/FINDINGS.md found 10-bit encodes spending time in (its `simd.json`);
  `wasm/hot.txt` lists the 87 files that took at least 0.1% of an encode there (its `files.json`).
- `simde/`: the headers of SIMDe 0.8.2 (github.com/simd-everywhere/simde, MIT: `simde/COPYING`)
  that libaom's NEON files include, unmodified, and an `arm_neon.h` of pngdeer's that includes them
  in place of the compiler's: SIMDe implements NEON's intrinsics on wasm's SIMD128.
- `glue/pngdeer_aom.c`: one entry point with plain arguments that encodes a still image (its color,
  or an alpha plane) the way libavif's `codec_aom.c` does, and hands back the frame as decoded;
  `glue/aom_config.c` stands in for CMake's.
- `src/lib.rs`: the Rust API (`encode`, `Options`), which checks what libaom would refuse, and sets
  up libaom's tables once (`av1_initialize_enc`, which libaom runs through `aom_once`, unlocked
  without `HAVE_PTHREAD_H`) before encoders can run on several threads.
- `src/workers.rs`: libaom's worker interface (`aom_util/aom_thread.h`) on rayon's threads, in place
  of libaom's own (see 4 below).

The C library functions libaom calls are `vendor/clib`'s, shared with `vendor/libwebp` and
`vendor/mozjpeg`.

## How it is built (build.rs)

The same bytes on every target:

1. **SIMD only where it agrees with the C.** 64-bit ARM, and wasm32 through SIMDe, get libaom's
   NEON code with
   `CONFIG_EXCLUDE_SIMD_MISMATCH` (which leaves out the floating-point neural networks' SIMD) and
   pngdeer.patch, which leaves out `aom_hadamard_16x16`'s and `av1_quantize_fp`'s: from speed 8,
   all-intra calls these 8-bit functions on 10-bit coefficients, and their SIMD truncates where the
   C doesn't (a libaom bug). Every other target runs the plain C code. On wasm32 only the NEON
   functions that matter are kept, and the files that matter built at -O2, the rest at -Os, so the
   module is about as big as plain C's (1.8 MB gzipped with the rest of pngdeer, 0.09 MB more).
   The compiler matters there: Amazon Linux's clang 15 builds SIMDe's NEON into code that writes
   other files (its plain C writes the right ones), LLVM 23 doesn't; the site is built with
   wasi-sdk 34's (web/vercel-build.sh), and web/check-avif.mjs, which web/build.sh runs last,
   refuses a build whose file differs from the command line's.
2. **No fused multiply-add.** `-ffp-contract=off`: compilers fuse `a * b + c` on targets with FMA,
   such as arm64, and libaom's rate control and neural networks work in floating point.
3. **One libm and one qsort.** `vendor/clib/include/pngdeer_clib.h`, included ahead of every file,
   sends the inexact math functions (libaom calls `exp`, `exp2`, `log`, `log1p`, `log1pf`, `log2`,
   `log10`, `pow`, `expf`, `tanh` and `cos`) to Rust's `libm` crate, and `qsort` to a stable sort.
4. **Threads, but never one.** libaom's encoder with one thread and with several write different
   files, but with any number from 2 the same one: it hands its rows out to its workers as they ask
   for them (row multithreading), and what one worker learns doesn't carry to the next row. So
   every encoder has at least 2 workers (as many as rayon has threads), and `src/workers.rs` runs
   them as jobs on rayon's threads, which whichever thread is free takes, or the encoder's own when
   it waits for one nobody has started: encoders running at once never wait on each other for a
   thread, and the browser, which has no threads of its own to give libaom, runs them on rayon's
   Web Workers. Without rayon (the browser's single-threaded build), the workers run one after
   another, one doing all the rows, and the file is the same. On wasm32, vendor/clib's `pthread.h`
   gives libaom its locks: futexes on wasm's atomic wait and notify, or nothing where there is one
   thread.
5. **wasm32 has no C library.** `vendor/clib`'s headers and functions stand in for one: libaom's
   stdio (its messages, statistics and option files, never used here), and `setjmp`, which returns
   0, with `longjmp` aborting (libaom jumps only on an error, which pngdeer's checks leave to running
   out of memory).
6. **Optimized in any profile.** libaom at `-O0` is tens of times slower, and its bytes don't
   depend on the level.

Checked: with the encoder settings libavif and aomenc use for stills (all-intra, tune=iq), the frame
data is byte for byte that of aomenc 3.15.2 built as plain C by its CMake (which wrote the same
bytes on macOS arm64, Linux arm64 and x86-64, and wasm32 in experiments/avif round 5): 38 encodes of
6 images at speeds 4, 6 and 8, 8- and 10-bit, from both this crate's plain C and its NEON build,
which also matched on a 12 MP photo at speeds 6 and 8, where libaom's own NEON differs. pngdeer's
`tests/avif.rs` pins the bytes of three whole compressions, which macOS arm64 (clang, NEON), Linux
arm64 (GCC 12, NEON) and Linux x86-64 (GCC 12, plain C) reproduce, and the browser build (wasm32,
SIMD128) writes the command line's files.
