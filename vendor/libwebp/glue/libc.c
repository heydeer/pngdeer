// pngdeer: the few C library functions libwebp needs on wasm32 that neither Rust's
// compiler-builtins nor src/lib.rs provides.
#include <stddef.h>

void abort(void) { __builtin_trap(); }

char* getenv(const char* name) {
  (void)name;
  return NULL;
}

int abs(int x) { return x < 0 ? -x : x; }
