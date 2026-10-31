#!/bin/sh
# In Docker (Debian bookworm, GCC 12): builds libaom's aomenc as plain C, then runs hashes.sh on /eq.
#   docker run --rm [--platform linux/amd64] -v LIBAOM:/aom:ro -v INPUTS:/eq -v experiments/avif/libaom:/lab:ro \
#     debian:bookworm /lab/linux.sh
set -e
apt-get update -qq >/dev/null && apt-get install -y -qq cmake g++ make perl python3 >/dev/null 2>&1
mkdir -p /tmp/b && cd /tmp/b
cmake /aom -DAOM_TARGET_CPU=generic -DCMAKE_BUILD_TYPE=Release -DENABLE_DOCS=0 -DENABLE_TESTS=0 -DENABLE_TESTDATA=0 \
  -DCONFIG_MULTITHREAD=0 -DCONFIG_RUNTIME_CPU_DETECT=0 -DCONFIG_WEBM_IO=0 \
  -DCMAKE_C_FLAGS="-ffp-contract=off" -DCMAKE_CXX_FLAGS="-ffp-contract=off" >/dev/null
make -j8 aomenc >/dev/null 2>&1
echo "built with $(gcc --version | head -1) on $(uname -m)" >&2
/lab/hashes.sh /eq /tmp/b/aomenc
