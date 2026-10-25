//! PU21-integrated XYB for HDR input (`hdr-pu` feature).
//!
//! Replaces the cube-root opsin nonlinearity with PU21 (Mantiuk & Azimi,
//! PCS 2021) at the perceptual-encoding layer, consuming **absolute
//! luminance** linear RGB in cd/m². This is the correct way to PU-adapt a
//! metric with its own perceptual transform — feeding PU-encoded values as
//! *input* lets the cube-root re-process already-uniform PU values and caps
//! HDR correlation (measured SROCC 0.59–0.61 on UPIQ HDR vs ~0.69 for the
//! integrated form; PU-SSIM, the strongest non-learned HDR baseline, is
//! luminance-only).
//!
//! Recipe: opsin LMS mix on absolute nits → `PU21(banding_glare)` per
//! channel (100-nit reference white → ~1.0, the range the cube-root white
//! point sits in) → opponent formation with the positive offsets folded in.
//! The X chroma scale is 4 (not the cube-root path's 14): PU-space opsin
//! differences are already large, and HDR validation favors
//! luminance-dominant weighting.
//!
//! Transfer decoders (PQ EOTF, HLG inverse-OETF + display gamma) are the
//! ITU-R BT.2100 / SMPTE ST 2084 published constants — same values as
//! `zenmetrics-api::hdr` and `zensim::{transfer, pu21}`.

/// PU21 `banding_glare` parameters `[p1..p7]` (gfxdisp/pu21, 2020-02-06).
#[allow(clippy::excessive_precision)]
const PU21_P: [f32; 7] = [
    0.353_487_901,
    0.373_465_862_9,
    8.277_049_286e-5,
    0.906_256_262_7,
    0.091_503_031_66,
    0.909_951_720_4,
    596.314_814_2,
];
/// Operating range of the encoding (cd/m²).
const PU21_L_MIN: f32 = 0.005;
const PU21_L_MAX: f32 = 10000.0;
/// PU21(100 cd/m²) — normalizes a 100-nit reference white to ~1.0.
const PU_WHITE: f32 = 256.3;
/// Opponent X amplification in PU space (cube-root path uses 14).
const PU_X_SCALE: f32 = 4.0;

/// PU21 encode: absolute luminance (cd/m²) → perceptually-uniform value.
/// `V = max(p7·(((p1 + p2·Y^p4)/(1 + p3·Y^p4))^p5 − p6), 0)`, `Y` clamped.
#[inline]
pub(crate) fn pu21_encode(y: f32) -> f32 {
    let y = y.clamp(PU21_L_MIN, PU21_L_MAX);
    let yp = y.powf(PU21_P[3]);
    let inner = (PU21_P[0] + PU21_P[1] * yp) / (1.0 + PU21_P[2] * yp);
    (PU21_P[6] * (inner.powf(PU21_P[4]) - PU21_P[5])).max(0.0)
}

/// SMPTE ST 2084 (PQ) EOTF: code value `[0,1]` → absolute luminance
/// `[0, 10000]` cd/m².
#[inline]
fn pq_eotf(v: f32) -> f32 {
    const L_MAX: f32 = 10000.0;
    const M1: f32 = 0.159_301_75;
    const M2: f32 = 78.843_75;
    const C1: f32 = 0.835_937_5;
    const C2: f32 = 18.851_562;
    const C3: f32 = 18.687_5;
    let im = v.powf(1.0 / M2);
    let num = (im - C1).max(0.0);
    let den = C2 - C3 * im;
    L_MAX * (num / den).powf(1.0 / M1)
}

/// ITU-R BT.2100 HLG inverse-OETF: `v ∈ [0,1]` → scene-relative linear.
#[inline]
fn hlg_inverse_oetf(v: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 1.0 - 4.0 * A;
    const C: f32 = 0.559_910_7;
    if v <= 0.5 {
        (v * v) / 3.0
    } else {
        (((v - C) / A).exp() + B) / 12.0
    }
}

/// HLG display luminance for the fixed 1000-nit reference display:
/// `inverse_oetf(v)` is scene-relative; the OOTF applies system gamma 1.2
/// to the *scene* luminance, which scales each channel's relative value.
#[inline]
fn hlg_to_nits(rgb: [f32; 3]) -> [f32; 3] {
    const GAMMA: f32 = 1.2;
    const PEAK: f32 = 1000.0;
    let lin = [
        hlg_inverse_oetf(rgb[0]),
        hlg_inverse_oetf(rgb[1]),
        hlg_inverse_oetf(rgb[2]),
    ];
    // OOTF: Y_scene^(γ−1) applied to each channel (BT.2100 formulation).
    let y_scene = 0.2627f32.mul_add(lin[0], 0.6780f32.mul_add(lin[1], 0.0593 * lin[2]));
    let scale = y_scene.powf(GAMMA - 1.0);
    [
        lin[0] * scale * PEAK,
        lin[1] * scale * PEAK,
        lin[2] * scale * PEAK,
    ]
}

/// Per-pixel HLG triple → nits (channel-dependent OOTF).
#[inline]
pub(crate) fn hlg_triple_to_nits(rgb: [f32; 3]) -> [f32; 3] {
    hlg_to_nits(rgb)
}

/// PQ single-channel → nits.
#[inline]
pub(crate) fn pq_channel_to_nits(v: f32) -> f32 {
    pq_eotf(v.clamp(0.0, 1.0))
}

/// In-place: absolute-luminance linear RGB planes (cd/m²) → positive
/// PU-XYB planes — the `XybFlavor::Pu` replacement for
/// [`crate::pipeline::planes_to_positive_xyb`].
pub fn planes_to_pu_xyb(p: &mut [Vec<f32>; 3], npix: usize) {
    use super::xyb::{BIAS, M00, M01, M02, M10, M11, M12, M20, M21, M22};
    for i in 0..npix {
        let (r, g, b) = (p[0][i], p[1][i], p[2][i]);
        // Opsin LMS mix on absolute luminance — no [0,1] clamp (HDR > 1).
        let mixed0 = M00
            .mul_add(r, M01.mul_add(g, M02.mul_add(b, BIAS)))
            .max(0.0);
        let mixed1 = M10
            .mul_add(r, M11.mul_add(g, M12.mul_add(b, BIAS)))
            .max(0.0);
        let mixed2 = M20
            .mul_add(r, M21.mul_add(g, M22.mul_add(b, BIAS)))
            .max(0.0);

        let c0 = pu21_encode(mixed0) / PU_WHITE;
        let c1 = pu21_encode(mixed1) / PU_WHITE;
        let c2 = pu21_encode(mixed2) / PU_WHITE;

        let x = 0.5 * (c0 - c1);
        let y = 0.5 * (c0 + c1);
        p[0][i] = x.mul_add(PU_X_SCALE, 0.42);
        p[1][i] = y + 0.01;
        p[2][i] = (c2 - y) + 0.55;
    }
}
