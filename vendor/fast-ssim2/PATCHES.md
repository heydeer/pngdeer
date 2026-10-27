# fast-ssim2 0.9.0, patched for pngdeer's WebAssembly build

Vendored from crates.io (BSD-2-Clause, https://github.com/imazen/fast-ssim2); every pngdeer build uses
it (`[patch.crates-io]` in the root `Cargo.toml`). Examples, tests, benches and dev-dependencies
are left out. The first three changes are in the scalar kernels that the wasm build runs; scores are
unchanged (checked against the native build to 4 decimals on 5 MP screenshots). The fourth cuts
memory and the fifth threads the scalar blur, both with bit-identical scores:

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
4. Memory, so that wasm32's 4 GB holds large images (`precompute.rs`, `pipeline/precompute.rs`,
   `pipeline/maps.rs`, `pipeline/simd.rs`):
   - `Ssimulacra2Reference::new_lean_with_config` keeps only the encoded source. Each comparison
     linearizes both images, walks their scales together and, per channel, runs the five blurs
     one after another with one scratch plane, then that channel's maps (`compare_lean`,
     `lean_scales`, `dist_scale_lean`). No reference pyramid is kept and at most five blurred
     planes are alive instead of fifteen; comparisons take longer.
   - The per-channel map code is split out of the fused functions (`maps_channel_stop`,
     `maps_channel_simd_stop`, `channel_maps`), and the whole-image comparison calls it too, so
     both paths run the same code.
   - The threaded blurs move their outputs out instead of cloning them (which held a second copy
     of every blurred plane), the non-threaded comparison no longer allocates three planes it
     never uses, encoded images of at least 8×8 are no longer copied for padding, and an opaque
     comparison frees its copy of the image once linearized.

   pngdeer's outputs are byte-identical with and without this change, on its 180 benchmark images
   at automatic and fixed quality, with the lean reference used for every image and for none.
5. `pipeline/precompute.rs` `scalar_blur`, `gauss.rs` `fast_gaussian_vertical_columns`: the scalar
   kernel's blur (the one the wasm builds run, lean comparisons included) multiplies two planes a
   row at a time into the output plane, which the vertical pass overwrites anyway, instead of into
   a plane of its own, and with rayon runs the horizontal pass a row per task and the vertical pass
   a 256-column range per task. Every row and column goes through the same arithmetic as before,
   so scores are bit-identical (whole-image and lean, with and without rayon, on 47 image pairs),
   and no plane is added: a threaded lean comparison is no longer on one thread.
