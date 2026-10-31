"""bisect.py AOMENC CONFIG IMAGE WANT: which rtcd functions of AOMENC (built with forcec.py; CONFIG is its config/ dir)
must be C for IMAGE's 10-bit speed-8 encode at cq-level 20 to have MD5 WANT (its first 12 hex digits, as hashes.sh
prints them for the plain C build). Found by bisecting FORCE_C, one function at a time."""

import hashlib
import os
import pathlib
import re
import subprocess
import sys

enc, cfg, image, want = sys.argv[1], pathlib.Path(sys.argv[2]), sys.argv[3], sys.argv[4]
names = sorted({n for h in ["aom_dsp_rtcd.h", "av1_rtcd.h"] for n in re.findall(r"^RTCD_EXTERN \w[\w\s\*]*?\(\*(\w+)\)", (cfg / h).read_text(), re.M)})


def ok(force: list[str]) -> bool:
    env = dict(os.environ, FORCE_C="," + ",".join(force) + ",")
    subprocess.run([enc, "--allintra", "--limit=1", "--cpu-used=8", "--end-usage=q", "--cq-level=20", "--tune=iq", "--threads=1",
                    "--bit-depth=10", "--input-bit-depth=10", "--ivf", "-q", "-o", "/tmp/bisect.ivf", image], env=env, check=True, capture_output=True)
    return hashlib.md5(open("/tmp/bisect.ivf", "rb").read()).hexdigest()[:12] == want


assert ok(names), "differs even with every function in C"
culprits, rest = [], names
while not ok(culprits):
    lo, hi = 0, len(rest)  # the shortest prefix of rest that, with culprits, matches ends in a culprit
    while hi - lo > 1:
        mid = (lo + hi) // 2
        lo, hi = (lo, mid) if ok(culprits + rest[:mid]) else (mid, hi)
    culprits.append(rest[hi - 1])
    rest = rest[: hi - 1]
    print("culprit:", culprits[-1], flush=True)
print("culprits:", culprits)
