// pngdeer: wasm32 has no C library; the vendored libraries only need the fixed-width integer types,
// and libaom the format macros for the messages and files it never writes here.
#include <stdint.h>
#define PRId32 "d"
#define PRIu32 "u"
#define PRIx32 "x"
#define PRId64 "lld"
#define PRIu64 "llu"
#define PRIx64 "llx"
