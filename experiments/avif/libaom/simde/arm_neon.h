/* libaom's NEON code on wasm32 SIMD128: SIMDe implements the NEON intrinsics. */
#define SIMDE_FLOAT16_API 1 /* SIMDE_FLOAT16_API_PORTABLE: wasm32 has no _Float16 */
#define SIMDE_ENABLE_NATIVE_ALIASES
#include <simde/arm/neon.h>
