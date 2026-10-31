# forcec.py BUILD: lets FORCE_C=",name,name," in the environment send those runtime-dispatched rtcd functions (a build
# with CONFIG_RUNTIME_CPU_DETECT) back to their C versions, for bisect.py; run after cmake, before make.
import re, sys, pathlib
for h in ["aom_dsp_rtcd.h", "av1_rtcd.h", "aom_scale_rtcd.h"]:
    p = pathlib.Path(sys.argv[1]) / "config" / h; t = p.read_text()
    names = sorted(set(re.findall(r"^RTCD_EXTERN \w[\w\s\*]*?\(\*(\w+)\)", t, re.M)))
    body = "".join(f'    if (strstr(f, ",{n},")) {n} = {n}_c;\n' for n in names)
    hook = "  {\n    const char *f = getenv(\"FORCE_C\");\n    if (f) {\n" + body + "    }\n  }\n"
    t, k = re.subn(r"(static void setup_rtcd_internal\(void\)\n\{\n(?:.*\n)*?)(\}\n)", lambda m: m.group(1) + hook + m.group(2), t, count=1)
    assert k == 1, h
    t = t.replace("#ifdef RTCD_C", "#include <stdlib.h>\n#include <string.h>\n#ifdef RTCD_C", 1)
    p.write_text(t); print(h, len(names))
