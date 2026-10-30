//! Builds the vendored mozjpeg's encoder the same way on every target: plain C (no SIMD: jsimd_none.c),
//! no fused multiply-add, no environment lookups (NO_GETENV, NO_PUTENV: JPEGMEM and the like can't
//! change a file), and the inexact math functions routed to one implementation in Rust (vendor/clib:
//! pngdeer_clib.h, included ahead of every file). The command line and the browser build then write
//! the same bytes. For wasm32 vendor/clib's headers stand in for a C library.

use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let wasm = env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "wasm32";
    let clib = PathBuf::from(env::var("DEP_PNGDEER_CLIB_INCLUDE").unwrap());
    let mut build = cc::Build::new();
    build
        .include(root.join("config"))
        .include(root.join("mozjpeg"))
        .define("NO_GETENV", None)
        .define("NO_PUTENV", None)
        .define("NDEBUG", None)
        // clang fuses a * b + c where the target has FMA (arm64), and a fused result differs
        .flag("-ffp-contract=off")
        .flag("-include")
        .flag(clib.join("pngdeer_clib.h").to_str().unwrap())
        .warnings(false);
    if wasm {
        build.include(env::var("DEP_PNGDEER_CLIB_LIBC").unwrap()).flag("-ffreestanding");
    }
    build.file(root.join("glue/pngdeer_jpeg.c"));
    // the encoder's files; jccolext.c and jstdhuff.c are included by others
    let mut files: Vec<PathBuf> = fs::read_dir(root.join("mozjpeg"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "c") && !["jccolext.c", "jstdhuff.c"].contains(&path.file_name().unwrap().to_str().unwrap()))
        .collect();
    files.sort();
    build.files(files);
    build.compile("mozjpeg");
    println!("cargo:rerun-if-changed=mozjpeg");
    println!("cargo:rerun-if-changed=config");
    println!("cargo:rerun-if-changed=glue");
}
