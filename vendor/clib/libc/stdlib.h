// slimdeer: wasm32 has no C library. The allocator is Rust's (src/lib.rs); abort, exit, getenv, abs,
// llabs, strtol, strtoul and rand are in libc.c; qsort goes to slimdeer_qsort (include/slimdeer_clib.h).
#ifndef SLIMDEER_STDLIB_H_
#define SLIMDEER_STDLIB_H_
#include <stddef.h>
void* malloc(size_t size);
void* calloc(size_t count, size_t size);
void* realloc(void* pointer, size_t size);
void free(void* pointer);
void abort(void);
void exit(int status);
char* getenv(const char* name);
int abs(int x);
long long llabs(long long x);
long strtol(const char* text, char** end, int base);
unsigned long strtoul(const char* text, char** end, int base);
int rand(void);
void qsort(void* base, size_t count, size_t size, int (*compare)(const void*, const void*));
#endif
