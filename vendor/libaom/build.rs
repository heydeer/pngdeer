//! Builds the vendored libaom's AV1 encoder to write the same bytes on every target: no fused
//! multiply-add, the inexact math functions and qsort routed to one implementation in Rust
//! (vendor/clib: slimdeer_clib.h, included ahead of every file), and its multithreaded encoder, whose
//! workers src/workers.rs runs (on wasm32, vendor/clib's pthread.h locks for them). 64-bit ARM gets
//! libaom's NEON code (arm64/config, as libaom's CMake generates it for AOM_TARGET_CPU=arm64 with
//! CONFIG_EXCLUDE_SIMD_MISMATCH and slimdeer.patch, which leave out the SIMD whose results differ from
//! the C's); wasm32 the NEON functions that matter, on its SIMD128 through SIMDe (wasm/config, simde/);
//! every other target plain C (generic/config, for AOM_TARGET_CPU=generic). The command line and the
//! browser build then write the same bytes.

use std::{env, fs, path::PathBuf};

/// The C files under `dir`: (plain C, 64-bit ARM's).
fn sources(dir: &PathBuf, files: &mut (Vec<PathBuf>, Vec<PathBuf>)) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, files);
        } else if path.extension().is_some_and(|e| e == "c") {
            let arm = path.parent().unwrap().ends_with("arm") || path.ends_with("aom_ports/aarch64_cpudetect.c");
            if arm { &mut files.1 } else { &mut files.0 }.push(path);
        }
    }
}

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let aom = root.join("libaom");
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let (arm, wasm) = (arch == "aarch64", arch == "wasm32");
    let config = root.join(if arm {
        "arm64"
    } else if wasm {
        "wasm"
    } else {
        "generic"
    });
    let clib = PathBuf::from(env::var("DEP_SLIMDEER_CLIB_INCLUDE").unwrap());
    let mut build = cc::Build::new();
    build
        .include(&aom)
        .include(&config)
        .define("NDEBUG", None)
        // slimdeer.patch: leave the functions only inter frames use out of the encoder's tables
        .define("SLIMDEER_STILL", "1")
        // libaom at -O0 (a debug build's) is tens of times slower; the bytes are the same at any level
        .opt_level(3)
        // clang fuses a * b + c where the target has FMA (arm64), and a fused result differs
        .flag("-ffp-contract=off")
        .flag("-include")
        .flag(clib.join("slimdeer_clib.h").to_str().unwrap())
        .warnings(false);
    if wasm {
        // no C library: vendor/clib's headers and functions stand in for one; and SIMDe's arm_neon.h
        // for the compiler's, on SIMD128
        build.include(env::var("DEP_SLIMDEER_CLIB_LIBC").unwrap()).include(root.join("simde")).flag("-ffreestanding").flag("-msimd128");
    }
    // every C file here is one libaom's CMake compiles into its library (with the configuration
    // above), and none is included by another
    let mut found = (Vec::new(), Vec::new());
    sources(&aom, &mut found);
    let (mut files, arm_files) = found;
    // ARMv8's CRC32 instructions (libaom's hash for screen content), which CMake builds with
    // -march=armv8-a+crc and SIMDe doesn't have
    let (crc, neon): (Vec<PathBuf>, Vec<PathBuf>) = arm_files.into_iter().partition(|f| f.ends_with("hash_arm_crc32.c"));
    if arm {
        build.clone().files(crc).flag("-march=armv8-a+crc").compile("aom_crc32");
    }
    if arm || wasm {
        files.extend(neon);
    }
    files.push(root.join("glue/aom_config.c"));
    files.push(root.join("glue/slimdeer_aom.c"));
    files.sort();
    if wasm {
        // to download: the files that took at least 0.1% of an encode (wasm/hot.txt, from round 7's
        // profiles) at -O2, as fast as -O3 there, and the rest at -Os
        let hot: Vec<PathBuf> = fs::read_to_string(config.join("hot.txt")).unwrap().lines().map(|f| aom.join(f)).collect();
        let (hot, cold): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|f| hot.contains(f));
        build.clone().opt_level(2).files(hot).compile("aom_hot");
        build.opt_level_str("s").files(cold).compile("aom");
    } else {
        build.files(files).compile("aom");
    }
    println!("cargo:rerun-if-changed=libaom");
    println!("cargo:rerun-if-changed=generic");
    println!("cargo:rerun-if-changed=arm64");
    println!("cargo:rerun-if-changed=wasm");
    println!("cargo:rerun-if-changed=simde");
    println!("cargo:rerun-if-changed=glue");
}
