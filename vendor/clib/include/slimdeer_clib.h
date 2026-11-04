// slimdeer: included ahead of every file of the vendored C libraries (their build.rs). Their calls to
// the C library's inexact math functions, and to qsort, go to one implementation on every target
// (src/lib.rs, with the libm crate): system math libraries differ in the last bits, and qsort may
// order equal elements either way, so the command line and the browser build would otherwise write
// different files. sqrt, floor, ceil, fabs and round are exact everywhere and stay as they are.
#ifndef SLIMDEER_CLIB_H_
#define SLIMDEER_CLIB_H_

#include <math.h>
#include <stddef.h>
#include <stdlib.h>

double slimdeer_log(double x);
double slimdeer_log2(double x);
double slimdeer_log10(double x);
double slimdeer_exp(double x);
double slimdeer_pow(double x, double y);
float slimdeer_logf(float x);
float slimdeer_expf(float x);
double slimdeer_exp2(double x);
double slimdeer_log1p(double x);
double slimdeer_tanh(double x);
double slimdeer_cos(double x);
float slimdeer_log1pf(float x);
void slimdeer_qsort(void* base, size_t count, size_t size,
                   int (*compare)(const void*, const void*));

#define log slimdeer_log
#define log2 slimdeer_log2
#define log10 slimdeer_log10
#define exp slimdeer_exp
#define pow slimdeer_pow
#define logf slimdeer_logf
#define expf slimdeer_expf
#define exp2 slimdeer_exp2
#define log1p slimdeer_log1p
#define tanh slimdeer_tanh
#define cos slimdeer_cos
#define log1pf slimdeer_log1pf
#define qsort slimdeer_qsort

#endif  // SLIMDEER_CLIB_H_
