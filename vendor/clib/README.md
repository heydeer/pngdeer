# The C library functions of pngdeer's vendored C code

`vendor/libwebp` and `vendor/mozjpeg` are C, and call the C library. Some of its functions differ
between targets, and wasm32 has no C library at all, so the command line and the browser build would
write different files. This crate gives both libraries the same functions on every target.

- `include/pngdeer_clib.h`: included ahead of every file of both libraries (their build.rs). It sends
  the inexact math functions (log, log2, log10, exp, pow, logf, expf) to Rust's `libm` crate, which
  gives the same bits on every target, and qsort to a stable sort (`src/lib.rs`): system math
  libraries differ in the last bits, and qsort implementations order equal elements differently.
  sqrt, floor, ceil, fabs and round are exact operations and stay as they are.
- `libc/` (wasm32 only): the few headers the libraries include, declarations only, with the exact
  math functions as wasm instructions.
- `libc.c` (wasm32 only): abort, exit, getenv, abs, strcmp, strncpy, `errno`, and the stdio
  functions the libraries reference but never reach in pngdeer's use (printing errors, reading
  files) as stubs. memcpy and friends come from Rust's compiler-builtins.
- `src/lib.rs`: the math functions and the sort, and on wasm32 malloc, calloc, realloc and free on
  Rust's allocator.

The crates that use it link it with `use pngdeer_clib as _;`, and their build scripts find the two
header folders in `DEP_PNGDEER_CLIB_INCLUDE` and `DEP_PNGDEER_CLIB_LIBC` (`links = "pngdeer_clib"`).
