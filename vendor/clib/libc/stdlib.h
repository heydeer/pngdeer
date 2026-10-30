// pngdeer: wasm32 has no C library. The allocator is Rust's (src/lib.rs); abort, exit, getenv and
// abs are in libc.c; qsort goes to pngdeer_qsort (include/pngdeer_clib.h).
#ifndef PNGDEER_STDLIB_H_
#define PNGDEER_STDLIB_H_
#include <stddef.h>
void* malloc(size_t size);
void* calloc(size_t count, size_t size);
void* realloc(void* pointer, size_t size);
void free(void* pointer);
void abort(void);
void exit(int status);
char* getenv(const char* name);
int abs(int x);
void qsort(void* base, size_t count, size_t size, int (*compare)(const void*, const void*));
#endif
