#!/bin/bash
# profile.sh INPUTS WASM OUTDIR: Node CPU profiles of the 10-bit encodes simd.json and files.json come from (hot.py):
# a photo at speeds 4, 6 and 8, a bigger photo, two icons, a screenshot with and without screen tuning, and 4:2:0.
# WASM needs its function names (simd-wasm.py's aomenc.wasm, not aomenc.opt.wasm). INPUTS is as hashes.sh's.
set -e
wasi="$(cd "$(dirname "$0")" && pwd)/wasi.mjs"
wasm="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
mkdir -p "$3"; out="$(cd "$3" && pwd)"
cd "$1"
run() { node --cpu-prof --cpu-prof-dir="$out" --cpu-prof-interval 250 "$wasi" "$wasm" --allintra --limit=1 --end-usage=q --tune=iq \
  --threads=1 --bit-depth=10 --input-bit-depth=10 --ivf -q -o profile.ivf "$@" 2>/dev/null; }
for s in 4 6 8; do run --cpu-used=$s --cq-level=30 in/web-10-etang-d-alate-04.y4m; done
run --cpu-used=6 --cq-level=30 in/web-02-belfry-in-millau-3.y4m
run --cpu-used=4 --cq-level=20 in/icons-openmoji-1f308.y4m
run --cpu-used=4 --cq-level=20 in/icons-twemoji-1f600.y4m
run --cpu-used=6 --cq-level=30 in/ui-hackernews-1x.y4m
run --cpu-used=6 --cq-level=30 --tune-content=screen in/ui-hackernews-1x.y4m
run --cpu-used=6 --cq-level=30 in420/web-10-etang-d-alate-04.y4m
rm -f profile.ivf
