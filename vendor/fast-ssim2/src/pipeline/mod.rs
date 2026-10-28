#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]

//! The SSIMULACRA2.1 pipeline — a bit-exact reimplementation of the
//! reference (`ssimulacra2.cc` + vendored libjxl primitives).
//!
//! Cloudinary's standalone `ssimulacra2` and the `ssimulacra2` tool shipped
//! in libjxl v0.12.0 produce identical scores (verified on 400+ image pairs
//! and by source diff); this module reproduces that single behavior,
//! including its floating-point quirks:
//!
//! - sRGB → linear via captured LUTs (lcms2's sampled-curve evaluation,
//!   which differs from both the exact EOTF and `TF_SRGB`'s rational
//!   polynomial by up to ~2.6e-7)
//! - `CubeRootAndAdd` (not `cbrt`)
//! - `FastGaussian` with the reference's exact 4-lane-unrolled FMA ordering
//! - `SSIMMap`/`EdgeDiffMap` with the f32-quotient/f64-aggregate split
//! - `Score` with the reference's exact f64 op ordering
//!
//! Bit-exactness is guaranteed for 8-bit sRGB inputs (JPEG/PNG decoded to
//! u8) — the captured lcms LUT reproduces the reference's linearization
//! bit-for-bit. `u16` and off-grid `f32` encoded inputs evaluate the sRGB
//! polynomial instead — close to, but not guaranteed bit-identical with,
//! the reference's lcms evaluation (the reference binary only ever sees
//! u8, so no u16 reference behavior exists to match).

/// pngdeer patch: wasm32 has no scalar fused multiply-add, so `f32::mul_add` becomes a call into
/// compiler-builtins' bit-exact software emulation. An f32 product is exact in f64, so this matches
/// a true fma except for rare double-rounding ties (1 ulp), and inlines to a few instructions.
pub(crate) trait Fma {
    fn fma(self, b: f32, c: f32) -> f32;
}

impl Fma for f32 {
    #[inline(always)]
    fn fma(self, b: f32, c: f32) -> f32 {
        #[cfg(target_arch = "wasm32")]
        {
            (self as f64 * b as f64 + c as f64) as f32
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.mul_add(b, c)
        }
    }
}

pub mod gauss;
mod lut8;
pub mod maps;
pub mod precompute;
#[cfg(feature = "hdr-pu")]
pub mod pu21;
pub mod simd;
pub mod strip;
#[cfg(feature = "rayon")]
use archmage::incant;
pub mod score;
pub mod xyb;

use enough::Stop;
use gauss::{RecursiveGaussian, create_recursive_gaussian, multiply_planes};
use score::ScaleAggregates;
pub(crate) use score::score as final_score;

/// Encoded sRGB pixel data fed to the pipeline.
///
/// Produced by the descriptor-driven input funnel for inputs
/// that carry quantized/encoded sRGB values rather than linear data.
pub enum EncodedData {
    /// Interleaved RGB u8 triples (e.g. PNG-8 decode).
    U8(Vec<u8>),
    /// Interleaved RGB u16 triples (e.g. PNG-16 decode).
    U16(Vec<u16>),
    /// Interleaved encoded-sRGB f32 triples in [0, 1] (e.g. `yuvxyb::Rgb`).
    F32(Vec<f32>),
}

/// Encoded sRGB image handed to the pipeline.
pub struct EncodedSrgb {
    pub width: usize,
    pub height: usize,
    pub data: EncodedData,
    /// Normalized alpha plane (`v * (1/max)` like the reference decode), if
    /// the input has an alpha channel. When present the reference binary
    /// alpha-blends onto a background and takes the worst of two scores.
    pub alpha: Option<Vec<f32>>,
}

impl EncodedSrgb {
    /// Extract the row range `[y0, y1)` as a new `EncodedSrgb` (strip
    /// slicing — keeps encoded data encoded so linearization still uses
    /// the LUT path per-strip).
    pub fn strip_rows(&self, y0: usize, y1: usize) -> Self {
        let w = self.width;
        let h = y1 - y0;
        let (a0, a1) = (y0 * w * 3, y1 * w * 3);
        let data = match &self.data {
            EncodedData::U8(d) => EncodedData::U8(d[a0..a1].to_vec()),
            EncodedData::U16(d) => EncodedData::U16(d[a0..a1].to_vec()),
            EncodedData::F32(d) => EncodedData::F32(d[a0..a1].to_vec()),
        };
        let alpha = self.alpha.as_ref().map(|a| a[y0 * w..y1 * w].to_vec());
        EncodedSrgb {
            width: w,
            height: h,
            data,
            alpha,
        }
    }

    /// Reflect(mirror)-pad encoded planes up to `min` px on each axis —
    /// the crate-level sub-8px contract (`compute_ssimulacra2` scores
    /// down to 1×1 where the reference binary refuses). Per-pixel LUT
    /// linearization makes padding encoded planes equivalent to padding
    /// post-linearization, so this is faithful to [`reflect_pad_linear`]
    /// while preserving `U8` exactness. NO-OP at ≥ `min` (or empty).
    pub fn reflect_padded(&self, min: usize) -> Self {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || (w >= min && h >= min) {
            return self.clone_shallow();
        }
        let (pw, ph) = (w.max(min), h.max(min));
        let data = match &self.data {
            EncodedData::U8(d) => EncodedData::U8(pad_px(w, h, pw, ph, d)),
            EncodedData::U16(d) => EncodedData::U16(pad_px(w, h, pw, ph, d)),
            EncodedData::F32(d) => EncodedData::F32(pad_px(w, h, pw, ph, d)),
        };
        let alpha = self.alpha.as_ref().map(|a| pad_scalars(w, h, pw, ph, a));
        EncodedSrgb {
            width: pw,
            height: ph,
            data,
            alpha,
        }
    }

    /// Cheap clone for the no-pad case (data Vec is shared via clone —
    /// encode data only cloned when padding actually fires).
    fn clone_shallow(&self) -> Self {
        EncodedSrgb {
            width: self.width,
            height: self.height,
            data: match &self.data {
                EncodedData::U8(d) => EncodedData::U8(d.clone()),
                EncodedData::U16(d) => EncodedData::U16(d.clone()),
                EncodedData::F32(d) => EncodedData::F32(d.clone()),
            },
            alpha: self.alpha.clone(),
        }
    }
}

/// Periodic edge-mirror, same convention as `reflect_pad_linear` in the
/// crate root: `i` beyond `n` mirrors without repeating the edge sample;
/// `n <= 1` collapses to 0.
fn reflect_index(i: usize, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut k = i % period;
    if k >= n {
        k = period - k;
    }
    k
}

fn pad_px<T: Copy>(w: usize, h: usize, pw: usize, ph: usize, data: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(pw * ph * 3);
    for y in 0..ph {
        let row = reflect_index(y, h) * w;
        for x in 0..pw {
            let i = (row + reflect_index(x, w)) * 3;
            out.extend_from_slice(&data[i..i + 3]);
        }
    }
    out
}

fn pad_scalars(w: usize, h: usize, pw: usize, ph: usize, data: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(pw * ph);
    for y in 0..ph {
        let row = reflect_index(y, h) * w;
        for x in 0..pw {
            out.push(data[row + reflect_index(x, w)]);
        }
    }
    out
}

/// Reference linearization: the sRGB curve as evaluated by the official
/// binary's cms (lcms2 in the solo repo), captured as a 256-entry LUT for
/// u8 inputs. u16 and arbitrary f32 inputs use the crate's `srgb_to_linear`
/// rational polynomial (spec algorithm; not bit-exact vs lcms).
///
/// `bg` is the alpha-blend background used by the reference `AlphaBlend`
/// (`a * v + (1 - a) * bg` in encoded space). The standalone binary calls
/// the metric twice — `bg = 0.1` and `bg = 0.9` — and keeps the worse
/// score. `bg` is ignored when `enc.alpha` is `None`.
pub fn linearize(enc: &EncodedSrgb, bg: f32) -> [Vec<f32>; 3] {
    linearize_stop(enc, bg, &enough::Unstoppable).unwrap_or_else(|_| unreachable!())
}

/// Rows between `stop` polls inside [`linearize_stop`]'s LUT loops —
/// checks stay out of the per-pixel loop so the inner pass still
/// vectorizes.
const LINEARIZE_STOP_ROWS: usize = 64;

/// [`linearize`] with cooperative cancellation — `stop` is checked every
/// [`LINEARIZE_STOP_ROWS`] rows between row chunks.
pub(crate) fn linearize_stop(
    enc: &EncodedSrgb,
    bg: f32,
    stop: &dyn enough::Stop,
) -> Result<[Vec<f32>; 3], enough::StopReason> {
    let stop = stop.may_stop().then_some(stop);
    let (w, h) = (enc.width, enc.height);
    if w == 0 || h == 0 {
        return Ok([Vec::new(), Vec::new(), Vec::new()]);
    }
    let n = w * h;
    let mut out = [
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    ];
    if let Some(alpha) = &enc.alpha {
        let encoded = |i: usize, v: f32| {
            let af = alpha[i];
            encoded_f32_to_linear(af * v + (1.0 - af) * bg)
        };
        match &enc.data {
            EncodedData::U8(data) => {
                // pngdeer patch: a blended pixel's value depends only on its alpha byte, its
                // channel byte and `bg`, so each pair is computed once (NaN: not yet). The
                // rational polynomial behind `encoded` is eight f64 `mul_add`s, which wasm32
                // runs as software fma calls: most of a translucent image's scoring there.
                // An alpha that isn't some byte's `b * (1/255)` (a caller's own) is computed
                // as before.
                let mut blended = vec![f32::NAN; 256 * 256];
                for (r, row) in data.chunks_exact(3 * w).enumerate() {
                    if r & (LINEARIZE_STOP_ROWS - 1) == 0 {
                        stop.check()?;
                    }
                    for (j, px) in row.chunks_exact(3).enumerate() {
                        let i = r * w + j;
                        let af = alpha[i];
                        let b = (af * 255.0).round();
                        let exact = (0.0..=255.0).contains(&b)
                            && (b as u8 as f32 * (1.0 / 255.0)).to_bits() == af.to_bits();
                        for c in 0..3 {
                            out[c].push(if af == 1.0 {
                                lut8::LINEAR_LUT_U8[px[c] as usize]
                            } else if exact {
                                let k = (b as usize) << 8 | px[c] as usize;
                                if blended[k].is_nan() {
                                    blended[k] = encoded(i, px[c] as f32 * (1.0 / 255.0));
                                }
                                blended[k]
                            } else {
                                encoded(i, px[c] as f32 * (1.0 / 255.0))
                            });
                        }
                    }
                }
            }
            EncodedData::U16(data) => {
                for (r, row) in data.chunks_exact(3 * w).enumerate() {
                    if r & (LINEARIZE_STOP_ROWS - 1) == 0 {
                        stop.check()?;
                    }
                    for (j, px) in row.chunks_exact(3).enumerate() {
                        let i = r * w + j;
                        out[0].push(encoded(i, px[0] as f32 * (1.0 / 65535.0)));
                        out[1].push(encoded(i, px[1] as f32 * (1.0 / 65535.0)));
                        out[2].push(encoded(i, px[2] as f32 * (1.0 / 65535.0)));
                    }
                }
            }
            EncodedData::F32(data) => {
                for (r, row) in data.chunks_exact(3 * w).enumerate() {
                    if r & (LINEARIZE_STOP_ROWS - 1) == 0 {
                        stop.check()?;
                    }
                    for (j, px) in row.chunks_exact(3).enumerate() {
                        let i = r * w + j;
                        out[0].push(encoded(i, px[0]));
                        out[1].push(encoded(i, px[1]));
                        out[2].push(encoded(i, px[2]));
                    }
                }
            }
        }
        return Ok(out);
    }
    match &enc.data {
        EncodedData::U8(data) => {
            for (r, row) in data.chunks_exact(3 * w).enumerate() {
                if r & (LINEARIZE_STOP_ROWS - 1) == 0 {
                    stop.check()?;
                }
                for px in row.chunks_exact(3) {
                    out[0].push(lut8::LINEAR_LUT_U8[px[0] as usize]);
                    out[1].push(lut8::LINEAR_LUT_U8[px[1] as usize]);
                    out[2].push(lut8::LINEAR_LUT_U8[px[2] as usize]);
                }
            }
        }
        EncodedData::U16(data) => {
            for (r, row) in data.chunks_exact(3 * w).enumerate() {
                if r & (LINEARIZE_STOP_ROWS - 1) == 0 {
                    stop.check()?;
                }
                for px in row.chunks_exact(3) {
                    out[0].push(crate::input::srgb_to_linear(px[0] as f32 * (1.0 / 65535.0)));
                    out[1].push(crate::input::srgb_to_linear(px[1] as f32 * (1.0 / 65535.0)));
                    out[2].push(crate::input::srgb_to_linear(px[2] as f32 * (1.0 / 65535.0)));
                }
            }
        }
        EncodedData::F32(data) => {
            for (r, row) in data.chunks_exact(3 * w).enumerate() {
                if r & (LINEARIZE_STOP_ROWS - 1) == 0 {
                    stop.check()?;
                }
                for px in row.chunks_exact(3) {
                    out[0].push(encoded_f32_to_linear(px[0]));
                    out[1].push(encoded_f32_to_linear(px[1]));
                    out[2].push(encoded_f32_to_linear(px[2]));
                }
            }
        }
    }
    Ok(out)
}

/// Reference-linearization approximation for arbitrary encoded f32 in
/// [0,1] (alpha-blended or unquantized inputs): the crate's `srgb_to_linear`
/// rational polynomial (libjxl `TF_SRGB`). NOT bit-exact vs the reference's
/// lcms evaluation on off-grid values — deviation ~1e-7 max.
#[inline]
fn encoded_f32_to_linear(x: f32) -> f32 {
    crate::input::srgb_to_linear(x.clamp(0.0, 1.0))
}

/// Reference `Downsample` — linear RGB, box 2×2, ceil output size,
/// clamped edge taps, `sum += ` in iy-outer/ix-inner order, `* 0.25`.
pub fn downsample_planes(
    p: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> ([Vec<f32>; 3], usize, usize) {
    let nw = width.div_ceil(2);
    let nh = height.div_ceil(2);
    let mut out = [
        vec![0f32; nw * nh],
        vec![0f32; nw * nh],
        vec![0f32; nw * nh],
    ];
    // Per-(channel,row-group) writes are disjoint — parallel when rayon
    // is on; the per-pixel math is identical either way.
    let fill = |c: usize, oy: usize, row: &mut [f32]| {
        for (ox, o) in row.iter_mut().enumerate() {
            let x0 = (2 * ox).min(width - 1);
            let x1 = (2 * ox + 1).min(width - 1);
            let y0 = (2 * oy).min(height - 1);
            let y1 = (2 * oy + 1).min(height - 1);
            *o = (p[c][y0 * width + x0]
                + p[c][y0 * width + x1]
                + p[c][y1 * width + x0]
                + p[c][y1 * width + x1])
                * 0.25;
        }
    };
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        out.par_iter_mut().enumerate().for_each(|(c, plane)| {
            plane
                .par_chunks_mut(nw)
                .enumerate()
                .for_each(|(oy, row)| fill(c, oy, row));
        });
    }
    #[cfg(not(feature = "rayon"))]
    {
        for c in 0..3 {
            for (oy, row) in out[c].chunks_mut(nw).enumerate() {
                fill(c, oy, row);
            }
        }
    }
    (out, nw, nh)
}

/// Gaussian-blur a 3-plane image with the reference `FastGaussian`
/// (horizontal into temp, then vertical).
pub fn blur_planes(
    rg: &RecursiveGaussian,
    p: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> [Vec<f32>; 3] {
    // Unstoppable never fires — the only effect is a few strided checks.
    match blur_planes_stop(rg, p, width, height, &enough::Unstoppable) {
        Ok(o) => o,
        Err(_) => unreachable!("Unstoppable never stops"),
    }
}

/// [`blur_planes`] with cooperative cancellation — `stop` is checked
/// per channel and per row-block inside each pass.
pub(crate) fn blur_planes_stop(
    rg: &RecursiveGaussian,
    p: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    stop: &dyn enough::Stop,
) -> Result<[Vec<f32>; 3], enough::StopReason> {
    let stop = stop.may_stop().then_some(stop);
    let mut out = [
        vec![0f32; width * height],
        vec![0f32; width * height],
        vec![0f32; width * height],
    ];
    let mut tmp = vec![0f32; width * height];
    for c in 0..3 {
        stop.check()?;
        // Horizontal pass: each row independently.
        for y in 0..height {
            if y & (MOD_ROW_STOP_STRIDE - 1) == 0 {
                stop.check()?;
            }
            rg.fast_gaussian_1d(
                &p[c][y * width..(y + 1) * width],
                &mut tmp[y * width..(y + 1) * width],
            );
        }
        stop.check()?;
        // Vertical pass: each column independently. Columns are
        // independent, so chunk them to poll mid-pass.
        for x0 in (0..width).step_by(MOD_COL_STOP_STRIDE) {
            let cw = (width - x0).min(MOD_COL_STOP_STRIDE);
            let tmp_ref = &tmp;
            let out_c = &mut out[c];
            rg.fast_gaussian_vertical_1d(
                cw,
                height,
                |row, x| tmp_ref[row * width + x0 + x],
                &mut |row, x, v| out_c[row * width + x0 + x] = v,
            );
            stop.check()?;
        }
    }
    Ok(out)
}

/// `blur_planes` writing into caller-provided scratch (strip-mode reuse).
/// `out`/`tmp` planes sized ≥ width*height; `tmp` shared across channels
/// (scalar path is serial anyway).
pub fn blur_planes_into(
    rg: &RecursiveGaussian,
    p: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    out: &mut [Vec<f32>; 3],
    tmp: &mut [Vec<f32>; 3],
) {
    for c in 0..3 {
        for y in 0..height {
            rg.fast_gaussian_1d(
                &p[c][y * width..(y + 1) * width],
                &mut tmp[c][y * width..(y + 1) * width],
            );
        }
        let tmp_ref = &tmp[c];
        rg.fast_gaussian_vertical_1d(
            width,
            height,
            |row, x| tmp_ref[row * width + x],
            &mut |row, x, v| out[c][row * width + x] = v,
        );
    }
}

/// `CubeRootLo`: `cbrt_lowp_f32` — 1 Halley, ~259 ulp max. Experiment knob.
pub fn planes_to_positive_xyb_lo(p: &mut [Vec<f32>; 3], npix: usize) {
    for i in 0..npix {
        let px = xyb::linear_rgb_to_xyb_pixel_lo([p[0][i], p[1][i], p[2][i]]);
        p[0][i] = px[0];
        p[1][i] = px[1];
        p[2][i] = px[2];
    }
}

/// `CubeRootHi`: same XYB formation with `magetypes`' mid-precision
/// Halley cube root (max ~3 ulp vs `f32::cbrt` — vs the reference
/// recipe's ~6 ulp). Scalar oracle for the SIMD variant.
pub fn planes_to_positive_xyb_hi(p: &mut [Vec<f32>; 3], npix: usize) {
    for i in 0..npix {
        let px = xyb::linear_rgb_to_xyb_pixel_hi([p[0][i], p[1][i], p[2][i]]);
        p[0][i] = px[0];
        p[1][i] = px[1];
        p[2][i] = px[2];
    }
}

/// Convert linear-RGB planes to positive-XYB planes in place
/// (`LinearRGBToXYB` + `MakePositiveXYB` per pixel).
pub fn planes_to_positive_xyb(p: &mut [Vec<f32>; 3], npix: usize) {
    // pngdeer patch: a pixel equal to the one before it (flat areas, palette images) reuses its
    // result instead of recomputing three cube roots. Bit-identical.
    let mut last = [u32::MAX; 3]; // a NaN pattern the linear input never holds
    let mut out = [0f32; 3];
    for i in 0..npix {
        let rgb = [p[0][i], p[1][i], p[2][i]];
        let bits = rgb.map(f32::to_bits);
        if bits != last {
            last = bits;
            out = xyb::linear_rgb_to_xyb_pixel(rgb);
        }
        p[0][i] = out[0];
        p[1][i] = out[1];
        p[2][i] = out[2];
    }
}

/// In-place linear planes → positive XYB, per `opts.flavor` (PU21 uses
/// its own encoding; `kernel` still selects scalar/SIMD for cbrt).
fn xyb_convert(p: &mut [Vec<f32>; 3], npix: usize, opts: Opts) {
    match opts.flavor {
        XybFlavor::CubeRoot => match opts.kernel {
            Kernel::Simd => simd::planes_to_positive_xyb_simd(p),
            Kernel::Scalar => planes_to_positive_xyb(p, npix),
        },
        XybFlavor::CubeRootHi => match opts.kernel {
            Kernel::Simd => simd::planes_to_positive_xyb_hi_simd(p),
            Kernel::Scalar => planes_to_positive_xyb_hi(p, npix),
        },
        XybFlavor::CubeRootLo => match opts.kernel {
            Kernel::Simd => simd::planes_to_positive_xyb_lo_simd(p),
            Kernel::Scalar => planes_to_positive_xyb_lo(p, npix),
        },
        #[cfg(feature = "hdr-pu")]
        XybFlavor::Pu21 => pu21::planes_to_pu_xyb(p, npix),
    }
}

/// The full pipeline on encoded sRGB inputs — `kernel` selects the
/// scalar or SIMD kernel family (bit-identical outputs).
pub fn compute_encoded(
    enc1: &EncodedSrgb,
    enc2: &EncodedSrgb,
    kernel: Kernel,
) -> Result<f64, crate::Ssimulacra2Error> {
    compute_encoded_stop(enc1, enc2, kernel, &enough::Unstoppable)
}

/// [`compute_encoded`] with cooperative cancellation — `stop` is
/// checked once per scale (never per-pixel).
pub fn compute_encoded_stop(
    enc1: &EncodedSrgb,
    enc2: &EncodedSrgb,
    kernel: Kernel,
    stop: &dyn enough::Stop,
) -> Result<f64, crate::Ssimulacra2Error> {
    let stop = stop.may_stop().then_some(stop);
    let (w, h) = (enc1.width, enc1.height);
    if w != enc2.width || h != enc2.height {
        return Err(crate::Ssimulacra2Error::NonMatchingImageDimensions);
    }
    let lin = |e: &EncodedSrgb, bg| -> Result<[Vec<f32>; 3], crate::Ssimulacra2Error> {
        let planes = linearize_stop(e, bg, &stop).map_err(crate::Ssimulacra2Error::Cancelled)?;
        stop.check().map_err(crate::Ssimulacra2Error::Cancelled)?;
        Ok(planes)
    };
    let opts = Opts {
        kernel,
        flavor: XybFlavor::CubeRoot,
    };
    if enc1.alpha.is_some() || enc2.alpha.is_some() {
        let lo = compute_planar_stop(lin(enc1, 0.1)?, lin(enc2, 0.1)?, w, h, opts, &stop)?;
        let hi = compute_planar_stop(lin(enc1, 0.9)?, lin(enc2, 0.9)?, w, h, opts, &stop)?;
        return Ok(lo.min(hi));
    }
    compute_planar_stop(lin(enc1, 0.5)?, lin(enc2, 0.5)?, w, h, opts, &stop)
}

/// Kernel family — the lane-wise SIMD implementations
/// ([`simd`]) are bit-identical to the scalar port ([`gauss`],
/// [`maps`]); `Scalar` remains as the audit oracle and the fallback for
/// architectures without a lane implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    Scalar,
    Simd,
}

impl Kernel {
    /// The SIMD kernels are bit-identical to scalar — this is purely a
    /// speed selector.
    pub fn from_impl(impl_type: crate::SimdImpl) -> Self {
        match impl_type {
            crate::SimdImpl::Scalar => Kernel::Scalar,
            crate::SimdImpl::Simd => Kernel::Simd,
        }
    }
    /// The default public-path kernel.
    pub const SIMD: Self = Self::Simd;
}

/// Perceptual-encoding flavor for the XYB stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Without hdr-pu, all experiment variants intentionally name the cube-root family.
#[allow(clippy::enum_variant_names)]
pub enum XybFlavor {
    /// Cube-root opsin — the SDR/reference encoding (hwy `CubeRootAndAdd`
    /// port: exponent seed + 3 Newton, ~6 ulp — bit-exact vs binary).
    CubeRoot,
    /// Same opsin structure with `magetypes` `cbrt_midp` (Kahan seed +
    /// 2 Halley iterations — max ~3 ulp vs `f32::cbrt`). Divergent from
    /// the reference by ≤ ~3 ulp per pixel — a defensible "idealized"
    /// variant for studying quantization-lattice artifacts. NOT
    /// bit-exact vs the reference binary.
    CubeRootHi,
    /// `magetypes` `cbrt_lowp` — 1 Halley iteration, ~259 max ulp
    /// (still well inside the opsin's working tolerance; an aggressive
    /// experiment knob, NOT a defensible scoring variant).
    CubeRootLo,
    /// PU21 `banding_glare` on absolute luminance (cd/m²) — HDR input
    /// (`hdr-pu` feature); scores are not comparable to SDR scores.
    #[cfg(feature = "hdr-pu")]
    Pu21,
}

/// Per-call pipeline options — only the kernel family survives; the
/// reference's FP behavior (LUT linearization, `CubeRootAndAdd`, f32 σ,
/// reference FMA ordering) is fixed.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub kernel: Kernel,
    /// Perceptual encoding for the XYB stage (SDR default).
    pub flavor: XybFlavor,
}

impl Opts {
    /// Scalar oracle — used by tests/ports comparing SIMD against the
    /// reference-order scalar computation.
    pub const SCALAR: Self = Self {
        kernel: Kernel::Scalar,
        flavor: XybFlavor::CubeRoot,
    };
    /// Default: the SIMD kernels.
    pub const SIMD: Self = Self {
        kernel: Kernel::Simd,
        flavor: XybFlavor::CubeRoot,
    };
}

/// The complete metric on already-linear planes (from [`linearize`] or
/// equivalent). Uses the SIMD kernels (bit-identical to the scalar
/// port, which remains the audit oracle).
///
/// Returns `Err(Ssimulacra2Error::InvalidImageSize)` below 8×8, matching
/// the reference binary's minimum-size behavior.
pub fn compute_planar(
    lin1: [Vec<f32>; 3],
    lin2: [Vec<f32>; 3],
    width: usize,
    height: usize,
) -> Result<f64, crate::Ssimulacra2Error> {
    compute_planar_stop(lin1, lin2, width, height, Opts::SIMD, &enough::Unstoppable)
}

/// [`compute_planar`] with explicit kernel selection.
pub fn compute_planar_with(
    lin1: [Vec<f32>; 3],
    lin2: [Vec<f32>; 3],
    width: usize,
    height: usize,
    kernel: Kernel,
) -> Result<f64, crate::Ssimulacra2Error> {
    compute_planar_stop(
        lin1,
        lin2,
        width,
        height,
        Opts {
            kernel,
            flavor: XybFlavor::CubeRoot,
        },
        &enough::Unstoppable,
    )
}

/// Rows between `stop` polls inside `blur_planes_stop`'s horizontal pass.
const MOD_ROW_STOP_STRIDE: usize = 1 << 10;
/// Columns per chunk between `stop` polls inside `blur_planes_stop`'s
/// vertical pass (columns are independent; each chunk is width-sliced).
const MOD_COL_STOP_STRIDE: usize = 256;

/// [`compute_planar`] with cooperative cancellation — `stop` is checked
/// between the per-scale plane ops and inside the blur passes.
pub fn compute_planar_stop(
    lin1: [Vec<f32>; 3],
    lin2: [Vec<f32>; 3],
    width: usize,
    height: usize,
    opts: Opts,
    stop: &dyn enough::Stop,
) -> Result<f64, crate::Ssimulacra2Error> {
    let stop = stop.may_stop().then_some(stop);
    if width < 8 || height < 8 {
        return Err(crate::Ssimulacra2Error::InvalidImageSize);
    }

    let rg = create_recursive_gaussian(1.5);
    const NUM_SCALES: usize = 6;

    let mut lin1 = lin1;
    let mut lin2 = lin2;
    let mut w = width;
    let mut h = height;
    let mut scales = Vec::with_capacity(NUM_SCALES);

    // NOTE: the reference gates `size < 8` on the *pre-downsample* dims —
    // a scale produced by downsampling into <8 territory still runs its
    // maps (e.g. 10x8 -> 5x4 runs scale maps at 5x4). `gw`,`gh` track the
    // gate dims (previous iteration's); `w`,`h` are the current scale's.
    let mut gw = width;
    let mut gh = height;
    for _scale in 0..NUM_SCALES {
        stop.check().map_err(crate::Ssimulacra2Error::Cancelled)?;
        if gw < 8 || gh < 8 {
            break;
        }
        // Produce next scale's linear planes first, then convert the
        // current linear planes to XYB *in place* — saves 6 plane
        // allocations (lin1/lin2 clones) per scale.
        let (lin1_next, nw, nh) = downsample_planes(&lin1, w, h);
        stop.check().map_err(crate::Ssimulacra2Error::Cancelled)?;
        let (lin2_next, _, _) = downsample_planes(&lin2, w, h);
        stop.check().map_err(crate::Ssimulacra2Error::Cancelled)?;

        let npix = w * h;
        let mut xyb1 = lin1;
        let mut xyb2 = lin2;
        xyb_convert(&mut xyb1, npix, opts);
        stop.check().map_err(crate::Ssimulacra2Error::Cancelled)?;
        xyb_convert(&mut xyb2, npix, opts);
        stop.check().map_err(crate::Ssimulacra2Error::Cancelled)?;

        let blur_sel = |p: &[Vec<f32>; 3]| -> Result<[Vec<f32>; 3], enough::StopReason> {
            blur_planes_stop(&rg, p, w, h, &stop)
        };

        // mul = xyb_i * xyb_i → blurred squares; mul = xyb1 * xyb2 → cross.
        // The five blurs (σ1,σ2,σ12,μ1,μ2) are mutually independent.
        // SIMD path: products fuse into the blur's input read (the f32
        // elementwise product is identical → bit-exact), so no `mul`
        // planes are materialized at all. Scalar path keeps the
        // reference's literal mul-then-blur sequence.
        let (sigma1_sq, sigma2_sq, sigma12, mu1, mu2);
        if opts.kernel == Kernel::Simd {
            // jobs: (a, b) — b present → blur of the product a·b.
            let jobs: [(&[Vec<f32>; 3], Option<&[Vec<f32>; 3]>); 5] = [
                (&xyb1, Some(&xyb1)),
                (&xyb2, Some(&xyb2)),
                (&xyb1, Some(&xyb2)),
                (&xyb1, None),
                (&xyb2, None),
            ];
            #[cfg(feature = "rayon")]
            {
                use rayon::prelude::*;
                let chan: Vec<(usize, usize)> =
                    (0..5).flat_map(|j| (0..3).map(move |c| (j, c))).collect();
                let flat: Vec<Vec<f32>> = chan
                    .into_par_iter()
                    .map(|(j, c)| {
                        let mut out = vec![0f32; npix];
                        let mut tmp = vec![0f32; npix];
                        let (a, b) = jobs[j];
                        simd::fast_gaussian_simd_stop(
                            &rg,
                            &a[c],
                            b.map(|bb| bb[c].as_slice()),
                            w,
                            h,
                            &mut out,
                            &mut tmp,
                            &stop,
                        )?;
                        Ok(out)
                    })
                    .collect::<Result<_, enough::StopReason>>()
                    .map_err(crate::Ssimulacra2Error::Cancelled)?;
                let take3 = |it: &mut std::vec::IntoIter<Vec<f32>>| {
                    [it.next().unwrap(), it.next().unwrap(), it.next().unwrap()]
                };
                let mut it = flat.into_iter();
                sigma1_sq = take3(&mut it);
                sigma2_sq = take3(&mut it);
                sigma12 = take3(&mut it);
                mu1 = take3(&mut it);
                mu2 = take3(&mut it);
            }
            #[cfg(not(feature = "rayon"))]
            {
                let run = |(a, b): (&[Vec<f32>; 3], Option<&[Vec<f32>; 3]>)| -> Result<
                    [Vec<f32>; 3],
                    crate::Ssimulacra2Error,
                > {
                    let mut out = [vec![0f32; npix], vec![0f32; npix], vec![0f32; npix]];
                    let mut tmp = vec![0f32; npix];
                    for c in 0..3 {
                        simd::fast_gaussian_simd_stop(
                            &rg,
                            &a[c],
                            b.map(|bb| bb[c].as_slice()),
                            w,
                            h,
                            &mut out[c],
                            &mut tmp,
                            &stop,
                        )
                        .map_err(crate::Ssimulacra2Error::Cancelled)?;
                    }
                    Ok(out)
                };
                sigma1_sq = run(jobs[0])?;
                sigma2_sq = run(jobs[1])?;
                sigma12 = run(jobs[2])?;
                mu1 = run(jobs[3])?;
                mu2 = run(jobs[4])?;
            }
        } else {
            let mut mul = [vec![0f32; npix], vec![0f32; npix], vec![0f32; npix]];
            multiply_planes(&xyb1, &xyb1, &mut mul);
            sigma1_sq = blur_sel(&mul).map_err(crate::Ssimulacra2Error::Cancelled)?;
            multiply_planes(&xyb2, &xyb2, &mut mul);
            sigma2_sq = blur_sel(&mul).map_err(crate::Ssimulacra2Error::Cancelled)?;
            multiply_planes(&xyb1, &xyb2, &mut mul);
            sigma12 = blur_sel(&mul).map_err(crate::Ssimulacra2Error::Cancelled)?;
            mu1 = blur_sel(&xyb1).map_err(crate::Ssimulacra2Error::Cancelled)?;
            mu2 = blur_sel(&xyb2).map_err(crate::Ssimulacra2Error::Cancelled)?;
        }

        // ssim (lanes) + edge_diff (scalar f64) run per channel — the
        // per-channel accumulation order is unchanged → bit-exact. The
        // six channel-tasks are independent → parallel under rayon.
        let (avg_ssim, avg_edgediff) = if opts.kernel == Kernel::Simd {
            #[cfg(feature = "rayon")]
            {
                let (mut so, mut eo) = ([0f64; 6], [0f64; 12]);
                use rayon::prelude::*;
                let n = w * h;
                let opp = 1.0 / n as f64;
                let parts: Vec<(f64, f64, [f64; 4])> = (0..3)
                    .into_par_iter()
                    .map(|c| {
                        let (mut s0, mut s1) = (0f64, 0f64);
                        incant!(
                            simd::ssim_map_inner(
                                &mu1[c],
                                &mu2[c],
                                &sigma1_sq[c],
                                &sigma2_sq[c],
                                &sigma12[c],
                                &mut s0,
                                &mut s1
                            ),
                            [v3, neon, wasm128, scalar]
                        );
                        stop.check()?;
                        let mut e = [0f64; 4];
                        simd::edge_sums_fast(0, h, w, &xyb1[c], &mu1[c], &xyb2[c], &mu2[c], &mut e);
                        Ok((
                            opp * s0,
                            (opp * s1).sqrt().sqrt(),
                            [
                                opp * e[0],
                                (opp * e[1]).sqrt().sqrt(),
                                opp * e[2],
                                (opp * e[3]).sqrt().sqrt(),
                            ],
                        ))
                    })
                    .collect::<Result<Vec<_>, enough::StopReason>>()
                    .map_err(crate::Ssimulacra2Error::Cancelled)?;
                for c in 0..3 {
                    so[c * 2] = parts[c].0;
                    so[c * 2 + 1] = parts[c].1;
                    eo[c * 4..c * 4 + 4].copy_from_slice(&parts[c].2);
                }
                (so, eo)
            }
            #[cfg(not(feature = "rayon"))]
            {
                simd::maps_fused_simd_stop(
                    &mu1, &mu2, &sigma1_sq, &sigma2_sq, &sigma12, &xyb1, &xyb2, w, h, &stop,
                )
                .map_err(crate::Ssimulacra2Error::Cancelled)?
            }
        } else {
            maps::maps_fused_stop(
                &mu1, &mu2, &sigma1_sq, &sigma2_sq, &sigma12, &xyb1, &xyb2, w, h, &stop,
            )
            .map_err(crate::Ssimulacra2Error::Cancelled)?
        };
        scales.push(ScaleAggregates {
            avg_ssim,
            avg_edgediff,
        });

        lin1 = lin1_next;
        lin2 = lin2_next;
        gw = w;
        gh = h;
        w = nw;
        h = nh;
    }

    Ok(final_score(&scales))
}
