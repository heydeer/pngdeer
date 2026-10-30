/* pngdeer: mozjpeg 4.1.5's jconfigint.h as its CMake build generates it, with the sizes taken from
 * the compiler's own macros so that it holds for every target (arm64, x86-64, wasm32). */

#define BUILD  "pngdeer"

#undef inline
#define INLINE  __inline__ __attribute__((always_inline))
#define THREAD_LOCAL  __thread

#define PACKAGE_NAME  "mozjpeg"
#define VERSION  "4.1.5"

#define SIZEOF_SIZE_T  __SIZEOF_SIZE_T__

/* __builtin_ctzl counts in an unsigned long, which must be as wide as size_t */
#if __SIZEOF_LONG__ == __SIZEOF_SIZE_T__
#define HAVE_BUILTIN_CTZL
#endif

#if defined(__has_attribute)
#if __has_attribute(fallthrough)
#define FALLTHROUGH  __attribute__((fallthrough));
#else
#define FALLTHROUGH
#endif
#else
#define FALLTHROUGH
#endif
