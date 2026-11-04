// slimdeer: what libaom's CMake generates as aom_config.c, for the build in ../build.rs.
#include "aom/aom_codec.h"
static const char* const cfg =
    "slimdeer: -DAOM_TARGET_CPU=generic -DCONFIG_AV1_DECODER=0 -DCONFIG_MULTITHREAD=0 "
    "-DCONFIG_RUNTIME_CPU_DETECT=0 -DCONFIG_WEBM_IO=0";
const char* aom_codec_build_config(void) { return cfg; }
