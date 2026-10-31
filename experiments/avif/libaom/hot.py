"""hot.py BUILD OUTDIR PROFILE...: from profile.sh's profiles of BUILD (a simd-wasm.py --all-simd --all-o3 build), writes

  OUTDIR/simd.json   each NEON rtcd function the encodes ran, with its largest share of an encode's time (a sample
                     counts for the closest rtcd function on its stack, so a function's helpers count for it)
  OUTDIR/files.json  each source file, with its largest share of an encode's time (samples by the function they're in)
"""

import collections
import json
import pathlib
import re
import subprocess
import sys

build, outdir, profiles = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3:]
rtcd = set()
for h in ["aom_dsp_rtcd.h", "av1_rtcd.h"]:
    rtcd |= {m.group(2) for m in re.finditer(r"^#define (\w+) (\w+)$", (build / "config" / h).read_text(), re.M)}
owner = {}
nm = subprocess.run(["/opt/homebrew/opt/llvm/bin/llvm-nm", "--defined-only", "-A", build / "libaom.a"], capture_output=True, text=True).stdout
for m in re.finditer(r"^.*libaom\.a:(\S+)\.o: +\S* *\w (\S+)$", nm, re.M):
    owner.setdefault(m.group(2), m.group(1))
simd, files = collections.defaultdict(float), collections.defaultdict(float)
for path in profiles:
    p = json.load(open(path))
    nodes = {n["id"]: n for n in p["nodes"]}
    parent = {c: n["id"] for n in p["nodes"] for c in n.get("children", [])}
    by_fn, by_file, total = collections.Counter(), collections.Counter(), 0
    for sample, dt in zip(p["samples"], p["timeDeltas"]):
        total += dt
        by_file[owner.get(nodes[sample]["callFrame"]["functionName"])] += dt
        n = sample
        while n is not None and nodes[n]["callFrame"]["functionName"] not in rtcd:
            n = parent.get(n)
        if n is not None:
            by_fn[nodes[n]["callFrame"]["functionName"]] += dt
    for k, v in by_fn.items():
        simd[k] = max(simd[k], v / total)
    for k, v in by_file.items():
        if k:
            files[k] = max(files[k], v / total)
for name, d in [("simd.json", {k: v for k, v in simd.items() if k.endswith("_neon")}), ("files.json", files)]:
    json.dump({k: round(v, 5) for k, v in sorted(d.items(), key=lambda kv: -kv[1])}, open(outdir / name, "w"), indent=0)
    print(outdir / name, len(d))
