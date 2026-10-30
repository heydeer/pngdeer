# mozjpeg 4.1.5's encoder, built for pngdeer

pngdeer's JPEG output (`pngdeer::compress_jpeg`, `src/jpeg.rs`) encodes with mozjpeg, Mozilla's
JPEG encoder built on libjpeg-turbo (the IJG license and BSD-3-Clause: `mozjpeg/LICENSE.md`,
`mozjpeg/README.ijg`). This software is based in part on the work of the Independent JPEG Group.
This crate builds the encoder from source so that every target, the browser's WebAssembly included,
writes the same bytes for the same input, as pngdeer's PNG and WebP output do. Decoding is
zune-jpeg's (pure Rust, see `src/jpeg.rs`): libjpeg ends on corrupt input through a longjmp, which
the browser build can't make.

## What is here

- `mozjpeg/`: from mozjpeg 4.1.5 (github.com/mozilla/mozjpeg, tag v4.1.5; the tag's archive's
  SHA-256 is 9fcbb7171f6ac383f5b391175d6fb3acde5e64c4c4727274eade84ed0998fcc1), unmodified: the
  encoder's sources and headers (`jc*.c`, `jfdct*.c`, `jdatadst.c`, the shared `jcomapi.c`,
  `jerror.c`, `jmemmgr.c`, `jmemnobs.c`, `jutils.c`, `jsimd_none.c`, `jstdhuff.c`), and LICENSE.md
  and README.ijg. Left out: the decoder, the SIMD code, arithmetic coding, the TurboJPEG API, the
  tools (cjpeg and the rest) and the build files.
- `config/`: `jconfig.h`, `jconfigint.h` and `jversion.h` as mozjpeg's CMake build generates them,
  fixed for every target: 8-bit samples, the in-memory destination, no arithmetic coding, no SIMD.
- `glue/pngdeer_jpeg.c`: one entry point with plain arguments (pixels, quality, base table, 4:2:0
  or 4:4:4, progressive or baseline, an ICC profile and an EXIF block to embed), so the Rust side
  declares none of libjpeg's structs. libjpeg ends on any error through `error_exit`, which here
  aborts: encoding fails only on arguments `src/lib.rs` has already checked, or out of memory.
- `src/lib.rs`: the Rust API (`encode`, `Options`), which checks what libjpeg would end on.

## How it is built (build.rs)

The same way on every target, for the same output everywhere:

1. **No SIMD.** `jsimd_none.c` stands in for libjpeg-turbo's SIMD dispatch, so x86, ARM and
   WebAssembly all run the plain C code.
2. **No fused multiply-add.** `-ffp-contract=off`: clang fuses `a * b + c` on targets with FMA, such
   as arm64, and mozjpeg's trellis quantization works in floating point.
3. **One libm.** `vendor/clib/include/pngdeer_clib.h`, included ahead of every file, sends the
   inexact math functions to Rust's `libm` crate (see `vendor/clib`), shared with `vendor/libwebp`.
4. **No environment.** `NO_GETENV` and `NO_PUTENV`: libjpeg reads JPEGMEM and the like, which could
   change a file.
5. **wasm32 has no C library.** `vendor/clib`'s headers and functions stand in for one.

Checked: with mozjpeg's quantization table 2, 4:2:0 and progressive scans, the files are byte for
byte cjpeg's from the same tag built with `-ffp-contract=off` (12 photos at qualities 60, 70 and
75); encodes hash the same natively (arm64) and as wasm32; and pngdeer's `tests/jpeg.rs` pins the
bytes of three whole compressions, which a wasm32 build reproduces.
