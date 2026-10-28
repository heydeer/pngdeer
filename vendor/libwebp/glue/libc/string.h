// pngdeer: wasm32 has no C library; Rust's compiler-builtins provides these.
#ifndef PNGDEER_STRING_H_
#define PNGDEER_STRING_H_
#include <stddef.h>
void* memcpy(void* to, const void* from, size_t size);
void* memmove(void* to, const void* from, size_t size);
void* memset(void* to, int value, size_t size);
int memcmp(const void* a, const void* b, size_t size);
size_t strlen(const char* s);
int strcmp(const char* a, const char* b);
#endif
