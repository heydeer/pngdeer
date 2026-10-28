//! Builds the vendored libwebp (its decoder, encoder and sharpyuv) the same way on every target:
//! plain C, no threads, no fused multiply-add, and the inexact math functions and qsort routed to
//! one implementation in Rust (glue/pngdeer.h, src/lib.rs). The command line and the browser build
//! then write the same bytes. For wasm32 a few headers stand in for a C library (glue/libc), whose
//! functions src/lib.rs and glue/libc.c provide.

use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let webp = root.join("libwebp");
    let wasm = env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "wasm32";
    let mut build = cc::Build::new();
    build
        .include(&webp)
        // libwebp/src/webp/config.h: no WEBP_HAVE_SSE2/NEON/..., so no SIMD code (its results can
        // differ from the plain C's), and no WEBP_USE_THREAD
        .define("HAVE_CONFIG_H", None)
        .define("NDEBUG", None)
        // clang fuses a * b + c where the target has FMA (arm64), and a fused result differs
        .flag("-ffp-contract=off")
        .flag("-include")
        .flag(root.join("glue/pngdeer.h").to_str().unwrap())
        .warnings(false);
    if wasm {
        build.include(root.join("glue/libc")).flag("-ffreestanding").file(root.join("glue/libc.c"));
    }
    build.file(root.join("glue/pngdeer_webp.c"));
    for dir in ["src/dec", "src/dsp", "src/enc", "src/utils", "sharpyuv"] {
        let mut files: Vec<PathBuf> =
            fs::read_dir(webp.join(dir)).unwrap().map(|entry| entry.unwrap().path()).filter(|path| path.extension().is_some_and(|e| e == "c")).collect();
        files.sort();
        build.files(files);
    }
    build.compile("webp");
    println!("cargo:rerun-if-changed=libwebp");
    println!("cargo:rerun-if-changed=glue");
}
