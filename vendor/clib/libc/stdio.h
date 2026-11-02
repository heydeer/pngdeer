// pngdeer: wasm32 has no C library. The vendored libraries print only error messages and debugging
// output, and mozjpeg's file destination and libaom's files are never used (pngdeer writes to
// memory): libc.c's stubs.
#ifndef PNGDEER_STDIO_H_
#define PNGDEER_STDIO_H_
#include <stdarg.h>
#include <stddef.h>
typedef struct FILE FILE;
extern FILE* stderr;
int printf(const char* format, ...);
int fprintf(FILE* file, const char* format, ...);
int snprintf(char* buffer, size_t size, const char* format, ...);
int vsnprintf(char* buffer, size_t size, const char* format, va_list args);
int sscanf(const char* text, const char* format, ...);
size_t fwrite(const void* data, size_t size, size_t count, FILE* file);
size_t fread(void* data, size_t size, size_t count, FILE* file);
int fflush(FILE* file);
int ferror(FILE* file);
// libaom's statistics and option files, which pngdeer never asks for
#define EOF (-1)
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
FILE* fopen(const char* path, const char* mode);
int fclose(FILE* file);
int fgetc(FILE* file);
int fputc(int c, FILE* file);
int ungetc(int c, FILE* file);
int feof(FILE* file);
int fseek(FILE* file, long offset, int whence);
int fscanf(FILE* file, const char* format, ...);
#endif
