#!/bin/bash
# hashes.sh INPUTS AOMENC...: one all-intra frame per setting, as libavif encodes stills (tune=iq,
# one thread), for every Y4M in INPUTS/in (10-bit 4:4:4), INPUTS/in8 (8-bit 4:4:4) and INPUTS/in420
# (10-bit 4:2:0), at speeds 4, 6 and 8 (12 MP and up: 6 and 8) and cq-levels 20 and 40; prints a
# line per file with its MD5, to diff between builds. AOMENC is a command, e.g. a native aomenc or
# `node experiments/avif/libaom/wasi.mjs aomenc.wasm` (it runs in INPUTS: paths are relative).
set -e
cd "$1"; shift
for y in in/*.y4m in8/*.y4m in420/*.y4m; do
  [ -f "$y" ] || continue
  case "$y" in in8/*) depth="--bit-depth=8";; *) depth="--bit-depth=10 --input-bit-depth=10";; esac
  big=$(( $(stat -c%s "$y" 2>/dev/null || stat -f%z "$y") > 60000000 ))
  for s in 4 6 8; do
    [ "$s" = 4 ] && [ "$big" = 1 ] && continue
    for cq in 20 40; do
      "$@" --allintra --limit=1 --cpu-used=$s --end-usage=q --cq-level=$cq --tune=iq --threads=1 $depth --ivf -q -o hashes.ivf "$y" >/dev/null 2>&1
      echo "$(dirname "$y")/$(basename "$y" .y4m | cut -c1-24) s$s cq$cq $( (md5sum 2>/dev/null || md5) < hashes.ivf | cut -c1-12)"
    done
  done
done
rm -f hashes.ivf
