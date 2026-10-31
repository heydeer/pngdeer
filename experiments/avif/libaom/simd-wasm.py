"""simd-wasm.py NATIVE OUT [--all-simd] [--all-o3]: aomenc for wasm32-wasip1 with wasm SIMD, from libaom's NEON code.

libaom has no wasm SIMD of its own, but its NEON functions write the same bytes as its C (pngdeer.patch drops the two
that don't), and SIMDe implements NEON's intrinsics on wasm's SIMD128. So this takes NATIVE, an arm64 build tree of the
patched libaom configured with -DCMAKE_EXPORT_COMPILE_COMMANDS=ON (see FINDINGS.md), recompiles each of its files for
wasm with simde/arm_neon.h in place of the real one, and links OUT/aomenc.wasm (with function names, for
profile.sh) and OUT/aomenc.opt.wasm (through wasm-opt, to ship and time), run with wasi.mjs.

To keep the module no bigger than the plain C one, only the NEON functions simd.json lists are used (the ones 10-bit
encodes spent time in, as profile.sh and hot.py measured; the rest go back to C, which writes the same bytes), files
that took under 0.1% of the time in files.json are built at -Os and the others at -O2, and every file is built with
-DPNGDEER_STILL=1, which leaves the encoder's inter-frame-only functions out of its tables (pngdeer.patch). --all-simd
and --all-o3 turn off the first two.
"""

import concurrent.futures
import json
import pathlib
import re
import shlex
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
LLVM = "/opt/homebrew/opt/llvm/bin"
SYSROOT = "/opt/homebrew/opt/wasi-libc/share/wasi-sysroot"
RUNTIMES = "/opt/homebrew/opt/wasi-runtimes/share"
SIMDE = "/opt/homebrew/opt/simde/include"


def config(native: pathlib.Path, out: pathlib.Path, simd: set[str] | None) -> None:
    """native's generated headers, for wasm: no OS support or ARM CRC32 (SIMDe has none), and only the NEON in simd."""
    (out / "config").mkdir(parents=True, exist_ok=True)
    for h in (native / "config").glob("*.h"):
        shutil.copy(h, out / "config" / h.name)
    path = out / "config" / "aom_config.h"
    text = path.read_text()
    for flag in ["CONFIG_OS_SUPPORT", "HAVE_ARM_CRC32", "HAVE_PTHREAD_SETNAME_NP"]:
        text = re.sub(rf"^#define {flag} 1$", f"#define {flag} 0", text, flags=re.M)
    path.write_text(text)
    for name in ["aom_dsp_rtcd.h", "av1_rtcd.h"]:
        path = out / "config" / name

        def pick(m: re.Match) -> str:
            fn, impl = m.groups()
            c = impl.endswith("_arm_crc32") or (simd is not None and impl.endswith("_neon") and impl not in simd)
            return f"#define {fn} {fn}_c" if c else m.group(0)

        path.write_text(re.sub(r"^#define (\w+) (\w+)$", pick, path.read_text(), flags=re.M))


def main() -> None:
    native, out = pathlib.Path(sys.argv[1]).resolve(), pathlib.Path(sys.argv[2]).resolve()
    all_simd, all_o3 = "--all-simd" in sys.argv, "--all-o3" in sys.argv
    config(native, out, None if all_simd else set(json.load(open(HERE / "simd.json"))))
    hot_files = json.load(open(HERE / "files.json"))
    source = pathlib.Path(re.search(r"^CMAKE_HOME_DIRECTORY:INTERNAL=(.*)$", (native / "CMakeCache.txt").read_text(), re.M).group(1))
    common = ["--target=wasm32-wasip1", f"--sysroot={SYSROOT}", f"-resource-dir={RUNTIMES}/wasi-runtimes", "-mllvm", "-wasm-enable-sjlj",
              "-msimd128", "-ffp-contract=off", "-D_GNU_SOURCE", "-DPNGDEER_STILL=1", f"-I{HERE / 'simde'}", f"-I{out}", f"-I{SIMDE}"]
    cxx = ["-fno-exceptions", "-isystem", f"{RUNTIMES}/wasi-sysroot/include/wasm32-wasip1/c++/v1", "-isystem", f"{RUNTIMES}/wasi-sysroot/include/c++/v1"]
    jobs = []
    for entry in json.load(open(native / "compile_commands.json")):
        file = pathlib.Path(entry["file"])
        if file.name == "hash_arm_crc32.c" or file.match("apps/aomdec.c"):
            continue
        args, flags, skip = shlex.split(entry["command"]), [], False
        for a in args[1:]:
            if skip:
                skip = False
            elif a in ("-arch", "-isysroot", "-o"):
                skip = True
            elif a != "-c" and a != entry["file"] and not a.startswith(("-march", "-mcpu", "-mtune", "-mmacosx-version-min")):
                flags.append(a)
        if not all_o3:
            key = str(file.relative_to(source) if file.is_relative_to(source) else file.name).replace("/", "_")
            flags = [("-O2" if hot_files.get(key, 0) > 0.001 else "-Os") if a == "-O3" else a for a in flags]
        compiler = f"{LLVM}/clang++" if args[0].endswith("++") else f"{LLVM}/clang"
        obj = out / "obj" / (str(file.relative_to(source) if file.is_relative_to(source) else file.relative_to(native)).replace("/", "_") + ".o")
        jobs.append((file, [compiler, *common, *(cxx if compiler.endswith("++") else []), *flags, "-c", str(file), "-o", str(obj)]))
    (out / "obj").mkdir(exist_ok=True)

    def compile(job):
        file, command = job
        r = subprocess.run(command, cwd=native, capture_output=True, text=True)
        return file, r.returncode, r.stderr

    with concurrent.futures.ThreadPoolExecutor() as pool:
        failed = [(f, err) for f, rc, err in pool.map(compile, jobs) if rc]
    for f, err in failed:
        print(f"{f}:\n{err[-2000:]}", file=sys.stderr)
    if failed:
        sys.exit(1)
    objs = sorted({str(out / "obj" / (str(f.relative_to(source) if f.is_relative_to(source) else f.relative_to(native)).replace("/", "_") + ".o")) for f, _ in jobs})
    # aomenc's own files on the command line; libaom's in an archive, so the linker takes only what is used (as libaom.a)
    app = [o for o in objs if re.search(r"/obj/(apps|common)_", o)]
    lib = [o for o in objs if o not in app and "_gen_src_" not in o and "/obj/gen_src_" not in o]
    (out / "libaom.a").unlink(missing_ok=True)
    subprocess.run([f"{LLVM}/llvm-ar", "rcs", str(out / "libaom.a"), *lib], check=True)
    # No -O at link: clang would run wasm-opt, which takes the function names profile.sh needs and saves no size
    subprocess.run([f"{LLVM}/clang++", "--target=wasm32-wasip1", f"--sysroot={SYSROOT}", f"-resource-dir={RUNTIMES}/wasi-runtimes",
                    "-fuse-ld=/opt/homebrew/opt/lld/bin/wasm-ld", f"-L{RUNTIMES}/wasi-sysroot/lib/wasm32-wasip1", "-lsetjmp",
                    "-Wl,-z,stack-size=16777216", "-Wl,--max-memory=4294967296", *app, str(out / "libaom.a"), "-o", str(out / "aomenc.wasm")], check=True)
    # What clang runs at -O3 when wasm-opt is installed (about 3% smaller gzipped); names stay in aomenc.wasm
    subprocess.run(["wasm-opt", "-O3", str(out / "aomenc.wasm"), "-o", str(out / "aomenc.opt.wasm")], check=True)
    print(out / "aomenc.opt.wasm")


if __name__ == "__main__":
    main()
