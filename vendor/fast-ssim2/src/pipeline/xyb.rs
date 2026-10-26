#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]
//! Bit-exact port of the reference `LinearRGBToXYB` + `MakePositiveXYB`.
//!
//! The opsin absorbance FMA chain and `StoreXYB` layout match our fast path
//! exactly; the difference is the cube root: the reference uses
//! `CubeRootAndAdd` (an integer-exponent trick plus three Newton iterations
//! and a final Halley-style correction, ~6 ulp error), not `cbrtf`/`cbrt`.

// Opsin absorbance matrix (frozen constants in both implementations).
pub(crate) const M00: f32 = 0.30;
pub(crate) const M01: f32 = 1.0 - 0.078 - 0.30;
pub(crate) const M02: f32 = 0.078;
pub(crate) const M10: f32 = 0.23;
pub(crate) const M11: f32 = 1.0 - 0.078 - 0.23;
pub(crate) const M12: f32 = 0.078;
pub(crate) const M20: f32 = 0.243_422_69; // f32(0.24342268924547819)
pub(crate) const M21: f32 = 0.204_767_45; // f32(0.20476744424496821)
pub(crate) const M22: f32 = 1.0 - M20 - M21;

/// Opsin absorbance bias and its libm `cbrtf` (what the reference uses for
/// `premul_absorb[9..12]` via `-cbrtf(kOpsinAbsorbanceBias[i])`).
pub(crate) const BIAS: f32 = 0.003_793_073_4; // f32(0.0037930732552754493)
pub(crate) const NEG_CBRT_BIAS: f32 = f32::from_bits(0xbe1f_b275); // -cbrtf(BIAS) via libm

/// Port of hwy's `CubeRootAndAdd` — the exact f32 op sequence, no f64.
/// Returns approximately `x.cbrt() + add` with ~6 ulp error.
#[allow(unused_imports)]
use super::Fma; // pngdeer patch
#[inline]
fn cube_root_and_add(x: f32, add: f32) -> f32 {
    const K_EXP_BIAS: i32 = 0x5480_0000; // cast(1.) + cast(1.) / 3
    const K_EXP_MUL: i32 = 0x002a_aaaa; // shifted 1/3
    const K1_3: f32 = 1.0 / 3.0;
    const K4_3: f32 = 4.0 / 3.0;

    let xa = x; // assume inputs never negative
    let xa_3 = K1_3 * xa;

    let m1 = xa.to_bits() as i32;
    let m2 = if m1 == 0 {
        0
    } else {
        K_EXP_BIAS.wrapping_sub((m1 >> 23).wrapping_mul(K_EXP_MUL))
    };
    let mut r = f32::from_bits(m2 as u32);

    // Newton-Raphson iterations
    for _ in 0..3 {
        let r2 = r * r;
        // NegMulAdd(xa_3, r2*r2, k4_3*r) = fma(-xa_3, r2*r2, k4_3*r)
        r = (-xa_3).fma(r2 * r2, K4_3 * r);
    }
    // Final iteration
    let r2 = r * r;
    // MulAdd(k1_3, NegMulAdd(xa, r2*r2, r), r)
    r = K1_3.fma((-xa).fma(r2 * r2, r), r);
    let r2 = r * r;
    r2.fma(x, add)
}

/// `LinearRGBToXYB` + `MakePositiveXYB` for one pixel triple — uses the
/// reference's `CubeRootAndAdd` bit-hack + Newton chain (not `f32::cbrt`),
/// which is part of the bit-exactness contract.
/// Input is linear sRGB (already EOTF-decoded) in [0, ~1.05].
#[inline]
pub fn linear_rgb_to_xyb_pixel(px: [f32; 3]) -> [f32; 3] {
    linear_rgb_to_xyb_with(px, |v| cube_root_and_add(v.max(0.0), NEG_CBRT_BIAS))
}

/// `CubeRootLo` variant — `cbrt_lowp_f32` (1 Halley, ~259 ulp).
#[inline]
pub fn linear_rgb_to_xyb_pixel_lo(px: [f32; 3]) -> [f32; 3] {
    linear_rgb_to_xyb_with(px, |v| {
        magetypes::nostd_math::cbrt_lowp_f32(v.max(0.0)) + NEG_CBRT_BIAS
    })
}

/// `CubeRootHi` variant — `magetypes::nostd_math::cbrt_midp_f32`
/// (Kahan seed + 2 Halley, max ~3 ulp). The `+ BIAS` add is a plain
/// f32 add (hwy fuses it into the final step; an honest root + add is
/// the defensible semantic here).
#[inline]
pub fn linear_rgb_to_xyb_pixel_hi(px: [f32; 3]) -> [f32; 3] {
    linear_rgb_to_xyb_with(px, |v| {
        magetypes::nostd_math::cbrt_midp_f32(v.max(0.0)) + NEG_CBRT_BIAS
    })
}

#[inline]
fn linear_rgb_to_xyb_with(px: [f32; 3], root: impl Fn(f32) -> f32) -> [f32; 3] {
    let (r, g, b) = (px[0], px[1], px[2]);
    let mixed0 = M00.fma(r, M01.fma(g, M02.fma(b, BIAS)));
    let mixed1 = M10.fma(r, M11.fma(g, M12.fma(b, BIAS)));
    let mixed2 = M20.fma(r, M21.fma(g, M22.fma(b, BIAS)));

    let m0 = root(mixed0);
    let m1 = root(mixed1);
    let m2 = root(mixed2);

    // StoreXYB
    let x = 0.5 * (m0 - m1);
    let y = 0.5 * (m0 + m1);
    let bb = m2;

    // MakePositiveXYB
    let b_pos = (bb - y) + 0.55;
    let x_pos = x * 14.0 + 0.42;
    let y_pos = y + 0.01;
    [x_pos, y_pos, b_pos]
}
