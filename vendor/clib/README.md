# The C library functions of slimdeer's vendored C code

`vendor/libwebp`, `vendor/mozjpeg` and `vendor/libaom` are C, and call the C library. Some of its
functions differ between targets, and wasm32 has no C library at all, so the command line and the
browser build would write different files. This crate gives the libraries the same functions on
every target.

- `include/slimdeer_clib.h`: included ahead of every file of the libraries (their build.rs). It sends
  the inexact math functions (log, log2, log10, exp, pow, logf, expf, and libaom's exp2, log1p,
  log1pf, tanh and cos) to Rust's `libm` crate, which gives the same bits on every target, and
  qsort to a stable sort (`src/lib.rs`): system math libraries differ in the last bits, and qsort
  implementations order equal elements differently.
  sqrt, floor, ceil, fabs and round are exact operations and stay as they are.
- `libc/` (wasm32 only): the few headers the libraries include, declarations only, with the exact
  math functions as wasm instructions; libaom's `setjmp.h`: setjmp returns 0 and longjmp aborts
  (libaom jumps only on an error, which slimdeer's checks leave to running out of memory); and its
  `pthread.h`: mutexes and condition variables as futexes on wasm's atomic wait and notify in a
  build with atomics, doing nothing in one without (one thread, where libaom's workers run one
  after another and nothing waits), and a `pthread_create` that fails, as libaom's workers run on
  rayon's threads (`vendor/libaom/src/workers.rs`).
- `libc.c` (wasm32 only): abort, exit, getenv, abs, llabs, strcmp, strncmp, strncpy, strtol,
  strtoul, rand, longjmp, `errno`, and the stdio functions the libraries reference but never reach
  in slimdeer's use (printing errors, reading and writing files) as stubs. memcpy and friends come
  from Rust's compiler-builtins.
- `src/lib.rs`: the math functions and the sort, and on wasm32 malloc, calloc, realloc and free on
  Rust's allocator.

The crates that use it link it with `use slimdeer_clib as _;`, and their build scripts find the two
header folders in `DEP_SLIMDEER_CLIB_INCLUDE` and `DEP_SLIMDEER_CLIB_LIBC` (`links = "slimdeer_clib"`).
