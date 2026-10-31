#!/bin/sh
# In Docker (Debian bookworm amd64, GCC 12): the patched libaom's aomenc for x86-64 with its SIMD and runtime CPU
# detection, static (for qemu-avx2.sh), then hashes.sh at each SIMD level up to SSE4.2 (AOM_SIMD_CAPS_MASK; Docker's
# Rosetta has no AVX):
#   docker run --rm --platform linux/amd64 -v PATCHED_LIBAOM:/aom:ro -v INPUTS:/eq -v experiments/avif/libaom:/lab:ro \
#     -v OUT:/out debian:bookworm /lab/linux-simd.sh
set -e
apt-get update -qq >/dev/null && apt-get install -y -qq cmake g++ make perl python3 nasm >/dev/null 2>&1
mkdir -p /tmp/b && cd /tmp/b
cmake /aom -DCMAKE_BUILD_TYPE=Release -DENABLE_DOCS=0 -DENABLE_TESTS=0 -DENABLE_TESTDATA=0 -DENABLE_EXAMPLES=0 -DCONFIG_AV1_DECODER=0 \
  -DCONFIG_MULTITHREAD=0 -DCONFIG_WEBM_IO=0 -DCONFIG_EXCLUDE_SIMD_MISMATCH=1 \
  -DCMAKE_C_FLAGS="-ffp-contract=off" -DCMAKE_CXX_FLAGS="-ffp-contract=off" -DCMAKE_EXE_LINKER_FLAGS="-static" >/dev/null
make -j10 aomenc >/dev/null 2>&1
cp aomenc /out/aomenc-x86
echo "built with $(gcc --version | head -1)" >&2
for mask in 0x07 0x3f 0x13f; do  # SSE2, up to SSE4.1, up to SSE4.2
  AOM_SIMD_CAPS_MASK=$mask /lab/hashes.sh /eq /tmp/b/aomenc > /out/x86-$mask.txt
done
