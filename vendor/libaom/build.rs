//! Builds the vendored libaom's AV1 encoder to write the same bytes on every target: no threads, no
//! fused multiply-add, and the inexact math functions and qsort routed to one implementation in Rust
//! (vendor/clib: pngdeer_clib.h, included ahead of every file). 64-bit ARM gets libaom's NEON code
//! (arm64/config, as libaom's CMake generates it for AOM_TARGET_CPU=arm64 with
//! CONFIG_EXCLUDE_SIMD_MISMATCH and pngdeer.patch, which leave out the SIMD whose results differ from
//! the C's); every other target plain C (generic/config, for AOM_TARGET_CPU=generic). The command
//! line and the browser build then write the same bytes.

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
    let arm = env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "aarch64";
    let config = root.join(if arm { "arm64" } else { "generic" });
    let clib = PathBuf::from(env::var("DEP_PNGDEER_CLIB_INCLUDE").unwrap());
    let mut build = cc::Build::new();
    build
        .include(&aom)
        .include(&config)
        .define("NDEBUG", None)
        // pngdeer.patch: leave the functions only inter frames use out of the encoder's tables
        .define("PNGDEER_STILL", "1")
        // libaom at -O0 (a debug build's) is tens of times slower; the bytes are the same at any level
        .opt_level(3)
        // clang fuses a * b + c where the target has FMA (arm64), and a fused result differs
        .flag("-ffp-contract=off")
        .flag("-include")
        .flag(clib.join("pngdeer_clib.h").to_str().unwrap())
        .warnings(false);
    // every C file here is one libaom's CMake compiles into its library (with the configuration
    // above), and none is included by another
    let mut found = (Vec::new(), Vec::new());
    sources(&aom, &mut found);
    let (mut files, mut arm_files) = found;
    files.sort();
    arm_files.sort();
    if arm {
        // ARMv8's CRC32 instructions (libaom's hash for screen content), which CMake builds with
        // -march=armv8-a+crc: a library of its own, for that one flag
        let (crc, neon): (Vec<PathBuf>, Vec<PathBuf>) = arm_files.into_iter().partition(|f| f.ends_with("hash_arm_crc32.c"));
        build.clone().files(crc).flag("-march=armv8-a+crc").compile("aom_crc32");
        files.extend(neon);
    }
    build.files(files).file(root.join("glue/aom_config.c")).file(root.join("glue/pngdeer_aom.c"));
    build.compile("aom");
    println!("cargo:rerun-if-changed=libaom");
    println!("cargo:rerun-if-changed=generic");
    println!("cargo:rerun-if-changed=arm64");
    println!("cargo:rerun-if-changed=glue");
}
