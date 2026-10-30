/* pngdeer: mozjpeg 4.1.5's jconfig.h as its CMake build generates it, for every target: 8-bit
 * samples, the in-memory destination, and no arithmetic coding (pngdeer writes Huffman-coded
 * files, which every viewer reads) or SIMD. */

#define JPEG_LIB_VERSION  62
#define LIBJPEG_TURBO_VERSION  4.1.5
#define LIBJPEG_TURBO_VERSION_NUMBER  4001005

/* #undef C_ARITH_CODING_SUPPORTED */
/* #undef D_ARITH_CODING_SUPPORTED */
#define MEM_SRCDST_SUPPORTED 1
/* #undef WITH_SIMD */

#define BITS_IN_JSAMPLE  8
/* #undef RIGHT_SHIFT_IS_UNSIGNED */
