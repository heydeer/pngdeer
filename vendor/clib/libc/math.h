// slimdeer: wasm32 has no C library. The exact operations are wasm instructions; log, log2, log10,
// exp, pow, logf and expf go to Rust (include/slimdeer_clib.h) as on every other target.
#ifndef SLIMDEER_MATH_H_
#define SLIMDEER_MATH_H_
// wasm evaluates float in float (FLT_EVAL_METHOD 0)
typedef float float_t;
typedef double double_t;
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
#define rint(x) __builtin_rint(x)
#define lrint(x) ((long)__builtin_rint(x))
#define isnan(x) __builtin_isnan(x)
// libaom's few calls never see a NaN, where C's fmax and wasm's f64.max differ
static inline double fmax(double a, double b) { return a > b ? a : b; }
double log(double x);
double log2(double x);
double log10(double x);
double exp(double x);
double pow(double x, double y);
float logf(float x);
float expf(float x);
#endif
