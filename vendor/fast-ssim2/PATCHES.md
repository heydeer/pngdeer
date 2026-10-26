# fast-ssim2 0.9.0, patched for pngdeer's WebAssembly build

Vendored from crates.io (BSD-2-Clause, https://github.com/imazen/fast-ssim2); every pngdeer build uses
it (`[patch.crates-io]` in the root `Cargo.toml`). Examples, tests, benches and dev-dependencies
are left out. Three changes, all in the scalar kernels that the wasm build runs; scores are unchanged
(checked against the native build to 4 decimals on 5 MP screenshots):

1. `pipeline/mod.rs` adds a `Fma` trait, and `gauss.rs`, `xyb.rs`, `maps.rs`, `simd.rs` call `.fma(`
   where they called `.mul_add(`. wasm32 has no scalar fused multiply-add, so `mul_add` compiled to a
   call into compiler-builtins' bit-exact software `fmaf`. On wasm32 the trait computes in f64 instead
   (an f32 product is exact there; the result differs from fma only on rare double-rounding ties);
   elsewhere it is `mul_add`.
2. `gauss.rs` `fast_gaussian_vertical_1d` walks columns in blocks of 64, row by row, instead of one
   whole column at a time. Same arithmetic per column, far fewer cache misses.
3. `pipeline/mod.rs` `planes_to_positive_xyb` reuses the previous pixel's XYB when the linear RGB
   is bit-identical, skipping three cube roots per pixel in flat areas (most of a screenshot, and
   every run in a palette image).
