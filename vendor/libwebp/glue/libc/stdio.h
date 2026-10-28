// pngdeer: wasm32 has no C library. libwebp prints only from debugging code that isn't built.
#ifndef PNGDEER_STDIO_H_
#define PNGDEER_STDIO_H_
typedef struct FILE FILE;
extern FILE* stderr;
int printf(const char* format, ...);
int fprintf(FILE* file, const char* format, ...);
#endif
