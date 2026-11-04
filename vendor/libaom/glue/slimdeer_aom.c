// slimdeer: entry points into libaom's AV1 encoder with plain arguments, so the Rust side
// (src/lib.rs) declares none of libaom's structs. One call encodes one still image (its color, or
// an alpha plane) the way libavif's codec_aom.c encodes a still AVIF (all-intra, one frame, a
// constant quantizer), and hands back the frame as a decoder shows it.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "aom/aom_encoder.h"
#include "aom/aomcx.h"
#include "av1/encoder/encoder.h"

typedef struct {
  int quantizer;  // 0-63, libaom's cq-level; 0 is lossless
  int speed;      // libaom's cpu-used, 0-9
  int tune;       // an aom_tune_metric, or -1 for libaom's default
  int screen;     // libaom's screen content tools (tune-content=screen)
  int threads;    // workers, 2 or more (src/workers.rs)
  // tile rows and columns, log2
  int tile_rows_log2;
  int tile_cols_log2;
  // the sequence header's color description (color only; alpha is full range, unspecified)
  int color_primaries;
  int transfer_characteristics;
  int matrix_coefficients;
} SlimdeerAomOptions;

// libaom sets up its function tables and lookup tables on its first encoder, through aom_once,
// which without HAVE_PTHREAD_H has no lock: src/lib.rs calls this once, before encoders can run on
// several threads.
void slimdeer_aom_init(void) { av1_initialize_enc(ALLINTRA, AOM_Q); }

static int append(uint8_t** out, size_t* size, size_t* capacity, const void* data, size_t n) {
  if (*size + n > *capacity) {
    size_t grown = (*size + n) * 2;
    uint8_t* bigger = realloc(*out, grown);
    if (!bigger) return 0;
    *out = bigger;
    *capacity = grown;
  }
  memcpy(*out + *size, data, n);
  *size += n;
  return 1;
}

// Encodes `count` planes (3: Y, U and V of 4:4:4; 1: an alpha plane, encoded as monochrome) of
// `height` rows of `width` samples of `depth` bits (8 or 10). The AV1 data (the temporal unit
// libaom writes) goes to *out (free it with slimdeer_aom_free), its size to *out_size, and the frame
// as decoded to `decoded` (count × width × height samples). Returns 1, or 0 with an error message
// in `message` (`message_size` bytes at most).
int slimdeer_aom_encode(const uint16_t* const* planes, int count, int width, int height, int depth,
                       const SlimdeerAomOptions* options, uint8_t** out, size_t* out_size,
                       uint16_t* decoded, char* message, size_t message_size) {
  const int alpha = count == 1;
  aom_codec_iface_t* iface = aom_codec_av1_cx();
  struct aom_codec_enc_cfg cfg;
  aom_codec_ctx_t encoder;
  *out = NULL;
  *out_size = 0;
  if (aom_codec_enc_config_default(iface, &cfg, AOM_USAGE_ALL_INTRA) != AOM_CODEC_OK) {
    snprintf(message, message_size, "no all-intra configuration");
    return 0;
  }
  cfg.rc_end_usage = AOM_Q;
  // profile 0: 4:2:0 and monochrome; 1: 4:4:4
  cfg.g_profile = alpha ? 0 : 1;
  cfg.g_bit_depth = depth;
  cfg.g_input_bit_depth = depth;
  cfg.g_w = width;
  cfg.g_h = height;
  // one frame: a still picture with the reduced sequence header
  cfg.g_limit = 1;
  cfg.monochrome = alpha;
  cfg.rc_min_quantizer = 0;
  cfg.rc_max_quantizer = 63;
  cfg.g_threads = options->threads;
  if (aom_codec_enc_init(&encoder, iface, &cfg, depth > 8 ? AOM_CODEC_USE_HIGHBITDEPTH : 0) !=
      AOM_CODEC_OK) {
    snprintf(message, message_size, "%s", aom_codec_error(&encoder));
    return 0;
  }
  aom_codec_control(&encoder, AOME_SET_CQ_LEVEL, options->quantizer);
  if (options->quantizer == 0) aom_codec_control(&encoder, AV1E_SET_LOSSLESS, 1);
  aom_codec_control(&encoder, AOME_SET_CPUUSED, options->speed);
  aom_codec_control(&encoder, AV1E_SET_ROW_MT, 1);
  aom_codec_control(&encoder, AV1E_SET_COLOR_RANGE, AOM_CR_FULL_RANGE);
  if (!alpha) {
    aom_codec_control(&encoder, AV1E_SET_COLOR_PRIMARIES, options->color_primaries);
    aom_codec_control(&encoder, AV1E_SET_TRANSFER_CHARACTERISTICS,
                      options->transfer_characteristics);
    aom_codec_control(&encoder, AV1E_SET_MATRIX_COEFFICIENTS, options->matrix_coefficients);
  }
  // libavif skips applying the last loop filters to the encoder's own copy of a still frame
  // (AV1E_SET_SKIP_POSTPROC_FILTERING): the bitstream is the same either way, and slimdeer scores
  // that copy, so it applies them
  if (options->tune >= 0) aom_codec_control(&encoder, AOME_SET_TUNING, options->tune);
  if (options->screen) aom_codec_control(&encoder, AV1E_SET_TUNE_CONTENT, AOM_CONTENT_SCREEN);
  if (options->tile_rows_log2)
    aom_codec_control(&encoder, AV1E_SET_TILE_ROWS, options->tile_rows_log2);
  if (options->tile_cols_log2)
    aom_codec_control(&encoder, AV1E_SET_TILE_COLUMNS, options->tile_cols_log2);

  // 8-bit samples go to libaom as bytes, as libavif gives them (its high bit depth path is another
  // encoder)
  uint8_t* bytes = NULL;
  aom_image_t image;
  memset(&image, 0, sizeof(image));
  image.fmt = alpha ? AOM_IMG_FMT_I420 : AOM_IMG_FMT_I444;
  if (depth > 8) image.fmt |= AOM_IMG_FMT_HIGHBITDEPTH;
  image.bit_depth = depth > 8 ? 16 : 8;
  image.w = image.d_w = width;
  image.h = image.d_h = height;
  image.bps = (alpha ? 12 : 24) * (depth > 8 ? 2 : 1);
  // libaom has no 4:0:0 format: an alpha plane is 4:2:0 marked monochrome, its chroma never read
  image.x_chroma_shift = image.y_chroma_shift = alpha;
  image.monochrome = alpha;
  image.range = AOM_CR_FULL_RANGE;
  if (!alpha) {
    image.cp = options->color_primaries;
    image.tc = options->transfer_characteristics;
    image.mc = options->matrix_coefficients;
  }
  const size_t plane = (size_t)width * height;
  if (depth == 8) {
    bytes = malloc(plane * count);
    if (!bytes) {
      aom_codec_destroy(&encoder);
      snprintf(message, message_size, "out of memory");
      return 0;
    }
  }
  for (int p = 0; p < count; p++) {
    if (depth == 8) {
      for (size_t i = 0; i < plane; i++) bytes[p * plane + i] = (uint8_t)planes[p][i];
      image.planes[p] = bytes + p * plane;
      image.stride[p] = width;
    } else {
      image.planes[p] = (unsigned char*)planes[p];
      image.stride[p] = width * 2;
    }
  }

  const char* error = NULL;
  size_t capacity = 0;
  // the frame, then the flush (a NULL image) that ends the stream
  for (int pass = 0; pass < 2 && !error; pass++) {
    if (aom_codec_encode(&encoder, pass ? NULL : &image, 0, 1, 0) != AOM_CODEC_OK) {
      error = aom_codec_error_detail(&encoder);
      if (!error) error = aom_codec_error(&encoder);
      break;
    }
    aom_codec_iter_t iter = NULL;
    const aom_codec_cx_pkt_t* packet;
    while ((packet = aom_codec_get_cx_data(&encoder, &iter)) != NULL) {
      if (packet->kind == AOM_CODEC_CX_FRAME_PKT &&
          !append(out, out_size, &capacity, packet->data.frame.buf, packet->data.frame.sz))
        error = "out of memory";
    }
  }
  if (!error) {
    aom_image_t frame;
    if (aom_codec_control(&encoder, AV1_GET_NEW_FRAME_IMAGE, &frame) != AOM_CODEC_OK ||
        (int)frame.d_w != width || (int)frame.d_h != height) {
      error = "no decoded frame";
    } else {
      const int wide = (frame.fmt & AOM_IMG_FMT_HIGHBITDEPTH) != 0;
      for (int p = 0; p < count; p++) {
        uint16_t* to = decoded + p * plane;
        for (int y = 0; y < height; y++) {
          const unsigned char* row = frame.planes[p] + (size_t)y * frame.stride[p];
          for (int x = 0; x < width; x++)
            to[(size_t)y * width + x] = wide ? ((const uint16_t*)row)[x] : row[x];
        }
      }
    }
  }
  // the message may be the encoder's own, gone once it is destroyed
  if (error) snprintf(message, message_size, "%s", error);
  aom_codec_destroy(&encoder);
  free(bytes);
  if (error) {
    free(*out);
    *out = NULL;
    *out_size = 0;
  }
  return !error;
}

void slimdeer_aom_free(void* pointer) { free(pointer); }
