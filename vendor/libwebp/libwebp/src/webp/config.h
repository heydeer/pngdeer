// pngdeer: libwebp's build configuration, which its autotools build would generate. It defines
// nothing on purpose: with HAVE_CONFIG_H set and no WEBP_HAVE_SSE2/SSE41/AVX2/NEON, every target
// runs the same plain C code (see ../../../build.rs), so the command line and the browser build
// write the same bytes.
