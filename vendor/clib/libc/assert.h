// pngdeer: wasm32 has no C library; the vendored libraries are built with NDEBUG, so asserts are no-ops anyway.
#define assert(condition) ((void)0)
// C11's name for _Static_assert, which libaom uses
#define static_assert _Static_assert
