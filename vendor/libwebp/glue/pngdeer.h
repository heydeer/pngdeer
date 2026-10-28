// pngdeer: included ahead of every libwebp file (build.rs). libwebp's calls to the C library's
// inexact math functions, and to qsort, go to one implementation on every target (src/lib.rs, with
// the libm crate): system math libraries differ in the last bits, and qsort may order equal
// elements either way, so the command line and the browser build would otherwise write different
// files. sqrt, floor, ceil, fabs and round are exact everywhere and stay as they are.
#ifndef PNGDEER_LIBWEBP_GLUE_H_
#define PNGDEER_LIBWEBP_GLUE_H_

#include <math.h>
#include <stddef.h>
#include <stdlib.h>

double pngdeer_log(double x);
double pngdeer_log2(double x);
double pngdeer_log10(double x);
double pngdeer_exp(double x);
double pngdeer_pow(double x, double y);
float pngdeer_logf(float x);
float pngdeer_expf(float x);
void pngdeer_qsort(void* base, size_t count, size_t size,
                   int (*compare)(const void*, const void*));

#define log pngdeer_log
#define log2 pngdeer_log2
#define log10 pngdeer_log10
#define exp pngdeer_exp
#define pow pngdeer_pow
#define logf pngdeer_logf
#define expf pngdeer_expf
#define qsort pngdeer_qsort

#endif  // PNGDEER_LIBWEBP_GLUE_H_
