#!/bin/sh
# In Docker (Debian bookworm arm64): linux-simd.sh's static aomenc under QEMU's "max" CPU, which has AVX2, through
# hashes.sh:
#   docker run --rm --platform linux/arm64 -v INPUTS:/eq -v experiments/avif/libaom:/lab:ro -v OUT:/out rust:1-bookworm \
#     /lab/qemu-avx2.sh
set -e
apt-get update -qq >/dev/null && apt-get install -y -qq qemu-user >/dev/null 2>&1
/lab/hashes.sh /eq qemu-x86_64 -cpu max /out/aomenc-x86 > /out/x86-avx2.txt
