#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]
//! Strip-bounded pipeline.
//!
//! The walker: 32-aligned strip boundaries, 96-row halo, interior-only
//! accumulation, per-scale bound halving — feeding `score::score`
//! aggregation.
//!
//! Fidelity note: strip processing is inherently approximate vs the
//! full-image reference — the IIR blur warmup at strip edges differs
//! (~1e-3-relative tail error from the 96-row halo) and f64 accumulation
//! order differs per-strip. Expect last-decimal deviations, same class as
//! the precise strip path.

use super::gauss::{RecursiveGaussian, create_recursive_gaussian};
use super::maps::{K_C2, tothe4th};
use super::precompute;
use super::score::{ScaleAggregates, score as final_score};
use super::simd;
use super::{EncodedSrgb, Kernel, Opts, planes_to_positive_xyb};
use crate::Ssimulacra2Error;

const NUM_SCALES: usize = 6;
/// Scale-0 row alignment that keeps strip boundaries consistent through
/// all six scales (same as `strip.rs`).
const STRIP_ALIGN: usize = 32;

/// Per-scale running sums (interior pixels only).
#[derive(Default, Clone)]
pub(crate) struct ScaleSums {
    ssim_sums: [f64; 6],
    edge_sums: [f64; 12],
    pixels: u64,
    initialised: bool,
}

pub(crate) struct StripAcc {
    per_scale: Vec<ScaleSums>,
    target_pixels: Vec<u64>,
}

impl StripAcc {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let mut per_scale = Vec::with_capacity(NUM_SCALES);
        let mut target_pixels = Vec::with_capacity(NUM_SCALES);
        let (mut w, mut h) = (width, height);
        for scale in 0..NUM_SCALES {
            if w < 8 || h < 8 {
                break;
            }
            if scale > 0 {
                w = w.div_ceil(2);
                h = h.div_ceil(2);
            }
            per_scale.push(ScaleSums::default());
            target_pixels.push((w * h) as u64);
        }
        Self {
            per_scale,
            target_pixels,
        }
    }

    pub(crate) fn finalise(self, _opts: Opts) -> f64 {
        let mut scales = Vec::with_capacity(self.per_scale.len());
        for (scale, s) in self.per_scale.iter().enumerate() {
            if !s.initialised {
                break;
            }
            let denom = self.target_pixels[scale] as f64;
            let inv = 1.0 / denom;
            let mut avg_ssim = [0.0f64; 6];
            for c in 0..3 {
                avg_ssim[c * 2] = inv * s.ssim_sums[c * 2];
                avg_ssim[c * 2 + 1] = (inv * s.ssim_sums[c * 2 + 1]).sqrt().sqrt();
            }
            let mut avg_edgediff = [0.0f64; 12];
            for c in 0..3 {
                for k in 0..4 {
                    let v = inv * s.edge_sums[c * 4 + k];
                    avg_edgediff[c * 4 + k] = if k % 2 == 0 { v } else { v.sqrt().sqrt() };
                }
            }
            scales.push(ScaleAggregates {
                avg_ssim,
                avg_edgediff,
            });
        }
        final_score(&scales)
    }
}

/// Per-worker scratch buffers, reused across strips AND scales so the
/// plane allocations mmap/fault once per worker instead of ~30 times per
/// strip. Under `parallel_strips` each worker thread owns one Scratch
/// (rayon `map_init`) — this is what keeps strip mode memory-bounded.
#[derive(Default)]
pub(crate) struct Scratch {
    mul: [Vec<f32>; 3],
    // Single-channel sigma/mu planes — the SSIM/edge maps consume one
    // channel at a time, so only one channel's blurred planes need to
    // be live simultaneously (halves the per-strip working set).
    s1: Vec<f32>,
    s2: Vec<f32>,
    s12: Vec<f32>,
    mu1: Vec<f32>,
    mu2: Vec<f32>,
    btmp: Vec<f32>,
    lin1_next: [Vec<f32>; 3],
    lin2_next: [Vec<f32>; 3],
}

fn size_planes(p: &mut [Vec<f32>; 3], npix: usize) {
    for v in p.iter_mut() {
        v.clear();
        v.resize(npix, 0.0);
    }
}

/// `downsample_planes` writing into a caller-provided buffer set.
fn downsample_into(
    p: &[Vec<f32>; 3],
    w: usize,
    h: usize,
    out: &mut [Vec<f32>; 3],
) -> (usize, usize) {
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    size_planes(out, nw * nh);
    for c in 0..3 {
        for oy in 0..nh {
            let iy0 = (oy * 2).min(h - 1);
            let iy1 = (oy * 2 + 1).min(h - 1);
            for ox in 0..nw {
                let ix0 = (ox * 2).min(w - 1);
                let ix1 = (ox * 2 + 1).min(w - 1);
                out[c][oy * nw + ox] = (p[c][iy0 * w + ix0]
                    + p[c][iy0 * w + ix1]
                    + p[c][iy1 * w + ix0]
                    + p[c][iy1 * w + ix1])
                    * 0.25;
            }
        }
    }
    (nw, nh)
}

/// Single-channel (sum_d, sum_d4) over `ys..ye` rows — official
/// per-pixel semantics, used by the channel-streamed strip pipeline.
fn ssim_sums_ch(
    ys: usize,
    ye: usize,
    w: usize,
    mu1: &[f32],
    mu2: &[f32],
    s11: &[f32],
    s22: &[f32],
    s12: &[f32],
) -> (f64, f64) {
    let (mut sum0, mut sum1) = (0.0f64, 0.0f64);
    for y in ys..ye {
        let row = y * w;
        for x in 0..w {
            let i = row + x;
            let m1 = mu1[i];
            let m2 = mu2[i];
            let mu11 = m1 * m1;
            let mu22 = m2 * m2;
            let mu12 = m1 * m2;
            let dm = m1 - m2;
            let num_m = (-dm).mul_add(dm, 1.0f32);
            let num_s = 2.0f32 * (s12[i] - mu12) + K_C2;
            let denom_s = (s11[i] - mu11) + (s22[i] - mu22) + K_C2;
            let q = num_m * num_s / denom_s;
            let d = (1.0f64 - q as f64).max(0.0);
            sum0 += d;
            sum1 += tothe4th(d);
        }
    }
    (sum0, sum1)
}

fn edge_sums_ch(
    ys: usize,
    ye: usize,
    w: usize,
    img1: &[f32],
    mu1: &[f32],
    img2: &[f32],
    mu2: &[f32],
    simd_path: bool,
) -> [f64; 4] {
    let mut sums = [0.0f64; 4];
    if simd_path {
        simd::edge_sums_fast(ys, ye, w, img1, mu1, img2, mu2, &mut sums);
        return sums;
    }
    for y in ys..ye {
        let row = y * w;
        for x in 0..w {
            let i = row + x;
            let num = 1.0 + (img2[i] - mu2[i]).abs() as f64;
            let den = 1.0 + (img1[i] - mu1[i]).abs() as f64;
            let d1 = num / den - 1.0;
            let artifact = d1.max(0.0);
            sums[0] += artifact;
            sums[1] += tothe4th(artifact);
            let detail_lost = (-d1).max(0.0);
            sums[2] += detail_lost;
            sums[3] += tothe4th(detail_lost);
        }
    }
    sums
}

/// Single-channel blur into caller scratch (`dst` out, `btmp` temp).
#[allow(clippy::too_many_arguments)]
fn blur_ch(
    rg: &RecursiveGaussian,
    opts: Opts,
    w: usize,
    h: usize,
    npix: usize,
    p: &[f32],
    b: Option<&[f32]>,
    mul: &mut Vec<f32>,
    dst: &mut Vec<f32>,
    btmp: &mut Vec<f32>,
) {
    dst.clear();
    dst.resize(npix, 0.0);
    match opts.kernel {
        Kernel::Scalar => {
            // Scalar path mirrors the reference literally: materialize
            // the product plane (or the input itself) then blur it.
            let src: &[f32] = match b {
                Some(bb) => {
                    mul.clear();
                    mul.extend(p.iter().zip(bb).map(|(x, y)| x * y));
                    mul
                }
                None => p,
            };
            for y in 0..h {
                rg.fast_gaussian_1d(&src[y * w..(y + 1) * w], &mut btmp[y * w..(y + 1) * w]);
            }
            let t = &*btmp;
            rg.fast_gaussian_vertical_1d(w, h, |row, x| t[row * w + x], &mut |row, x, v| {
                dst[row * w + x] = v
            });
        }
        Kernel::Simd => {
            simd::fast_gaussian_simd(rg, p, b, w, h, dst, btmp);
        }
    }
}

/// Per-strip pipeline: full multi-scale walk on the strip's planes,
/// accumulating interior rows only.
#[allow(clippy::too_many_arguments)]
fn process_strip(
    lin1: [Vec<f32>; 3],
    lin2: [Vec<f32>; 3],
    width: usize,
    height: usize,
    interior_start: usize,
    interior_end: usize,
    scales_hint: usize,
    opts: Opts,
    scratch: &mut Scratch,
) -> Vec<ScaleSums> {
    let rg = create_recursive_gaussian(1.5);
    let mut strip_sums = vec![ScaleSums::default(); scales_hint];
    let mut lin1 = lin1;
    let mut lin2 = lin2;
    let mut w = width;
    let mut h = height;
    // Gate dims lag by one iteration: the reference checks the
    // pre-downsample size, so a scale landing below 8 still runs.
    let mut gw = width;
    let mut gh = height;
    let mut int_s = interior_start;
    let mut int_e = interior_end;
    let total_scales = scales_hint;

    for scale in 0..total_scales {
        if gw < 8 || gh < 8 {
            break;
        }
        // Downsample next scale's planes before converting lin→xyb in
        // place (matches `compute_opts`' allocation layout).
        let (nw, nh) = downsample_into(&lin1, w, h, &mut scratch.lin1_next);
        let _ = downsample_into(&lin2, w, h, &mut scratch.lin2_next);

        let npix = w * h;
        let mut xyb1 = lin1;
        let mut xyb2 = lin2;
        match opts.kernel {
            Kernel::Simd => {
                simd::planes_to_positive_xyb_simd(&mut xyb1);
                simd::planes_to_positive_xyb_simd(&mut xyb2);
            }
            Kernel::Scalar => {
                planes_to_positive_xyb(&mut xyb1, npix);
                planes_to_positive_xyb(&mut xyb2, npix);
            }
        }

        // Per-channel blur into single-channel scratch planes — each
        // channel's maps are consumed before moving on, keeping the
        // sigma/mu working set at ~1 channel instead of 3.
        let sc = &mut *scratch;
        sc.btmp.clear();
        sc.btmp.resize(npix, 0.0);
        size_planes(&mut sc.mul, npix);

        // Only compute maps when interior rows exist at this scale.
        let is = int_s.min(h);
        let ie = int_e.min(h);
        if is < ie {
            // Channel-streamed: hold one channel's sigma/mu planes at a
            // time; the per-pixel math is channel-independent so the
            // ordering is bit-exact vs the 3-channel layout.
            let s = &mut strip_sums[scale];
            for c in 0..3 {
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb1[c],
                    Some(&xyb1[c]),
                    &mut sc.mul[c],
                    &mut sc.s1,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb2[c],
                    Some(&xyb2[c]),
                    &mut sc.mul[c],
                    &mut sc.s2,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb1[c],
                    Some(&xyb2[c]),
                    &mut sc.mul[c],
                    &mut sc.s12,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb1[c],
                    None,
                    &mut sc.mul[c],
                    &mut sc.mu1,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb2[c],
                    None,
                    &mut sc.mul[c],
                    &mut sc.mu2,
                    &mut sc.btmp,
                );

                let (sd, sd4) = ssim_sums_ch(is, ie, w, &sc.mu1, &sc.mu2, &sc.s1, &sc.s2, &sc.s12);
                let es = edge_sums_ch(
                    is,
                    ie,
                    w,
                    &xyb1[c],
                    &sc.mu1,
                    &xyb2[c],
                    &sc.mu2,
                    opts.kernel == Kernel::Simd,
                );
                s.ssim_sums[c * 2] += sd;
                s.ssim_sums[c * 2 + 1] += sd4;
                for k in 0..4 {
                    s.edge_sums[c * 4 + k] += es[k];
                }
            }
            s.pixels += (ie - is) as u64 * w as u64;
            s.initialised = true;
        }

        lin1 = std::mem::take(&mut sc.lin1_next);
        lin2 = std::mem::take(&mut sc.lin2_next);
        gw = w;
        gh = h;
        w = nw;
        h = nh;
        int_s = int_s.div_ceil(2);
        int_e = int_e.div_ceil(2);
    }
    strip_sums
}

/// Strip processor against a cached [`precompute::OfficialReference`]:
/// the ref side is a row-window of the stored `xyb1` planes (re-blurred
/// on the strip so both sides share the strip's IIR boundary handling),
/// the dist side walks its own lin→xyb→blur chain per strip.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_strip_cached(
    lin2: [Vec<f32>; 3],
    refstack: &[precompute::RefScale],
    strip_y0_in_ref: usize,
    width: usize,
    height: usize,
    interior_start: usize,
    interior_end: usize,
    scales_hint: usize,
    opts: Opts,
    scratch: &mut Scratch,
) -> Vec<ScaleSums> {
    let rg = create_recursive_gaussian(1.5);
    let mut strip_sums = vec![ScaleSums::default(); scales_hint];
    let mut lin2 = lin2;
    let mut w = width;
    let mut h = height;
    let mut gw = width;
    let mut gh = height;
    let mut int_s = interior_start;
    let mut int_e = interior_end;
    let mut y0_ref = strip_y0_in_ref;

    for scale in 0..scales_hint {
        if gw < 8 || gh < 8 {
            break;
        }
        let (_nw, _nh) = downsample_into(&lin2, w, h, &mut scratch.lin2_next);
        let npix = w * h;
        let mut xyb2 = lin2;
        match opts.kernel {
            Kernel::Simd => simd::planes_to_positive_xyb_simd(&mut xyb2),
            Kernel::Scalar => planes_to_positive_xyb(&mut xyb2, npix),
        }

        let rs = match refstack.get(scale) {
            Some(r) => r,
            None => break,
        };
        let is = int_s.min(h);
        let ie = int_e.min(h);
        if is < ie {
            let sc = &mut *scratch;
            sc.btmp.clear();
            sc.btmp.resize(npix, 0.0);
            size_planes(&mut sc.mul, npix);
            let s = &mut strip_sums[scale];
            for c in 0..3 {
                // Ref-side xyb1 row-window sliced directly from the
                // stored full-scale plane — no copy; the blur reads the
                // contiguous row range.
                let row0 = y0_ref * rs.width;
                let row1 = (y0_ref + h).min(rs.height) * rs.width;
                let xyb1 = &rs.xyb1[c][row0..row1.min(rs.xyb1[c].len())];

                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb2[c],
                    Some(&xyb2[c]),
                    &mut sc.mul[c],
                    &mut sc.s2,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    xyb1,
                    Some(&xyb2[c]),
                    &mut sc.mul[c],
                    &mut sc.s12,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    xyb1,
                    Some(xyb1),
                    &mut sc.mul[c],
                    &mut sc.s1,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    xyb1,
                    None,
                    &mut sc.mul[c],
                    &mut sc.mu1,
                    &mut sc.btmp,
                );
                blur_ch(
                    &rg,
                    opts,
                    w,
                    h,
                    npix,
                    &xyb2[c],
                    None,
                    &mut sc.mul[c],
                    &mut sc.mu2,
                    &mut sc.btmp,
                );

                let (sd, sd4) = ssim_sums_ch(is, ie, w, &sc.mu1, &sc.mu2, &sc.s1, &sc.s2, &sc.s12);
                let es = edge_sums_ch(
                    is,
                    ie,
                    w,
                    xyb1,
                    &sc.mu1,
                    &xyb2[c],
                    &sc.mu2,
                    opts.kernel == Kernel::Simd,
                );
                s.ssim_sums[c * 2] += sd;
                s.ssim_sums[c * 2 + 1] += sd4;
                for k in 0..4 {
                    s.edge_sums[c * 4 + k] += es[k];
                }
            }
            s.pixels += (ie - is) as u64 * w as u64;
            s.initialised = true;
        }

        lin2 = std::mem::take(&mut scratch.lin2_next);
        gw = w;
        gh = h;
        int_s = int_s.div_ceil(2);
        int_e = int_e.div_ceil(2);
        y0_ref /= 2;
        w = rs.width.div_ceil(2);
        h = h.div_ceil(2);
    }
    strip_sums
}

/// `strip_lin(y0, y1)` produces the dist-side linear strip planes.
/// `refstack` is the stored per-scale reference stack; per-strip ref
/// planes are sliced from it (re-blurred on the strip).
#[allow(clippy::too_many_arguments)]
pub(crate) fn accumulate_strips_cached(
    w: usize,
    h: usize,
    strip_height: usize,
    halo: usize,
    refstack: &[precompute::RefScale],
    strip_lin: impl Fn(usize, usize) -> [Vec<f32>; 3] + Sync,
    opts: Opts,
    #[allow(unused_variables)] parallel: bool,
    stop: &dyn enough::Stop,
    acc: &mut StripAcc,
) -> Result<(), Ssimulacra2Error> {
    let strip_h = strip_height.max(8).min(h);
    let n_scales = acc.per_scale.len();
    let mut strips = Vec::new();
    let mut y = 0usize;
    while y < h {
        let mut next_y = (y + strip_h).next_multiple_of(STRIP_ALIGN);
        if next_y >= h || h - next_y < STRIP_ALIGN {
            next_y = h;
        }
        let halo_above = halo.min(y);
        let halo_below = halo.min(h - next_y);
        strips.push((y, next_y, y - halo_above, next_y + halo_below));
        y = next_y;
    }

    let run_strip = |scratch: &mut Scratch,
                     &(int_s, int_e, sy0, sy1): &(usize, usize, usize, usize)| {
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let lin2 = strip_lin(sy0, sy1);
        Ok::<_, Ssimulacra2Error>(process_strip_cached(
            lin2,
            refstack,
            sy0,
            w,
            sy1 - sy0,
            int_s - sy0,
            int_e - sy0,
            n_scales,
            opts,
            scratch,
        ))
    };

    #[cfg(feature = "rayon")]
    let strip_results: Vec<Vec<ScaleSums>> = if parallel {
        use rayon::prelude::*;
        let cap = rayon::current_num_threads().min(8).max(1);
        let mut out = Vec::with_capacity(strips.len());
        for group in strips.chunks(cap) {
            let g: Vec<Vec<ScaleSums>> = group
                .par_iter()
                .map_init(Scratch::default, |scratch, d| run_strip(scratch, d))
                .collect::<Result<Vec<_>, _>>()?;
            out.extend(g);
        }
        out
    } else {
        let mut scratch = Scratch::default();
        strips
            .iter()
            .map(|d| run_strip(&mut scratch, d))
            .collect::<Result<Vec<_>, _>>()?
    };
    #[cfg(not(feature = "rayon"))]
    let strip_results: Vec<Vec<ScaleSums>> = {
        let mut scratch = Scratch::default();
        strips
            .iter()
            .map(|d| run_strip(&mut scratch, d))
            .collect::<Result<Vec<_>, _>>()?
    };

    for sums in strip_results {
        for (scale, s) in sums.iter().enumerate() {
            if !s.initialised {
                continue;
            }
            let dst = &mut acc.per_scale[scale];
            for (d, &v) in dst.ssim_sums.iter_mut().zip(s.ssim_sums.iter()) {
                *d += v;
            }
            for (d, &v) in dst.edge_sums.iter_mut().zip(s.edge_sums.iter()) {
                *d += v;
            }
            dst.pixels += s.pixels;
            dst.initialised = true;
        }
    }
    Ok(())
}

/// Strip-walk `w`×`h` sources at one alpha-blend background, accumulating
/// per-scale interior sums. `strip_lin(i, y0, y1)` returns the linear
/// planes for rows `[y0, y1)` of source `i`.
fn accumulate_strips(
    w: usize,
    h: usize,
    strip_height: usize,
    halo: usize,
    strip_lin: impl Fn(usize, usize, usize) -> [Vec<f32>; 3] + Sync,
    opts: Opts,
    #[allow(unused_variables)] parallel: bool,
    stop: &dyn enough::Stop,
    acc: &mut StripAcc,
) -> Result<(), Ssimulacra2Error> {
    let strip_h = strip_height.max(8).min(h);
    let n_scales = acc.per_scale.len();

    // Build the strip list up front: each entry is (interior bounds,
    // strip bounds). Deterministic ordering lets the parallel path merge
    // f64 sums in a fixed order — output is bit-identical regardless of
    // thread count or scheduling.
    let mut strips = Vec::new();
    let mut y = 0usize;
    while y < h {
        let mut next_y = (y + strip_h).next_multiple_of(STRIP_ALIGN);
        if next_y >= h || h - next_y < STRIP_ALIGN {
            next_y = h;
        }
        let halo_above = halo.min(y);
        let halo_below = halo.min(h - next_y);
        strips.push((y, next_y, y - halo_above, next_y + halo_below));
        y = next_y;
    }

    // `scratch` is per-worker: parallel uses rayon's `map_init` (one
    // Scratch per pool thread, reused across all strips it runs) —
    // allocations happen once per thread, not ~30 planes per strip.
    let run_strip = |scratch: &mut Scratch,
                     &(int_s, int_e, sy0, sy1): &(usize, usize, usize, usize)| {
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let lin1 = strip_lin(0, sy0, sy1);
        let lin2 = strip_lin(1, sy0, sy1);
        Ok::<_, Ssimulacra2Error>(process_strip(
            lin1,
            lin2,
            w,
            sy1 - sy0,
            int_s - sy0,
            int_e - sy0,
            n_scales,
            opts,
            scratch,
        ))
    };

    #[cfg(feature = "rayon")]
    let strip_results: Vec<Vec<ScaleSums>> = if parallel {
        use rayon::prelude::*;
        // Strip concurrency is bandwidth-bound — beyond ~8 strips in
        // flight there is no speedup (measured: knee at T=8 on 4K) and
        // peak RSS keeps growing (~threads × strip footprint). Cap the
        // in-flight count at min(threads, 8): maximal useful speed with
        // a bounded memory multiple.
        let cap = rayon::current_num_threads().min(8).max(1);
        let mut out = Vec::with_capacity(strips.len());
        for group in strips.chunks(cap) {
            let g: Vec<Vec<ScaleSums>> = group
                .par_iter()
                .map_init(Scratch::default, |scratch, d| run_strip(scratch, d))
                .collect::<Result<Vec<_>, _>>()?;
            out.extend(g);
        }
        out
    } else {
        let mut scratch = Scratch::default();
        strips
            .iter()
            .map(|d| run_strip(&mut scratch, d))
            .collect::<Result<Vec<_>, _>>()?
    };
    #[cfg(not(feature = "rayon"))]
    let strip_results: Vec<Vec<ScaleSums>> = {
        let mut scratch = Scratch::default();
        strips
            .iter()
            .map(|d| run_strip(&mut scratch, d))
            .collect::<Result<Vec<_>, _>>()?
    };

    // Ordered merge — deterministic f64 accumulation.
    for sums in strip_results {
        for (scale, s) in sums.iter().enumerate() {
            if !s.initialised {
                continue;
            }
            let dst = &mut acc.per_scale[scale];
            for (d, &v) in dst.ssim_sums.iter_mut().zip(s.ssim_sums.iter()) {
                *d += v;
            }
            for (d, &v) in dst.edge_sums.iter_mut().zip(s.edge_sums.iter()) {
                *d += v;
            }
            dst.pixels += s.pixels;
            dst.initialised = true;
        }
    }
    Ok(())
}

/// Strip-bounded evaluation on encoded inputs.
///
/// Same semantics as [`super::compute_encoded`] — including the
/// dual-background alpha min — but peaks at O((strip + 2·halo)·width)
/// working set. Scores approximate the full-image result to within the
/// halo-tail tolerance (~1e-3 relative).
pub fn compute_encoded_strip(
    enc1: &EncodedSrgb,
    enc2: &EncodedSrgb,
    strip_height: usize,
    halo: usize,
    opts: Opts,
    parallel: bool,
) -> Result<f64, Ssimulacra2Error> {
    compute_encoded_strip_stop(
        enc1,
        enc2,
        strip_height,
        halo,
        opts,
        parallel,
        &enough::Unstoppable,
    )
}

/// [`compute_encoded_strip`] with cooperative cancellation — `stop` is
/// checked once per strip.
#[allow(clippy::too_many_arguments)]
pub fn compute_encoded_strip_stop(
    enc1: &EncodedSrgb,
    enc2: &EncodedSrgb,
    strip_height: usize,
    halo: usize,
    opts: Opts,
    parallel: bool,
    stop: &dyn enough::Stop,
) -> Result<f64, Ssimulacra2Error> {
    if enc1.width != enc2.width || enc1.height != enc2.height {
        return Err(Ssimulacra2Error::NonMatchingImageDimensions);
    }
    if enc1.width < 8 || enc1.height < 8 {
        return Err(Ssimulacra2Error::InvalidImageSize);
    }
    if enc1.alpha.is_some() || enc2.alpha.is_some() {
        let mut acc_lo = StripAcc::new(enc1.width, enc1.height);
        let mut acc_hi = StripAcc::new(enc1.width, enc1.height);
        let (e1, e2) = (enc1, enc2);
        accumulate_strips(
            enc1.width,
            enc1.height,
            strip_height,
            halo,
            |i: usize, y0: usize, y1: usize| {
                let e = if i == 0 { e1 } else { e2 };
                super::linearize(&e.strip_rows(y0, y1), 0.1)
            },
            opts,
            parallel,
            stop,
            &mut acc_lo,
        )?;
        accumulate_strips(
            enc1.width,
            enc1.height,
            strip_height,
            halo,
            |i: usize, y0: usize, y1: usize| {
                let e = if i == 0 { e1 } else { e2 };
                super::linearize(&e.strip_rows(y0, y1), 0.9)
            },
            opts,
            parallel,
            stop,
            &mut acc_hi,
        )?;
        return Ok(acc_lo.finalise(opts).min(acc_hi.finalise(opts)));
    }
    let mut acc = StripAcc::new(enc1.width, enc1.height);
    let (e1, e2) = (enc1, enc2);
    accumulate_strips(
        enc1.width,
        enc1.height,
        strip_height,
        halo,
        |i: usize, y0: usize, y1: usize| {
            let e = if i == 0 { e1 } else { e2 };
            super::linearize(&e.strip_rows(y0, y1), 0.5)
        },
        opts,
        parallel,
        stop,
        &mut acc,
    )?;
    Ok(acc.finalise(opts))
}

/// Strip-bounded match-official on *already-linear* plane triples — the
/// fallback for inputs with no encoded form (mirrors `compute()`'s
/// treatment of `to_linear_rgb()` data).
pub fn compute_linear_strip(
    p1: &[Vec<f32>; 3],
    p2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    strip_height: usize,
    halo: usize,
    opts: Opts,
    parallel: bool,
) -> Result<f64, Ssimulacra2Error> {
    compute_linear_strip_stop(
        p1,
        p2,
        width,
        height,
        strip_height,
        halo,
        opts,
        parallel,
        &enough::Unstoppable,
    )
}

/// [`compute_linear_strip`] with cooperative cancellation — `stop` is
/// checked once per strip.
#[allow(clippy::too_many_arguments)]
pub fn compute_linear_strip_stop(
    p1: &[Vec<f32>; 3],
    p2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    strip_height: usize,
    halo: usize,
    opts: Opts,
    parallel: bool,
    stop: &dyn enough::Stop,
) -> Result<f64, Ssimulacra2Error> {
    if p1[0].len() != (width * height) || p2[0].len() != (width * height) {
        return Err(Ssimulacra2Error::NonMatchingImageDimensions);
    }
    if width < 8 || height < 8 {
        return Err(Ssimulacra2Error::InvalidImageSize);
    }
    let slice = |p: &[Vec<f32>; 3], y0: usize, y1: usize| -> [Vec<f32>; 3] {
        [
            p[0][y0 * width..y1 * width].to_vec(),
            p[1][y0 * width..y1 * width].to_vec(),
            p[2][y0 * width..y1 * width].to_vec(),
        ]
    };
    let mut acc = StripAcc::new(width, height);
    accumulate_strips(
        width,
        height,
        strip_height,
        halo,
        |i, y0, y1| slice(if i == 0 { p1 } else { p2 }, y0, y1),
        opts,
        parallel,
        stop,
        &mut acc,
    )?;
    Ok(acc.finalise(opts))
}
