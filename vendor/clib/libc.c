// pngdeer: the few C library functions the vendored libraries need on wasm32 that neither Rust's
// compiler-builtins nor src/lib.rs provides. Messages go nowhere: pngdeer reports errors itself.
#include <stddef.h>

typedef struct FILE FILE;
FILE* stderr = NULL;
int errno = 0;

void abort(void) { __builtin_trap(); }
void exit(int status) {
  (void)status;
  __builtin_trap();
}

char* getenv(const char* name) {
  (void)name;
  return NULL;
}

int abs(int x) { return x < 0 ? -x : x; }

int printf(const char* format, ...) {
  (void)format;
  return 0;
}
int fprintf(FILE* file, const char* format, ...) {
  (void)file, (void)format;
  return 0;
}
int snprintf(char* buffer, size_t size, const char* format, ...) {
  (void)format;
  if (size > 0) buffer[0] = 0;
  return 0;
}
int sscanf(const char* text, const char* format, ...) {
  (void)text, (void)format;
  return 0;
}
size_t fwrite(const void* data, size_t size, size_t count, FILE* file) {
  (void)data, (void)size, (void)count, (void)file;
  return 0;
}
size_t fread(void* data, size_t size, size_t count, FILE* file) {
  (void)data, (void)size, (void)count, (void)file;
  return 0;
}
int fflush(FILE* file) {
  (void)file;
  return 0;
}
int ferror(FILE* file) {
  (void)file;
  return 1;
}

int strcmp(const char* a, const char* b) {
  while (*a && *a == *b) a++, b++;
  return (unsigned char)*a - (unsigned char)*b;
}
char* strncpy(char* to, const char* from, size_t size) {
  size_t i = 0;
  for (; i < size && from[i]; i++) to[i] = from[i];
  for (; i < size; i++) to[i] = 0;
  return to;
}
