// slimdeer: the few C library functions the vendored libraries need on wasm32 that neither Rust's
// compiler-builtins nor src/lib.rs provides. Messages go nowhere: slimdeer reports errors itself.
#include <stddef.h>

typedef struct FILE FILE;
FILE* stderr = NULL;
int errno = 0;

void abort(void) { __builtin_trap(); }
// setjmp.h: an error inside libaom, which slimdeer's checks leave to running out of memory
_Noreturn void longjmp(int* env, int value) {
  (void)env, (void)value;
  __builtin_trap();
}
void exit(int status) {
  (void)status;
  __builtin_trap();
}

char* getenv(const char* name) {
  (void)name;
  return NULL;
}

int abs(int x) { return x < 0 ? -x : x; }
long long llabs(long long x) { return x < 0 ? -x : x; }

// libaom's option parsing (decimal, or hexadecimal or octal by prefix with base 0), which slimdeer
// doesn't use; and its film grain's seed, off in slimdeer
unsigned long strtoul(const char* text, char** end, int base) {
  const char* s = text;
  while (*s == ' ' || (*s >= '\t' && *s <= '\r')) s++;
  int negative = *s == '-';
  if (*s == '-' || *s == '+') s++;
  if ((base == 0 || base == 16) && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
    s += 2;
    base = 16;
  } else if (base == 0) {
    base = s[0] == '0' ? 8 : 10;
  }
  unsigned long value = 0;
  for (;; s++) {
    int digit = *s >= '0' && *s <= '9' ? *s - '0' : *s >= 'a' && *s <= 'z' ? *s - 'a' + 10 : *s >= 'A' && *s <= 'Z' ? *s - 'A' + 10 : 99;
    if (digit >= base) break;
    value = value * base + digit;
  }
  if (end) *end = (char*)s;
  return negative ? -value : value;
}
long strtol(const char* text, char** end, int base) { return (long)strtoul(text, end, base); }
int rand(void) { return 0; }

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
typedef __builtin_va_list va_list;
int vsnprintf(char* buffer, size_t size, const char* format, va_list args) {
  (void)format, (void)args;
  if (size > 0) buffer[0] = 0;
  return 0;
}
FILE* fopen(const char* path, const char* mode) {
  (void)path, (void)mode;
  return NULL;
}
int fclose(FILE* file) {
  (void)file;
  return 0;
}
int fgetc(FILE* file) {
  (void)file;
  return -1;
}
int fputc(int c, FILE* file) {
  (void)c, (void)file;
  return -1;
}
int ungetc(int c, FILE* file) {
  (void)c, (void)file;
  return -1;
}
int feof(FILE* file) {
  (void)file;
  return 1;
}
int fseek(FILE* file, long offset, int whence) {
  (void)file, (void)offset, (void)whence;
  return -1;
}
int fscanf(FILE* file, const char* format, ...) {
  (void)file, (void)format;
  return -1;
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
int strncmp(const char* a, const char* b, size_t size) {
  for (; size > 0; a++, b++, size--)
    if (*a != *b || !*a) return (unsigned char)*a - (unsigned char)*b;
  return 0;
}
char* strncpy(char* to, const char* from, size_t size) {
  size_t i = 0;
  for (; i < size && from[i]; i++) to[i] = from[i];
  for (; i < size; i++) to[i] = 0;
  return to;
}
