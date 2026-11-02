/* pngdeer: libaom's NEON code on wasm32's SIMD128, for the browser build (../build.rs): SIMDe implements
 * the NEON intrinsics, and this stands in for the compiler's arm_neon.h. */
#define SIMDE_FLOAT16_API 1 /* SIMDE_FLOAT16_API_PORTABLE: wasm32 has no _Float16 */
#define SIMDE_ENABLE_NATIVE_ALIASES
#include <simde/arm/neon.h>
