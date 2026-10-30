// pngdeer: one entry point into mozjpeg with plain arguments, so the Rust side (src/lib.rs) declares
// none of libjpeg's structs.
#include <stddef.h>
#include <stdio.h>  // jpeglib.h needs FILE and size_t declared first
#include <stdlib.h>

#include "jpeglib.h"

typedef struct {
  int quality;      // 1-100, scaling the base table as libjpeg's quality does
  int quant_table;  // mozjpeg's base quantization table (0-8), or -1 for its default
  int subsampling;  // chroma: 0 for 4:2:0 (libjpeg's default for color), 1 for 4:4:4
  int progressive;  // mozjpeg's progressive scans, chosen per image; 0 for one baseline scan
} PngdeerJpegOptions;

// libjpeg ends on any error through error_exit, which must not return. Encoding fails only on
// arguments src/lib.rs has already checked, or out of memory, where pngdeer aborts anyway.
static void error_exit(j_common_ptr cinfo) {
  (void)cinfo;
  abort();
}

static void output_message(j_common_ptr cinfo) { (void)cinfo; }

// `pixels`: `height` rows of `width` × `components` bytes (1: gray, 3: RGB). The file's bytes are
// in *out (free them with pngdeer_jpeg_free); returns their count.
size_t pngdeer_jpeg_encode(const unsigned char* pixels, int width, int height, int components,
                           const PngdeerJpegOptions* options, const unsigned char* icc,
                           size_t icc_size, const unsigned char* exif, size_t exif_size,
                           unsigned char** out) {
  struct jpeg_compress_struct cinfo;
  struct jpeg_error_mgr err;
  unsigned char* buffer = NULL;
  unsigned long size = 0;
  cinfo.err = jpeg_std_error(&err);
  err.error_exit = error_exit;
  err.output_message = output_message;
  jpeg_create_compress(&cinfo);
  jpeg_mem_dest(&cinfo, &buffer, &size);
  cinfo.image_width = (JDIMENSION)width;
  cinfo.image_height = (JDIMENSION)height;
  cinfo.input_components = components;
  cinfo.in_color_space = components == 1 ? JCS_GRAYSCALE : JCS_RGB;
  jpeg_set_defaults(&cinfo);
  // the base table first: jpeg_set_quality scales it
  if (options->quant_table >= 0)
    jpeg_c_set_int_param(&cinfo, JINT_BASE_QUANT_TBL_IDX, options->quant_table);
  jpeg_set_quality(&cinfo, options->quality, TRUE);
  if (components == 3 && options->subsampling == 1) {
    for (int c = 0; c < 3; c++) {
      cinfo.comp_info[c].h_samp_factor = 1;
      cinfo.comp_info[c].v_samp_factor = 1;
    }
  }
  if (!options->progressive) {
    jpeg_c_set_bool_param(&cinfo, JBOOLEAN_OPTIMIZE_SCANS, FALSE);
    cinfo.num_scans = 0;
    cinfo.scan_info = NULL;
  }
  jpeg_start_compress(&cinfo, TRUE);
  if (exif_size) jpeg_write_marker(&cinfo, JPEG_APP0 + 1, exif, (unsigned int)exif_size);
  if (icc_size) jpeg_write_icc_profile(&cinfo, icc, (unsigned int)icc_size);
  size_t stride = (size_t)width * (size_t)components;
  while (cinfo.next_scanline < cinfo.image_height) {
    JSAMPROW row = (JSAMPROW)(pixels + (size_t)cinfo.next_scanline * stride);
    jpeg_write_scanlines(&cinfo, &row, 1);
  }
  jpeg_finish_compress(&cinfo);
  jpeg_destroy_compress(&cinfo);
  *out = buffer;
  return size;
}

void pngdeer_jpeg_free(void* pointer) { free(pointer); }
