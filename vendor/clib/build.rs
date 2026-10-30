//! For wasm32, which has no C library, builds libc.c (the functions there) and offers libc/ (headers
//! declaring what the vendored libraries call). Every target gets include/pngdeer_clib.h, which the
//! vendored libraries include ahead of every file. The crates that depend on this one find both
//! folders in DEP_PNGDEER_CLIB_INCLUDE and DEP_PNGDEER_CLIB_LIBC.

use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    if env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "wasm32" {
        cc::Build::new().include(root.join("libc")).flag("-ffreestanding").file(root.join("libc.c")).warnings(false).compile("pngdeer_clib");
    }
    println!("cargo:include={}", root.join("include").display());
    println!("cargo:libc={}", root.join("libc").display());
    println!("cargo:rerun-if-changed=libc");
    println!("cargo:rerun-if-changed=libc.c");
    println!("cargo:rerun-if-changed=include");
}
