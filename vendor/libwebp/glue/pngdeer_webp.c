// pngdeer's entry points into libwebp. They take plain values rather than libwebp's structs, so
// the Rust side (src/lib.rs) declares no struct layouts of libwebp's.

#include <string.h>

#include "src/webp/decode.h"
#include "src/webp/encode.h"

// Mirrored by `Options` in src/lib.rs.
typedef struct {
  int lossless;           // 1: lossless at `level`; 0: lossy at `quality` and `method`
  int level;              // 0-9, as cwebp -z
  float quality;          // 0-100
  int method;             // 0-6
  int sharp_yuv;          // lossy: the slower, sharper RGB to YUV conversion
  int alpha_quality;      // lossy: 0-100, 100 keeps the alpha plane lossless
  int alpha_filtering;    // lossy: 0-2
  int alpha_compression;  // lossy: 0 stores the alpha plane as it is, 1 compresses it
  int exact;              // keep the RGB of fully transparent pixels
  // libwebp's finer settings (WebPConfig's), -1 for its default
  int segments;           // lossy: 1-4
  int sns_strength;       // lossy: spatial noise shaping, 0-100
  int filter_strength;    // lossy: 0-100
  int filter_sharpness;   // lossy: 0-7
  int filter_type;        // lossy: 0 simple, 1 strong
  int autofilter;         // lossy: 0-1
  int preprocessing;      // lossy: 0-7
  int near_lossless;      // lossless: 0-100, 100 is off
} PngdeerWebpOptions;

// A WebPConfig from the options; 0 if libwebp rejects them.
static int Configure(WebPConfig* config, const PngdeerWebpOptions* options) {
  if (!WebPConfigInit(config)) return 0;
  if (options->lossless) {
    if (!WebPConfigLosslessPreset(config, options->level)) return 0;
  } else {
    config->quality = options->quality;
    config->method = options->method;
  }
  config->use_sharp_yuv = options->sharp_yuv;
  config->alpha_quality = options->alpha_quality;
  config->alpha_filtering = options->alpha_filtering;
  config->alpha_compression = options->alpha_compression;
  config->exact = options->exact;
  config->thread_level = 0;
  if (options->segments >= 0) config->segments = options->segments;
  if (options->sns_strength >= 0) config->sns_strength = options->sns_strength;
  if (options->filter_strength >= 0) config->filter_strength = options->filter_strength;
  if (options->filter_sharpness >= 0) config->filter_sharpness = options->filter_sharpness;
  if (options->filter_type >= 0) config->filter_type = options->filter_type;
  if (options->autofilter >= 0) config->autofilter = options->autofilter;
  if (options->preprocessing >= 0) config->preprocessing = options->preprocessing;
  if (options->near_lossless >= 0) config->near_lossless = options->near_lossless;
  return WebPValidateConfig(config);
}

// Encodes `picture` (freeing it) into memory; see pngdeer_webp_encode.
static size_t Encode(const WebPConfig* config, WebPPicture* picture,
                     WebPMemoryWriter* writer, uint8_t** out, int* error) {
  WebPMemoryWriterInit(writer);
  picture->writer = WebPMemoryWrite;
  picture->custom_ptr = writer;
  if (!WebPEncode(config, picture)) {
    *error = picture->error_code;
    WebPPictureFree(picture);
    WebPMemoryWriterClear(writer);
    return 0;
  }
  WebPPictureFree(picture);
  *out = writer->mem;
  return writer->size;
}

// Encodes `rgba` (width * 4 bytes a row). Returns the file's size and points `*out` at it (to free
// with pngdeer_webp_free), or returns 0 and sets `*error` to a WebPEncodingError (-1: bad options).
size_t pngdeer_webp_encode(const uint8_t* rgba, int width, int height,
                           const PngdeerWebpOptions* options, uint8_t** out,
                           int* error) {
  WebPConfig config;
  WebPPicture picture;
  WebPMemoryWriter writer;
  *out = NULL;
  *error = 0;
  if (!Configure(&config, options) || !WebPPictureInit(&picture)) {
    *error = -1;
    return 0;
  }
  // ARGB in, as cwebp does: WebPEncode converts it to YUV itself, with sharp YUV if asked
  picture.use_argb = 1;
  picture.width = width;
  picture.height = height;
  if (!WebPPictureImportRGBA(&picture, rgba, width * 4)) {
    *error = picture.error_code;
    WebPPictureFree(&picture);
    return 0;
  }
  return Encode(&config, &picture, &writer, out, error);
}

// An image converted to YUV once, for any number of lossy encodes (pngdeer_webp_source_encode):
// WebPEncode converts a picture that has no YUV samples yet, then works on its YUV samples alone, so
// encoding a copy of this one is the same as encoding the RGBA with the same conversion. NULL on
// failure, with `*error` set.
WebPPicture* pngdeer_webp_source_new(const uint8_t* rgba, int width, int height,
                                     int sharp_yuv, int* error) {
  WebPPicture picture;
  WebPPicture* source = (WebPPicture*)WebPMalloc(sizeof(*source));
  *error = 0;
  if (source == NULL || !WebPPictureInit(&picture) || !WebPPictureInit(source)) {
    WebPFree(source);
    *error = 1;
    return NULL;
  }
  picture.use_argb = 1;
  picture.width = width;
  picture.height = height;
  // as WebPEncode converts it (no dithering: pngdeer sets no preprocessing)
  if (!WebPPictureImportRGBA(&picture, rgba, width * 4) ||
      !(sharp_yuv ? WebPPictureSharpARGBToYUVA(&picture)
                  : WebPPictureARGBToYUVADithered(&picture, WEBP_YUV420, 0.f)) ||
      !WebPPictureCopy(&picture, source)) {  // the YUV samples only
    *error = picture.error_code ? picture.error_code : 1;
    WebPPictureFree(&picture);
    WebPFree(source);
    return NULL;
  }
  WebPPictureFree(&picture);
  return source;
}

// pngdeer_webp_encode for a source's picture (lossy only; `options->sharp_yuv` must be the
// source's). Safe to call from several threads at once on one source: it is only read.
size_t pngdeer_webp_source_encode(const WebPPicture* source,
                                  const PngdeerWebpOptions* options, uint8_t** out,
                                  int* error) {
  WebPConfig config;
  WebPPicture picture;
  WebPMemoryWriter writer;
  *out = NULL;
  *error = 0;
  if (options->lossless || !Configure(&config, options)) {
    *error = -1;
    return 0;
  }
  if (!WebPPictureCopy(source, &picture)) {
    *error = 1;
    return 0;
  }
  return Encode(&config, &picture, &writer, out, error);
}

void pngdeer_webp_source_free(WebPPicture* source) {
  WebPPictureFree(source);
  WebPFree(source);
}

// A WebP file as RGBA (width * 4 bytes a row, fancy upsampling as browsers decode it), to free with
// pngdeer_webp_free; NULL if it can't be decoded.
uint8_t* pngdeer_webp_decode(const uint8_t* data, size_t size, int* width,
                             int* height) {
  return WebPDecodeRGBA(data, size, width, height);
}

void pngdeer_webp_free(void* pointer) { WebPFree(pointer); }
