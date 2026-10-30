// pngdeer: wasm32 has no C library. The exact operations are wasm instructions; log, log2, log10,
// exp, pow, logf and expf go to Rust (include/pngdeer_clib.h) as on every other target.
#ifndef PNGDEER_MATH_H_
#define PNGDEER_MATH_H_
#define sqrt(x) __builtin_sqrt(x)
#define sqrtf(x) __builtin_sqrtf(x)
#define floor(x) __builtin_floor(x)
#define ceil(x) __builtin_ceil(x)
#define fabs(x) __builtin_fabs(x)
#define round(x) __builtin_round(x)
#define ceilf(x) __builtin_ceilf(x)
#define floorf(x) __builtin_floorf(x)
#define fabsf(x) __builtin_fabsf(x)
#define roundf(x) __builtin_roundf(x)
double log(double x);
double log2(double x);
double log10(double x);
double exp(double x);
double pow(double x, double y);
float logf(float x);
float expf(float x);
#endif
