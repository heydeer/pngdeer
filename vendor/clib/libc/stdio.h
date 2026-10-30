// pngdeer: wasm32 has no C library. The vendored libraries print only error messages and debugging
// output, and mozjpeg's file destination is never used (pngdeer writes to memory): libc.c's stubs.
#ifndef PNGDEER_STDIO_H_
#define PNGDEER_STDIO_H_
#include <stddef.h>
typedef struct FILE FILE;
extern FILE* stderr;
int printf(const char* format, ...);
int fprintf(FILE* file, const char* format, ...);
int snprintf(char* buffer, size_t size, const char* format, ...);
int sscanf(const char* text, const char* format, ...);
size_t fwrite(const void* data, size_t size, size_t count, FILE* file);
size_t fread(void* data, size_t size, size_t count, FILE* file);
int fflush(FILE* file);
int ferror(FILE* file);
#endif
