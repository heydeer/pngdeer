#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]
//! Lane-wise SIMD versions of the match-official kernels.
//!
//! Every kernel computes *exactly the same scalar operation sequence per
//! lane* as the scalar reference port — the parallelism comes from
//! independent lanes (different pixels / rows / columns), so results are
//! bit-identical to [`super`]'s scalar code and hence to the official
//! binaries.
//!
//! Ordering-sensitive reductions (the f64 `sum0`/`sum1` accumulators in
//! the maps) are kept sequential: SIMD computes per-pixel terms into a
//! small buffer and the accumulation stays scalar-ordered.

#![allow(non_camel_case_types)]
#[allow(unused_imports)]
use super::Fma; // pngdeer patch
use archmage::incant;
use archmage::magetypes;
#[cfg(target_arch = "x86_64")]
use archmage::{SimdToken, X64V3Token};
use enough::Stop;

/// Rows between `stop` polls in the sequential horizontal-pass fallback.
#[cfg(not(feature = "rayon"))]
const STOP_ROW_STRIDE: usize = 1 << 10;
/// Column-block iterations between `stop` polls in the vertical pass
/// (each block processes 8 columns × full height).
#[cfg(not(feature = "rayon"))]
const STOP_COL_STRIDE: usize = 128;
/// Pixels between `stop` polls inside `maps_fused_simd_stop`.
const MAPS_SIMD_STOP_STRIDE: usize = 1 << 21;

/// 8-pixel edge-diff chunk in AVX2: the f64 division lane-widened via
/// `_mm256_cvtps_pd` instead of per-lane `as f64` extraction — the
/// magetypes widening path measured 70% more instructions than scalar
/// (iai: 49.3M vs 28.9M at 512²). Per-pixel results are bit-exact vs the
/// scalar port (identical f64 division, sequential accumulate order is
/// the caller's concern).
#[cfg(target_arch = "x86_64")]
#[archmage::arcane]
fn edge_chunk8_v3(
    _token: X64V3Token,
    i2: &[f32; 8],
    m2: &[f32; 8],
    i1: &[f32; 8],
    m1: &[f32; 8],
) -> [f64; 8] {
    use core::arch::x86_64::*;
    let abs_mask = _mm256_castsi256_ps(_mm256_set1_epi32(0x7fffffff_u32 as i32));
    let i2v = _mm256_setr_ps(i2[0], i2[1], i2[2], i2[3], i2[4], i2[5], i2[6], i2[7]);
    let m2v = _mm256_setr_ps(m2[0], m2[1], m2[2], m2[3], m2[4], m2[5], m2[6], m2[7]);
    let i1v = _mm256_setr_ps(i1[0], i1[1], i1[2], i1[3], i1[4], i1[5], i1[6], i1[7]);
    let m1v = _mm256_setr_ps(m1[0], m1[1], m1[2], m1[3], m1[4], m1[5], m1[6], m1[7]);
    let da = _mm256_and_ps(_mm256_sub_ps(i2v, m2v), abs_mask);
    let db = _mm256_and_ps(_mm256_sub_ps(i1v, m1v), abs_mask);
    let one = _mm256_set1_pd(1.0);
    let num_lo = _mm256_add_pd(_mm256_cvtps_pd(_mm256_extractf128_ps(da, 0)), one);
    let num_hi = _mm256_add_pd(_mm256_cvtps_pd(_mm256_extractf128_ps(da, 1)), one);
    let den_lo = _mm256_add_pd(_mm256_cvtps_pd(_mm256_extractf128_ps(db, 0)), one);
    let den_hi = _mm256_add_pd(_mm256_cvtps_pd(_mm256_extractf128_ps(db, 1)), one);
    // Sequential-order output: extract lanes via register ops so the
    // caller's f64 accumulation stays in strict pixel order.
    let lo = _mm256_sub_pd(_mm256_div_pd(num_lo, den_lo), one);
    let hi = _mm256_sub_pd(_mm256_div_pd(num_hi, den_hi), one);
    let lo01 = _mm256_castpd256_pd128(lo);
    let lo23 = _mm256_extractf128_pd(lo, 1);
    let hi01 = _mm256_castpd256_pd128(hi);
    let hi23 = _mm256_extractf128_pd(hi, 1);
    [
        _mm_cvtsd_f64(lo01),
        _mm_cvtsd_f64(_mm_unpackhi_pd(lo01, lo01)),
        _mm_cvtsd_f64(lo23),
        _mm_cvtsd_f64(_mm_unpackhi_pd(lo23, lo23)),
        _mm_cvtsd_f64(hi01),
        _mm_cvtsd_f64(_mm_unpackhi_pd(hi01, hi01)),
        _mm_cvtsd_f64(hi23),
        _mm_cvtsd_f64(_mm_unpackhi_pd(hi23, hi23)),
    ]
}

/// Scalar fallback for [`edge_chunk8_v3`].
fn edge_chunk8_scalar(i2: &[f32; 8], m2: &[f32; 8], i1: &[f32; 8], m1: &[f32; 8]) -> [f64; 8] {
    let mut out = [0f64; 8];
    for l in 0..8 {
        let num = 1.0 + (i2[l] - m2[l]).abs() as f64;
        let den = 1.0 + (i1[l] - m1[l]).abs() as f64;
        out[l] = num / den - 1.0;
    }
    out
}

/// Per-channel edge-diff sums over `ys..ye` rows, chunk-wide — V3 path
/// when available, scalar otherwise. f64 accumulation stays sequential
/// → bit-exact vs `edge_sums_ch` / `edge_diff_map`.
#[allow(clippy::too_many_arguments)]
pub fn edge_sums_fast(
    ys: usize,
    ye: usize,
    w: usize,
    img1: &[f32],
    mu1: &[f32],
    img2: &[f32],
    mu2: &[f32],
    sums: &mut [f64; 4],
) {
    #[cfg(target_arch = "x86_64")]
    let v3tok = X64V3Token::summon();
    for y in ys..ye {
        let row = y * w;
        let mut x = 0usize;
        while x + 8 <= w {
            let d: [f64; 8] = {
                let (i2, m2, i1, m1) = (
                    &img2[row + x..row + x + 8],
                    &mu2[row + x..row + x + 8],
                    &img1[row + x..row + x + 8],
                    &mu1[row + x..row + x + 8],
                );
                let (i2, m2, i1, m1) = (
                    i2.try_into().unwrap(),
                    m2.try_into().unwrap(),
                    i1.try_into().unwrap(),
                    m1.try_into().unwrap(),
                );
                #[cfg(target_arch = "x86_64")]
                match v3tok {
                    Some(tok) => edge_chunk8_v3(tok, i2, m2, i1, m1),
                    None => edge_chunk8_scalar(i2, m2, i1, m1),
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    edge_chunk8_scalar(i2, m2, i1, m1)
                }
            };
            for v in d {
                let artifact = v.max(0.0);
                sums[0] += artifact;
                sums[1] += super::maps::tothe4th(artifact);
                let detail_lost = (-v).max(0.0);
                sums[2] += detail_lost;
                sums[3] += super::maps::tothe4th(detail_lost);
            }
            x += 8;
        }
        while x < w {
            let num = 1.0 + (img2[row + x] - mu2[row + x]).abs() as f64;
            let den = 1.0 + (img1[row + x] - mu1[row + x]).abs() as f64;
            let d1 = num / den - 1.0;
            let artifact = d1.max(0.0);
            sums[0] += artifact;
            sums[1] += super::maps::tothe4th(artifact);
            let detail_lost = (-d1).max(0.0);
            sums[2] += detail_lost;
            sums[3] += super::maps::tothe4th(detail_lost);
            x += 1;
        }
    }
}

use magetypes::simd::generic::f32x8 as GenericF32x8;
use magetypes::simd::generic::i32x8 as GenericI32x8;

use super::gauss::RecursiveGaussian;
use super::xyb::{self, BIAS, M00, M01, M02, M10, M11, M12, M20, M21, M22, NEG_CBRT_BIAS};

// ===========================================================================
// CubeRootAndAdd — lane-wise port of the exact scalar sequence
// ===========================================================================

/// `CubeRootAndAdd` for 8 lanes — identical instruction sequence to the
/// scalar `xyb::cube_root_and_add`.
#[macro_export]
macro_rules! cube_root_and_add_x8 {
    ($token:expr, $x:expr, $add:expr) => {{
        const K_EXP_BIAS: i32 = 0x5480_0000;
        const K_EXP_MUL: i32 = 0x002a_aaaa;
        const K1_3: f32 = 1.0 / 3.0;
        const K4_3: f32 = 4.0 / 3.0;
        let token = $token;
        let xa = $x;
        let xa_3 = GenericF32x8::splat_t(token, K1_3) * xa;
        let k4_3 = GenericF32x8::splat_t(token, K4_3);
        let k1_3 = GenericF32x8::splat_t(token, K1_3);

        let m1 = xa.bitcast_to_i32();
        let m2 = GenericI32x8::splat_t(token, K_EXP_BIAS)
            - (m1.shr_arithmetic::<23>() * GenericI32x8::splat_t(token, K_EXP_MUL));
        let m2 = GenericF32x8::blend(
            m1.simd_eq(GenericI32x8::zero_t(token)).bitcast_f32x8(),
            GenericI32x8::zero_t(token).bitcast_f32x8(),
            m2.bitcast_f32x8(),
        );

        let mut r = m2;
        for _ in 0..3 {
            let r2 = r * r;
            r = (-xa_3).mul_add_portable(r2 * r2, k4_3 * r);
        }
        let r2 = r * r;
        r = k1_3.mul_add_portable((-xa).mul_add_portable(r2 * r2, r), r);
        let r2 = r * r;
        r2.mul_add_portable($x, $add)
    }};
}

// ===========================================================================
// XYB conversion (LinearRGBToXYB + MakePositiveXYB), planar f32x8
// ===========================================================================

#[magetypes(v3, neon, wasm128, scalar)]
fn planes_to_positive_xyb_inner(token: Token, p0: &mut [f32], p1: &mut [f32], p2: &mut [f32]) {
    type f32x8 = GenericF32x8<Token>;
    let n = p0.len();
    let chunks = n / 8;
    let (p0c, p0r) = f32x8::partition_slice_mut_t(token, p0);
    let (p1c, p1r) = f32x8::partition_slice_mut_t(token, p1);
    let (p2c, p2r) = f32x8::partition_slice_mut_t(token, p2);
    let _ = chunks;

    let m00 = f32x8::splat_t(token, M00);
    let m01 = f32x8::splat_t(token, M01);
    let m02 = f32x8::splat_t(token, M02);
    let m10 = f32x8::splat_t(token, M10);
    let m11 = f32x8::splat_t(token, M11);
    let m12 = f32x8::splat_t(token, M12);
    let m20 = f32x8::splat_t(token, M20);
    let m21 = f32x8::splat_t(token, M21);
    let m22 = f32x8::splat_t(token, M22);
    let bias = f32x8::splat_t(token, BIAS);
    let zero = f32x8::zero_t(token);
    let half = f32x8::splat_t(token, 0.5);
    let add_bias = f32x8::splat_t(token, NEG_CBRT_BIAS);
    let c055 = f32x8::splat_t(token, 0.55);
    let c14 = f32x8::splat_t(token, 14.0);
    let c042 = f32x8::splat_t(token, 0.42);
    let c001 = f32x8::splat_t(token, 0.01);

    for i in 0..p0c.len() {
        let r = f32x8::load_t(token, &p0c[i]);
        let g = f32x8::load_t(token, &p1c[i]);
        let b = f32x8::load_t(token, &p2c[i]);

        let mx0 = m00.mul_add_portable(r, m01.mul_add_portable(g, m02.mul_add_portable(b, bias)));
        let mx1 = m10.mul_add_portable(r, m11.mul_add_portable(g, m12.mul_add_portable(b, bias)));
        let mx2 = m20.mul_add_portable(r, m21.mul_add_portable(g, m22.mul_add_portable(b, bias)));

        let m0 = cube_root_and_add_x8!(token, mx0.max(zero), add_bias);
        let m1 = cube_root_and_add_x8!(token, mx1.max(zero), add_bias);
        let m2 = cube_root_and_add_x8!(token, mx2.max(zero), add_bias);

        // StoreXYB + MakePositiveXYB (x*14+0.42 unfused, matching reference)
        let x = half * (m0 - m1);
        let y = half * (m0 + m1);
        p0c[i] = (x * c14 + c042).to_array();
        p1c[i] = (y + c001).to_array();
        p2c[i] = ((m2 - y) + c055).to_array();
    }

    // Scalar tail — identical scalar sequence
    let off = p0c.len() * 8;
    for i in 0..p0r.len() {
        let px = xyb::linear_rgb_to_xyb_pixel([p0r[i], p1r[i], p2r[i]]);
        p0r[i] = px[0];
        p1r[i] = px[1];
        p2r[i] = px[2];
    }
    let _ = off;
}

/// `CubeRootLo` SIMD inner — `cbrt_lowp` (1 Halley, ~259 ulp).
#[magetypes(v3, neon, wasm128, scalar)]
fn planes_to_positive_xyb_lo_inner(token: Token, p0: &mut [f32], p1: &mut [f32], p2: &mut [f32]) {
    type f32x8 = GenericF32x8<Token>;
    let (p0c, p0r) = f32x8::partition_slice_mut_t(token, p0);
    let (p1c, p1r) = f32x8::partition_slice_mut_t(token, p1);
    let (p2c, p2r) = f32x8::partition_slice_mut_t(token, p2);

    let m00 = f32x8::splat_t(token, M00);
    let m01 = f32x8::splat_t(token, M01);
    let m02 = f32x8::splat_t(token, M02);
    let m10 = f32x8::splat_t(token, M10);
    let m11 = f32x8::splat_t(token, M11);
    let m12 = f32x8::splat_t(token, M12);
    let m20 = f32x8::splat_t(token, M20);
    let m21 = f32x8::splat_t(token, M21);
    let m22 = f32x8::splat_t(token, M22);
    let bias = f32x8::splat_t(token, BIAS);
    let zero = f32x8::zero_t(token);
    let half = f32x8::splat_t(token, 0.5);
    let add_bias = f32x8::splat_t(token, NEG_CBRT_BIAS);
    let c055 = f32x8::splat_t(token, 0.55);
    let c14 = f32x8::splat_t(token, 14.0);
    let c042 = f32x8::splat_t(token, 0.42);
    let c001 = f32x8::splat_t(token, 0.01);

    for i in 0..p0c.len() {
        let r = f32x8::load_t(token, &p0c[i]);
        let g = f32x8::load_t(token, &p1c[i]);
        let b = f32x8::load_t(token, &p2c[i]);

        let mx0 = m00.mul_add_portable(r, m01.mul_add_portable(g, m02.mul_add_portable(b, bias)));
        let mx1 = m10.mul_add_portable(r, m11.mul_add_portable(g, m12.mul_add_portable(b, bias)));
        let mx2 = m20.mul_add_portable(r, m21.mul_add_portable(g, m22.mul_add_portable(b, bias)));

        let m0 = mx0.max(zero).cbrt_lowp() + add_bias;
        let m1 = mx1.max(zero).cbrt_lowp() + add_bias;
        let m2 = mx2.max(zero).cbrt_lowp() + add_bias;

        let x = half * (m0 - m1);
        let y = half * (m0 + m1);
        p0c[i] = (x * c14 + c042).to_array();
        p1c[i] = (y + c001).to_array();
        p2c[i] = ((m2 - y) + c055).to_array();
    }

    // Scalar tail — same scalar sequence as `linear_rgb_to_xyb_pixel_hi`.
    for i in 0..p0r.len() {
        let px = xyb::linear_rgb_to_xyb_pixel_lo([p0r[i], p1r[i], p2r[i]]);
        p0r[i] = px[0];
        p1r[i] = px[1];
        p2r[i] = px[2];
    }
}

/// `CubeRootHi` SIMD inner — identical opsin mix; `cbrt_midp` (Kahan
/// seed + 2 Halley, max ~3 ulp) replaces the hwy Newton chain.
#[magetypes(v3, neon, wasm128, scalar)]
fn planes_to_positive_xyb_hi_inner(token: Token, p0: &mut [f32], p1: &mut [f32], p2: &mut [f32]) {
    type f32x8 = GenericF32x8<Token>;
    let (p0c, p0r) = f32x8::partition_slice_mut_t(token, p0);
    let (p1c, p1r) = f32x8::partition_slice_mut_t(token, p1);
    let (p2c, p2r) = f32x8::partition_slice_mut_t(token, p2);

    let m00 = f32x8::splat_t(token, M00);
    let m01 = f32x8::splat_t(token, M01);
    let m02 = f32x8::splat_t(token, M02);
    let m10 = f32x8::splat_t(token, M10);
    let m11 = f32x8::splat_t(token, M11);
    let m12 = f32x8::splat_t(token, M12);
    let m20 = f32x8::splat_t(token, M20);
    let m21 = f32x8::splat_t(token, M21);
    let m22 = f32x8::splat_t(token, M22);
    let bias = f32x8::splat_t(token, BIAS);
    let zero = f32x8::zero_t(token);
    let half = f32x8::splat_t(token, 0.5);
    let add_bias = f32x8::splat_t(token, NEG_CBRT_BIAS);
    let c055 = f32x8::splat_t(token, 0.55);
    let c14 = f32x8::splat_t(token, 14.0);
    let c042 = f32x8::splat_t(token, 0.42);
    let c001 = f32x8::splat_t(token, 0.01);

    for i in 0..p0c.len() {
        let r = f32x8::load_t(token, &p0c[i]);
        let g = f32x8::load_t(token, &p1c[i]);
        let b = f32x8::load_t(token, &p2c[i]);

        let mx0 = m00.mul_add_portable(r, m01.mul_add_portable(g, m02.mul_add_portable(b, bias)));
        let mx1 = m10.mul_add_portable(r, m11.mul_add_portable(g, m12.mul_add_portable(b, bias)));
        let mx2 = m20.mul_add_portable(r, m21.mul_add_portable(g, m22.mul_add_portable(b, bias)));

        let m0 = mx0.max(zero).cbrt_midp() + add_bias;
        let m1 = mx1.max(zero).cbrt_midp() + add_bias;
        let m2 = mx2.max(zero).cbrt_midp() + add_bias;

        let x = half * (m0 - m1);
        let y = half * (m0 + m1);
        p0c[i] = (x * c14 + c042).to_array();
        p1c[i] = (y + c001).to_array();
        p2c[i] = ((m2 - y) + c055).to_array();
    }

    // Scalar tail — same scalar sequence as `linear_rgb_to_xyb_pixel_hi`.
    for i in 0..p0r.len() {
        let px = xyb::linear_rgb_to_xyb_pixel_hi([p0r[i], p1r[i], p2r[i]]);
        p0r[i] = px[0];
        p1r[i] = px[1];
        p2r[i] = px[2];
    }
}

/// SIMD `planes_to_positive_xyb` — bit-identical to the scalar version.
pub fn planes_to_positive_xyb_simd(p: &mut [Vec<f32>; 3]) {
    let [p0, p1, p2] = &mut *p;
    incant!(
        planes_to_positive_xyb_inner(p0, p1, p2),
        [v3, neon, wasm128, scalar]
    );
}

/// `CubeRootHi` SIMD — `cbrt_midp` cube root (≤ ~3 ulp); NOT bit-exact
/// vs the reference recipe.
pub fn planes_to_positive_xyb_hi_simd(p: &mut [Vec<f32>; 3]) {
    let [p0, p1, p2] = &mut *p;
    incant!(
        planes_to_positive_xyb_hi_inner(p0, p1, p2),
        [v3, neon, wasm128, scalar]
    );
}

/// `CubeRootLo` SIMD — `cbrt_lowp` (1 Halley, ~259 ulp); experiment knob.
pub fn planes_to_positive_xyb_lo_simd(p: &mut [Vec<f32>; 3]) {
    let [p0, p1, p2] = &mut *p;
    incant!(
        planes_to_positive_xyb_lo_inner(p0, p1, p2),
        [v3, neon, wasm128, scalar]
    );
}

// ===========================================================================
// FastGaussian — lane-wise across rows (horizontal) and columns (vertical)
// ===========================================================================

macro_rules! gather8p {
    ($token:expr, $p:expr, $pb:expr, $row0:expr, $w:expr, $x:expr) => {{
        let (p, pb, row0, w, x) = ($p, $pb, $row0, $w, $x);
        match pb {
            Some(b) => GenericF32x8::from_array_t(
                $token,
                [
                    p[row0 * w + x] * b[row0 * w + x],
                    p[(row0 + 1) * w + x] * b[(row0 + 1) * w + x],
                    p[(row0 + 2) * w + x] * b[(row0 + 2) * w + x],
                    p[(row0 + 3) * w + x] * b[(row0 + 3) * w + x],
                    p[(row0 + 4) * w + x] * b[(row0 + 4) * w + x],
                    p[(row0 + 5) * w + x] * b[(row0 + 5) * w + x],
                    p[(row0 + 6) * w + x] * b[(row0 + 6) * w + x],
                    p[(row0 + 7) * w + x] * b[(row0 + 7) * w + x],
                ],
            ),
            None => GenericF32x8::from_array_t(
                $token,
                [
                    p[row0 * w + x],
                    p[(row0 + 1) * w + x],
                    p[(row0 + 2) * w + x],
                    p[(row0 + 3) * w + x],
                    p[(row0 + 4) * w + x],
                    p[(row0 + 5) * w + x],
                    p[(row0 + 6) * w + x],
                    p[(row0 + 7) * w + x],
                ],
            ),
        }
    }};
}

/// `FastGaussian1D` applied simultaneously to 8 rows (`row0..row0+8`) of a
/// plane — lane l runs the identical scalar sequence on row `row0 + l`, so
/// outputs are bit-identical to the scalar `fast_gaussian_1d` per row.
/// `out` is the destination chunk for exactly those 8 rows (`out.len() ==
/// 8 * width`).
#[magetypes(v3, neon, wasm128, scalar)]
fn fast_gaussian_1d_rows_inner(
    token: Token,
    rg: &RecursiveGaussian,
    plane: &[f32],
    plane_b: Option<&[f32]>,
    width: usize,
    row0: usize,
    out: &mut [f32],
) {
    type f32x8 = GenericF32x8<Token>;
    let w = width as i64;
    let n_radius = rg.radius as i64;
    let zero = f32x8::zero_t(token);

    let mut prev = [zero, zero, zero];
    let mut prev2 = [zero, zero, zero];

    let splat = |v: f32| f32x8::splat_t(token, v);

    macro_rules! step_bounds {
        ($n:expr) => {{
            let n = $n;
            let left = n - n_radius - 1;
            let right = n + n_radius - 1;
            let lv = if left >= 0 {
                gather8p!(token, plane, plane_b, row0, width, left as usize)
            } else {
                zero
            };
            let rv = if right < w {
                gather8p!(token, plane, plane_b, row0, width, right as usize)
            } else {
                zero
            };
            let sum = lv + rv;
            let mut o_arr = [zero; 3];
            for i in 0..3 {
                let mut o = sum * splat(rg.mul_in[i][0]);
                o = splat(rg.mul_prev2[i][0]).mul_add_portable(prev2[i], o);
                prev2[i] = prev[i];
                o = splat(rg.mul_prev[i][0]).mul_add_portable(prev[i], o);
                prev[i] = o;
                o_arr[i] = o;
            }
            if n >= 0 {
                let r = (o_arr[0] + (o_arr[1] + o_arr[2])).to_array();
                for l in 0..8 {
                    out[l * width + n as usize] = r[l];
                }
            }
        }};
    }

    let mut n = -n_radius + 1;
    let first_aligned = (n_radius + 1).div_euclid(4) * 4
        + if (n_radius + 1).rem_euclid(4) != 0 {
            4
        } else {
            0
        };
    while n < first_aligned.min(w) {
        step_bounds!(n);
        n += 1;
    }

    while n < w - n_radius + 1 - 3 {
        let base = n as usize;
        let off = n_radius as usize;
        let sum = [
            gather8p!(token, plane, plane_b, row0, width, base - off - 1)
                + gather8p!(token, plane, plane_b, row0, width, base + off - 1),
            gather8p!(token, plane, plane_b, row0, width, base - off)
                + gather8p!(token, plane, plane_b, row0, width, base + off),
            gather8p!(token, plane, plane_b, row0, width, base - off + 1)
                + gather8p!(token, plane, plane_b, row0, width, base + off + 1),
            gather8p!(token, plane, plane_b, row0, width, base - off + 2)
                + gather8p!(token, plane, plane_b, row0, width, base + off + 2),
        ];
        let mut o4 = [[zero; 3]; 4];
        for j in 0..4 {
            for i in 0..3 {
                let mut acc = sum[0] * splat(rg.mul_in[i][j]);
                if j >= 1 {
                    acc = splat(rg.mul_in[i][j - 1]).mul_add_portable(sum[1], acc);
                }
                if j >= 2 {
                    acc = splat(rg.mul_in[i][j - 2]).mul_add_portable(sum[2], acc);
                }
                if j >= 3 {
                    acc = splat(rg.mul_in[i][j - 3]).mul_add_portable(sum[3], acc);
                }
                acc = splat(rg.mul_prev2[i][j]).mul_add_portable(prev2[i], acc);
                acc = splat(rg.mul_prev[i][j]).mul_add_portable(prev[i], acc);
                o4[j][i] = acc;
            }
        }
        for i in 0..3 {
            prev2[i] = o4[2][i];
            prev[i] = o4[3][i];
        }
        for j in 0..4 {
            let r = (o4[j][0] + (o4[j][1] + o4[j][2])).to_array();
            for l in 0..8 {
                out[l * width + base + j] = r[l];
            }
        }
        n += 4;
    }

    while n < w {
        step_bounds!(n);
        n += 1;
    }
}

/// `FastGaussianVertical` across 8 columns — identical scalar sequence per
/// lane (ring buffer, warmup/border phases, NegMulSub ordering).
#[magetypes(v3, neon, wasm128, scalar)]
fn fast_gaussian_vertical_x8_inner(
    token: Token,
    rg: &RecursiveGaussian,
    input: &[f32],
    width: usize,
    height: usize,
    x0: usize,
    out: &mut [f32],
    out_off: usize,
    out_stride: usize,
) {
    type f32x8 = GenericF32x8<Token>;
    let n_rad = rg.radius as i64;
    let zero = f32x8::zero_t(token);
    let splat = |v: f32| f32x8::splat_t(token, v);

    let load = |row: usize| -> f32x8 {
        f32x8::from_slice_t(token, &input[row * width + x0..row * width + x0 + 8])
    };

    let mut y1_hist = [zero; 4];
    let mut y3_hist = [zero; 4];
    let mut y5_hist = [zero; 4];
    let mut ctr: usize = 0;

    let mut n: i64 = -n_rad + 1;
    while n < height as i64 {
        let top = n - n_rad - 1;
        let bottom = n + n_rad - 1;
        let sum = if top < 0 {
            if bottom < height as i64 {
                load(bottom as usize)
            } else {
                zero
            }
        } else {
            load(top as usize)
                + if bottom < height as i64 {
                    load(bottom as usize)
                } else {
                    zero
                }
        };

        ctr = ctr.wrapping_add(1);
        let n_0 = ctr % 4;
        let n_1 = (ctr.wrapping_sub(1)) % 4;
        let n_2 = (ctr.wrapping_sub(2)) % 4;

        let y1 = splat(rg.n2[0]).mul_add_portable(
            sum,
            splat(-rg.d1[0]).mul_add_portable(y1_hist[n_1], -y1_hist[n_2]),
        );
        let y3 = splat(rg.n2[1]).mul_add_portable(
            sum,
            splat(-rg.d1[1]).mul_add_portable(y3_hist[n_1], -y3_hist[n_2]),
        );
        let y5 = splat(rg.n2[2]).mul_add_portable(
            sum,
            splat(-rg.d1[2]).mul_add_portable(y5_hist[n_1], -y5_hist[n_2]),
        );
        y1_hist[n_0] = y1;
        y3_hist[n_0] = y3;
        y5_hist[n_0] = y5;

        if n >= 0 {
            let arr = (y1 + (y3 + y5)).to_array();
            out[n as usize * out_stride + out_off..n as usize * out_stride + out_off + 8]
                .copy_from_slice(&arr);
        }
        n += 1;
    }
}

/// Full `FastGaussian` (horizontal then vertical) with per-lane semantics
/// identical to the scalar port — outputs are bit-identical.
pub fn fast_gaussian_simd(
    rg: &RecursiveGaussian,
    input: &[f32],
    plane_b: Option<&[f32]>,
    width: usize,
    height: usize,
    out: &mut [f32],
    tmp: &mut [f32],
) {
    // Unstoppable never fires — the only effect is a few strided checks.
    let _ = fast_gaussian_simd_stop(
        rg,
        input,
        plane_b,
        width,
        height,
        out,
        tmp,
        &enough::Unstoppable,
    );
}

/// [`fast_gaussian_simd`] with cooperative cancellation — `stop` is
/// checked between the horizontal and vertical passes and inside the
/// (sequential even under `rayon`) vertical column-block loop.
pub(crate) fn fast_gaussian_simd_stop(
    rg: &RecursiveGaussian,
    input: &[f32],
    plane_b: Option<&[f32]>,
    width: usize,
    height: usize,
    out: &mut [f32],
    tmp: &mut [f32],
    stop: &dyn enough::Stop,
) -> Result<(), enough::StopReason> {
    // `may_stop` collapses Unstoppable to a None check in the loops below.
    let stop = stop.may_stop().then_some(stop);
    stop.check()?;
    // Horizontal: 8-row blocks + scalar tail rows.
    // Each block is an independent lane-group — bit-exact under
    // `rayon` (disjoint output chunks, identical per-lane math).
    let rows8 = height / 8 * 8;
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        tmp[..rows8 * width]
            .par_chunks_exact_mut(width * 8)
            .enumerate()
            .try_for_each(|(block, chunk)| {
                stop.check()?;
                incant!(
                    fast_gaussian_1d_rows_inner(rg, input, plane_b, width, block * 8, chunk),
                    [v3, neon, wasm128, scalar]
                );
                Ok::<(), enough::StopReason>(())
            })?;
    }
    #[cfg(not(feature = "rayon"))]
    for row0 in (0..rows8).step_by(8) {
        if row0 & (STOP_ROW_STRIDE - 1) == 0 {
            stop.check()?;
        }
        incant!(
            fast_gaussian_1d_rows_inner(rg, input, plane_b, width, row0, &mut tmp[row0 * width..]),
            [v3, neon, wasm128, scalar]
        )
    }
    {
        let mut rowbuf = Vec::new();
        for row in rows8..height {
            let src: &[f32] = match plane_b {
                Some(bb) => {
                    rowbuf.clear();
                    rowbuf.extend(
                        input[row * width..row * width + width]
                            .iter()
                            .zip(&bb[row * width..])
                            .map(|(x, y)| x * y),
                    );
                    &rowbuf
                }
                None => &input[row * width..row * width + width],
            };
            rg.fast_gaussian_1d(src, &mut tmp[row * width..(row + 1) * width]);
        }
    }

    stop.check()?;

    // Vertical: 8-column blocks + scalar tail columns. Each block's IIR
    // runs down its own columns — fully independent — but writes land
    // strided inside `out`, so the blocks can't borrow disjoint slices
    // (and `forbid(unsafe_code)` rules out a shared-mutable split).
    // Under `rayon` each block computes into private scratch
    // (out_off=0, out_stride=8 → contiguous) and a parallel scatter
    // copies the row strips into place. Bit-identical: same inner call,
    // same column writes, different intermediate buffer.
    let cols8 = width / 8 * 8;
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        let nblocks = cols8 / 8;
        let block_outs: Vec<Vec<f32>> = (0..nblocks)
            .into_par_iter()
            .map(|b| -> Result<Vec<f32>, enough::StopReason> {
                stop.check()?;
                let mut bo = vec![0f32; height * 8];
                incant!(
                    fast_gaussian_vertical_x8_inner(rg, tmp, width, height, b * 8, &mut bo, 0, 8),
                    [v3, neon, wasm128, scalar]
                );
                Ok(bo)
            })
            .collect::<Result<_, _>>()?;
        // Scatter: row `r` of `out`'s column range [0..cols8) is the
        // concatenation of every block's row — a parallel row-major copy.
        out.par_chunks_mut(width)
            .take(height)
            .enumerate()
            .for_each(|(r, orow)| {
                for (b, bo) in block_outs.iter().enumerate() {
                    orow[b * 8..b * 8 + 8].copy_from_slice(&bo[r * 8..r * 8 + 8]);
                }
            });
    }
    #[cfg(not(feature = "rayon"))]
    for x0 in (0..cols8).step_by(8) {
        if x0 & (STOP_COL_STRIDE - 1) == 0 {
            stop.check()?;
        }
        incant!(
            fast_gaussian_vertical_x8_inner(rg, tmp, width, height, x0, out, x0, width),
            [v3, neon, wasm128, scalar]
        )
    }
    if cols8 < width {
        rg.fast_gaussian_vertical_1d(
            width,
            height,
            |r, x| if x >= cols8 { tmp[r * width + x] } else { 0.0 },
            &mut |r, x, v| {
                if x >= cols8 {
                    out[r * width + x] = v;
                }
            },
        );
    }
    Ok(())
}

/// Plane-level wrapper: 3 channels through `fast_gaussian_simd`.
///
/// With `rayon` the channels run in parallel (disjoint `out` planes,
/// per-channel scratch) — identical per-lane math, bit-exact.
pub fn blur_planes_simd(
    rg: &RecursiveGaussian,
    p: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> [Vec<f32>; 3] {
    let np = width * height;
    let mut out = [vec![0f32; np], vec![0f32; np], vec![0f32; np]];
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        let [o0, o1, o2] = &mut out;
        let outs = [o0.as_mut_slice(), o1.as_mut_slice(), o2.as_mut_slice()];
        outs.into_par_iter().enumerate().for_each(|(c, oc)| {
            let mut tmp = vec![0f32; np];
            fast_gaussian_simd(rg, &p[c], None, width, height, oc, &mut tmp);
        });
    }
    #[cfg(not(feature = "rayon"))]
    {
        let mut tmp = vec![0f32; np];
        for c in 0..3 {
            fast_gaussian_simd(rg, &p[c], None, width, height, &mut out[c], &mut tmp);
        }
    }
    out
}

/// `blur_planes_simd` writing into caller-provided `out` + `tmp` scratch
/// (per-channel `tmp` planes; sized ≥ width*height). Lets strip callers
/// reuse allocations across strips instead of mmap-faulting ~5 fresh
/// planes per blur call.
pub fn blur_planes_simd_into(
    rg: &RecursiveGaussian,
    p: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    out: &mut [Vec<f32>; 3],
    tmp: &mut [Vec<f32>; 3],
) {
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        let [o0, o1, o2] = out;
        let [t0, t1, t2] = tmp;
        [(o0, t0), (o1, t1), (o2, t2)]
            .into_par_iter()
            .enumerate()
            .for_each(|(c, (oc, tc))| {
                fast_gaussian_simd(rg, &p[c], None, width, height, oc, tc);
            });
    }
    #[cfg(not(feature = "rayon"))]
    {
        for c in 0..3 {
            fast_gaussian_simd(rg, &p[c], None, width, height, &mut out[c], &mut tmp[0]);
        }
    }
}

// ===========================================================================
// SSIMMap / EdgeDiffMap — f32 per-pixel terms in lanes; f64 accumulation
// stays sequential (ordering-sensitive).
// ===========================================================================

/// SIMD `ssim_map` — identical per-pixel math; accumulation order preserved
/// (terms are computed in lanes, then added to the f64 sums in index order).
#[magetypes(v3, neon, wasm128, scalar)]
pub fn ssim_map_inner(
    token: Token,
    m1: &[f32],
    m2: &[f32],
    s11: &[f32],
    s22: &[f32],
    s12: &[f32],
    sum0: &mut f64,
    sum1: &mut f64,
) {
    type f32x8 = GenericF32x8<Token>;
    let (m1c, m1r) = f32x8::partition_slice_t(token, m1);
    let (m2c, m2r) = f32x8::partition_slice_t(token, m2);
    let (s11c, s11r) = f32x8::partition_slice_t(token, s11);
    let (s22c, s22r) = f32x8::partition_slice_t(token, s22);
    let (s12c, s12r) = f32x8::partition_slice_t(token, s12);
    let kc2 = f32x8::splat_t(token, super::maps::K_C2);
    let two = f32x8::splat_t(token, 2.0);
    let one = f32x8::splat_t(token, 1.0);

    let mut i_base = 0usize;
    for i in 0..m1c.len() {
        let a = f32x8::load_t(token, &m1c[i]);
        let b = f32x8::load_t(token, &m2c[i]);
        let mu11 = a * a;
        let mu22 = b * b;
        let mu12 = a * b;
        let dm = a - b;
        // official f32 sequence, lane-wise
        let num_m = (-dm).mul_add_portable(dm, one);
        let num_s = two * (f32x8::load_t(token, &s12c[i]) - mu12) + kc2;
        let denom_s =
            (f32x8::load_t(token, &s11c[i]) - mu11) + (f32x8::load_t(token, &s22c[i]) - mu22) + kc2;
        let q = num_m * num_s / denom_s;
        let qa = q.to_array();
        for l in 0..8 {
            let d = (1.0f64 - qa[l] as f64).max(0.0);
            *sum0 += d;
            *sum1 += super::maps::tothe4th(d);
        }
        i_base += 8;
    }
    let _ = i_base;
    for i in 0..m1r.len() {
        let mu1 = m1r[i];
        let mu2 = m2r[i];
        let mu11 = mu1 * mu1;
        let mu22 = mu2 * mu2;
        let mu12 = mu1 * mu2;
        let dm = mu1 - mu2;
        let num_m = (-dm).fma(dm, 1.0f32);
        let num_s = 2.0f32 * (s12r[i] - mu12) + super::maps::K_C2;
        let denom_s = (s11r[i] - mu11) + (s22r[i] - mu22) + super::maps::K_C2;
        let q = num_m * num_s / denom_s;
        let d = (1.0f64 - q as f64).max(0.0);
        *sum0 += d;
        *sum1 += super::maps::tothe4th(d);
    }
}

/// SIMD `edge_diff_map` — f32 abs-diffs in lanes; the f64 division and
/// accumulation are per-lane f64 (identical per-pixel value), accumulated in
/// index order.
#[magetypes(v3, neon, wasm128, scalar)]
fn edge_diff_map_inner(
    token: Token,
    img1: &[f32],
    mu1: &[f32],
    img2: &[f32],
    mu2: &[f32],
    sums: &mut [f64; 4],
) {
    type f32x8 = GenericF32x8<Token>;
    type f64x4 = magetypes::simd::generic::f64x4<Token>;
    let (i1c, i1r) = f32x8::partition_slice_t(token, img1);
    let (m1c, m1r) = f32x8::partition_slice_t(token, mu1);
    let (i2c, i2r) = f32x8::partition_slice_t(token, img2);
    let (m2c, m2r) = f32x8::partition_slice_t(token, mu2);
    let one4 = f64x4::splat_t(token, 1.0);

    for i in 0..i1c.len() {
        // |img - mu| per lane, widened to f64 for the division.
        let da = (f32x8::load_t(token, &i2c[i]) - f32x8::load_t(token, &m2c[i]))
            .abs()
            .to_array();
        let db = (f32x8::load_t(token, &i1c[i]) - f32x8::load_t(token, &m1c[i]))
            .abs()
            .to_array();
        // widen: two f64x4 each
        let num_lo = f64x4::from_array_t(
            token,
            [
                1.0 + da[0] as f64,
                1.0 + da[1] as f64,
                1.0 + da[2] as f64,
                1.0 + da[3] as f64,
            ],
        );
        let num_hi = f64x4::from_array_t(
            token,
            [
                1.0 + da[4] as f64,
                1.0 + da[5] as f64,
                1.0 + da[6] as f64,
                1.0 + da[7] as f64,
            ],
        );
        let den_lo = f64x4::from_array_t(
            token,
            [
                1.0 + db[0] as f64,
                1.0 + db[1] as f64,
                1.0 + db[2] as f64,
                1.0 + db[3] as f64,
            ],
        );
        let den_hi = f64x4::from_array_t(
            token,
            [
                1.0 + db[4] as f64,
                1.0 + db[5] as f64,
                1.0 + db[6] as f64,
                1.0 + db[7] as f64,
            ],
        );
        let q_lo = num_lo / den_lo - one4;
        let q_hi = num_hi / den_hi - one4;
        let d_lo = q_lo.to_array();
        let d_hi = q_hi.to_array();
        for v in [d_lo, d_hi].iter().flat_map(|a| a.iter()) {
            let artifact = v.max(0.0);
            sums[0] += artifact;
            sums[1] += super::maps::tothe4th(artifact);
            let detail_lost = (-*v).max(0.0);
            sums[2] += detail_lost;
            sums[3] += super::maps::tothe4th(detail_lost);
        }
    }
    for i in 0..i1r.len() {
        let num = 1.0 + (i2r[i] - m2r[i]).abs() as f64;
        let den = 1.0 + (i1r[i] - m1r[i]).abs() as f64;
        let d1 = num / den - 1.0;
        let artifact = d1.max(0.0);
        sums[0] += artifact;
        sums[1] += super::maps::tothe4th(artifact);
        let detail_lost = (-d1).max(0.0);
        sums[2] += detail_lost;
        sums[3] += super::maps::tothe4th(detail_lost);
    }
}

/// `ssim_map` via lanes — returns the same `[f64; 6]` aggregates.
pub fn ssim_map_simd(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> [f64; 6] {
    let one_per_pixels = 1.0 / (height * width) as f64;
    let mut out = [0f64; 6];
    for c in 0..3 {
        let mut sum0 = 0f64;
        let mut sum1 = 0f64;
        incant!(
            ssim_map_inner(
                &m1[c], &m2[c], &s11[c], &s22[c], &s12[c], &mut sum0, &mut sum1
            ),
            [v3, neon, wasm128, scalar]
        );
        out[c * 2] = one_per_pixels * sum0;
        out[c * 2 + 1] = (one_per_pixels * sum1).sqrt().sqrt();
    }
    out
}

/// Fused ssim+edge inner: per-8-pixel chunk computes the ssim terms in
/// lanes then the edge terms scalar — each map's f64 accumulation order
/// is per-pixel sequential, bit-exact vs the separate scalar fns.
#[magetypes(v3, neon, wasm128, scalar)]
fn maps_fused_inner(
    token: Token,
    m1: &[f32],
    m2: &[f32],
    s11: &[f32],
    s22: &[f32],
    s12: &[f32],
    img1: &[f32],
    img2: &[f32],
    sum0: &mut f64,
    sum1: &mut f64,
    esums: &mut [f64; 4],
) {
    type f32x8 = GenericF32x8<Token>;
    let (m1c, _m1r) = f32x8::partition_slice_t(token, m1);
    let (m2c, _m2r) = f32x8::partition_slice_t(token, m2);
    let (s11c, _s11r) = f32x8::partition_slice_t(token, s11);
    let (s22c, _s22r) = f32x8::partition_slice_t(token, s22);
    let (s12c, _s12r) = f32x8::partition_slice_t(token, s12);
    let (i1c, _i1r) = f32x8::partition_slice_t(token, img1);
    let (i2c, _i2r) = f32x8::partition_slice_t(token, img2);
    let kc2 = f32x8::splat_t(token, super::maps::K_C2);
    let two = f32x8::splat_t(token, 2.0);
    let one = f32x8::splat_t(token, 1.0);
    let nchunk = m1c.len();
    for i in 0..nchunk {
        let a = f32x8::load_t(token, &m1c[i]);
        let b = f32x8::load_t(token, &m2c[i]);
        let mu11 = a * a;
        let mu22 = b * b;
        let mu12 = a * b;
        let dm = a - b;
        let num_m = (-dm).mul_add_portable(dm, one);
        let num_s = two * (f32x8::load_t(token, &s12c[i]) - mu12) + kc2;
        let denom_s =
            (f32x8::load_t(token, &s11c[i]) - mu11) + (f32x8::load_t(token, &s22c[i]) - mu22) + kc2;
        let q = num_m * num_s / denom_s;
        let qa = q.to_array();
        for l in 0..8 {
            let d = (1.0f64 - qa[l] as f64).max(0.0);
            *sum0 += d;
            *sum1 += super::maps::tothe4th(d);
        }
        let i1 = i1c[i];
        let i2 = i2c[i];
        let m1a = m1c[i];
        let m2a = m2c[i];
        for l in 0..8 {
            let num = 1.0 + (i2[l] - m2a[l]).abs() as f64;
            let den = 1.0 + (i1[l] - m1a[l]).abs() as f64;
            let d1 = num / den - 1.0;
            let artifact = d1.max(0.0);
            esums[0] += artifact;
            esums[1] += super::maps::tothe4th(artifact);
            let detail_lost = (-d1).max(0.0);
            esums[2] += detail_lost;
            esums[3] += super::maps::tothe4th(detail_lost);
        }
    }
    for i in (nchunk * 8)..m1.len() {
        let mu1 = m1[i];
        let mu2 = m2[i];
        let mu11 = mu1 * mu1;
        let mu22 = mu2 * mu2;
        let mu12 = mu1 * mu2;
        let dm = mu1 - mu2;
        let num_m = (-dm).fma(dm, 1.0f32);
        let num_s = 2.0f32 * (s12[i] - mu12) + super::maps::K_C2;
        let denom_s = (s11[i] - mu11) + (s22[i] - mu22) + super::maps::K_C2;
        let q = num_m * num_s / denom_s;
        let d = (1.0f64 - q as f64).max(0.0);
        *sum0 += d;
        *sum1 += super::maps::tothe4th(d);
        let num = 1.0 + (img2[i] - mu2).abs() as f64;
        let den = 1.0 + (img1[i] - mu1).abs() as f64;
        let d1 = num / den - 1.0;
        let artifact = d1.max(0.0);
        esums[0] += artifact;
        esums[1] += super::maps::tothe4th(artifact);
        let detail_lost = (-d1).max(0.0);
        esums[2] += detail_lost;
        esums[3] += super::maps::tothe4th(detail_lost);
    }
}

/// Fused `ssim_map` + `edge_diff_map`, SIMD ssim lanes + scalar edge
/// per-pixel — returns (`[f64; 6]`, `[f64; 12]`) bit-exact vs separate calls.
pub fn maps_fused_simd(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    img1: &[Vec<f32>; 3],
    img2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> ([f64; 6], [f64; 12]) {
    // Unstoppable never fires — the only effect is strided checks.
    match maps_fused_simd_stop(
        m1,
        m2,
        s11,
        s22,
        s12,
        img1,
        img2,
        width,
        height,
        &enough::Unstoppable,
    ) {
        Ok(o) => o,
        Err(_) => unreachable!("Unstoppable never stops"),
    }
}

/// [`maps_fused_simd`] with cooperative cancellation — `stop` is checked
/// per channel and per `MAPS_SIMD_STOP_STRIDE`-pixel chunk. Each chunk
/// calls the inner kernel on sub-slices; per-pixel math is independent
/// and the f64 accumulators stay in index order → bit-exact.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maps_fused_simd_stop(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    img1: &[Vec<f32>; 3],
    img2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    stop: &dyn enough::Stop,
) -> Result<([f64; 6], [f64; 12]), enough::StopReason> {
    let stop = stop.may_stop().then_some(stop);
    let (mut so, mut eo) = ([0f64; 6], [0f64; 12]);
    for c in 0..3 {
        let (ssim, edge) = maps_channel_simd_stop(
            &m1[c], &m2[c], &s11[c], &s22[c], &s12[c], &img1[c], &img2[c], width, height, stop,
        )?;
        so[c * 2..c * 2 + 2].copy_from_slice(&ssim);
        eo[c * 4..c * 4 + 4].copy_from_slice(&edge);
    }
    Ok((so, eo))
}

/// One channel of [`maps_fused_simd_stop`]. pngdeer patch: split out so the low-memory comparison,
/// which scores channel by channel, runs this same code.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maps_channel_simd_stop(
    m1: &[f32],
    m2: &[f32],
    s11: &[f32],
    s22: &[f32],
    s12: &[f32],
    img1: &[f32],
    img2: &[f32],
    width: usize,
    height: usize,
    stop: Option<&dyn enough::Stop>,
) -> Result<([f64; 2], [f64; 4]), enough::StopReason> {
    let one_per_pixels = 1.0 / (height * width) as f64;
    let npix = width * height;
    let (mut sum0, mut sum1) = (0f64, 0f64);
    let mut esums = [0f64; 4];
    for off in (0..npix).step_by(MAPS_SIMD_STOP_STRIDE) {
        let end = npix.min(off + MAPS_SIMD_STOP_STRIDE);
        incant!(
            maps_fused_inner(
                &m1[off..end],
                &m2[off..end],
                &s11[off..end],
                &s22[off..end],
                &s12[off..end],
                &img1[off..end],
                &img2[off..end],
                &mut sum0,
                &mut sum1,
                &mut esums
            ),
            [v3, neon, wasm128, scalar]
        );
        stop.check()?;
    }
    Ok((
        [one_per_pixels * sum0, (one_per_pixels * sum1).sqrt().sqrt()],
        [
            one_per_pixels * esums[0],
            (one_per_pixels * esums[1]).sqrt().sqrt(),
            one_per_pixels * esums[2],
            (one_per_pixels * esums[3]).sqrt().sqrt(),
        ],
    ))
}

/// `edge_diff_map` via lanes — returns the same `[f64; 12]` aggregates.
pub fn edge_diff_map_simd(
    img1: &[Vec<f32>; 3],
    mu1: &[Vec<f32>; 3],
    img2: &[Vec<f32>; 3],
    mu2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> [f64; 12] {
    let one_per_pixels = 1.0 / (height * width) as f64;
    let mut out = [0f64; 12];
    for c in 0..3 {
        let mut sums = [0f64; 4];
        incant!(
            edge_diff_map_inner(&img1[c], &mu1[c], &img2[c], &mu2[c], &mut sums),
            [v3, neon, wasm128, scalar]
        );
        out[c * 4] = one_per_pixels * sums[0];
        out[c * 4 + 1] = (one_per_pixels * sums[1]).sqrt().sqrt();
        out[c * 4 + 2] = one_per_pixels * sums[2];
        out[c * 4 + 3] = (one_per_pixels * sums[3]).sqrt().sqrt();
    }
    out
}

#[cfg(test)]
mod stop_tests {
    use super::*;

    struct Cancel;
    impl enough::Stop for Cancel {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }

    #[test]
    fn cancelled_blur_does_not_start_horizontal_pass() {
        let (w, h) = (32, 32);
        let input = vec![1.0; w * h];
        let mut out = vec![-7.0; w * h];
        let mut tmp = vec![-7.0; w * h];
        let rg = super::super::gauss::create_recursive_gaussian(1.5);
        assert_eq!(
            fast_gaussian_simd_stop(&rg, &input, None, w, h, &mut out, &mut tmp, &Cancel),
            Err(enough::StopReason::Cancelled)
        );
        assert!(tmp.iter().all(|&v| v == -7.0));
        assert!(out.iter().all(|&v| v == -7.0));
    }
}
