// pngdeer: wasm32 has no C library; libwebp is built with NDEBUG, so asserts are no-ops anyway.
#define assert(condition) ((void)0)
