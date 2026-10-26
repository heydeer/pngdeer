#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]
//! Precomputed reference for SDR — the official-semantics
//! analogue of [`crate::Ssimulacra2Reference`].
//!
//! Stores the reference-side per-scale planes produced by the official
//! pipeline (`xyb1`, `mu1`, `sigma1_sq` — the exact values a fresh
//! `compute_ssimulacra2` call produces, so `compare` scores are
//! bit-identical to the one-shot path). For repeated comparisons against
//! one source (encoder rate-distortion search, tuning sweeps), the
//! reference-side linearize→XYB→blur work is paid once instead of per
//! call.
//!
//! Sources with alpha precompute both blend backgrounds (0.1 / 0.9) and
//! `compare` takes the minimum, mirroring the reference binary.

use super::Kernel;
use super::gauss::create_recursive_gaussian;
use super::simd;
use super::{EncodedSrgb, ScaleAggregates, downsample_planes, final_score, linearize};
use crate::Ssimulacra2Error;
#[cfg(feature = "rayon")]
use archmage::incant;

const NUM_SCALES: usize = 6;

/// Per-scale cached reference-side planes (official semantics).
#[derive(Clone, Debug)]
pub(crate) struct RefScale {
    /// `MakePositiveXYB`-converted reference plane at this scale.
    pub(crate) xyb1: [Vec<f32>; 3],
    /// `blur(xyb1)` — the μ1 term.
    pub(crate) mu1: [Vec<f32>; 3],
    /// `blur(xyb1 * xyb1)` — the σ1² term.
    pub(crate) sigma1_sq: [Vec<f32>; 3],
    /// Scale width (ceil-halved per scale).
    pub(crate) width: usize,
    /// Scale height.
    pub(crate) height: usize,
}

/// Precomputed SDR reference.
///
/// Cloning deep-copies the buffers; keep one
/// per batch loop. `Sync` — one reference can be shared across worker
/// threads as long as each has its own distorted-side scratch.
#[derive(Clone, Debug)]
pub struct ReferenceCache {
    /// Per-scale stacks — one for opaque inputs, two (`bg = 0.1`, `0.9`)
    /// when the source has alpha.
    stacks: Vec<Vec<RefScale>>,
    width: usize,
    height: usize,
    /// `true` when the source carries alpha (compare uses min-score).
    has_alpha: bool,
}

/// Run the official scale walk on the reference side only, storing the
/// planes each scale's maps need. Mirrors `compute_opts`'s traversal
/// exactly (lagging scale gate, in-place lin→XYB, product-fused blur).
fn ref_scales(
    mut lin1: [Vec<f32>; 3],
    mut w: usize,
    mut h: usize,
    kernel: Kernel,
    stop: &dyn enough::Stop,
) -> Result<Vec<RefScale>, Ssimulacra2Error> {
    let rg = create_recursive_gaussian(1.5);
    let mut out = Vec::with_capacity(NUM_SCALES);
    let (mut gw, mut gh) = (w, h);
    for _scale in 0..NUM_SCALES {
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        if gw < 8 || gh < 8 {
            break;
        }
        let (lin1_next, nw, nh) = downsample_planes(&lin1, w, h);
        let npix = w * h;
        let mut xyb1 = lin1;
        super::xyb_convert(
            &mut xyb1,
            npix,
            super::Opts {
                kernel,
                flavor: super::XybFlavor::CubeRoot,
            },
        );

        // mu1 = blur(xyb1); sigma1 = blur(xyb1·xyb1) — the product fuses
        // into the blur input read (identical f32 product).
        let (mu1, sigma1_sq);
        #[cfg(feature = "rayon")]
        {
            use rayon::prelude::*;
            // mu: blur(xyb1); sigma1: blur(xyb1·xyb1) — 6 independent
            // channel-jobs total, no product planes materialized.
            let jobs: Vec<(usize, bool)> = (0..3).flat_map(|c| [(c, false), (c, true)]).collect();
            let flat: Vec<Vec<f32>> = jobs
                .into_par_iter()
                .map(|(c, is_prod)| {
                    let mut o = vec![0f32; npix];
                    let mut t = vec![0f32; npix];
                    let b = if is_prod { Some(&xyb1[c]) } else { None };
                    blur_channel(
                        kernel,
                        &rg,
                        &xyb1[c],
                        b.map(|v| v.as_slice()),
                        w,
                        h,
                        &mut o,
                        &mut t,
                    );
                    o
                })
                .collect();
            let t3 = |f: &[Vec<f32>], i: usize| {
                [f[2 * i].clone(), f[2 * i + 1].clone(), f[2 * i + 2].clone()]
            };
            let _ = t3;
            mu1 = [flat[0].clone(), flat[2].clone(), flat[4].clone()];
            sigma1_sq = [flat[1].clone(), flat[3].clone(), flat[5].clone()];
        }
        #[cfg(not(feature = "rayon"))]
        {
            let mut tmp = vec![0f32; npix];
            let (mut mu1_a, mut s1_a) = (vec![0f32; npix], vec![0f32; npix]);
            let (mut mu1_b, mut s1_b) = (vec![0f32; npix], vec![0f32; npix]);
            let (mut mu1_c, mut s1_c) = (vec![0f32; npix], vec![0f32; npix]);
            blur_channel(kernel, &rg, &xyb1[0], None, w, h, &mut mu1_a, &mut tmp);
            blur_channel(
                kernel,
                &rg,
                &xyb1[0],
                Some(&xyb1[0]),
                w,
                h,
                &mut s1_a,
                &mut tmp,
            );
            blur_channel(kernel, &rg, &xyb1[1], None, w, h, &mut mu1_b, &mut tmp);
            blur_channel(
                kernel,
                &rg,
                &xyb1[1],
                Some(&xyb1[1]),
                w,
                h,
                &mut s1_b,
                &mut tmp,
            );
            blur_channel(kernel, &rg, &xyb1[2], None, w, h, &mut mu1_c, &mut tmp);
            blur_channel(
                kernel,
                &rg,
                &xyb1[2],
                Some(&xyb1[2]),
                w,
                h,
                &mut s1_c,
                &mut tmp,
            );
            mu1 = [mu1_a, mu1_b, mu1_c];
            sigma1_sq = [s1_a, s1_b, s1_c];
        }

        out.push(RefScale {
            xyb1,
            mu1,
            sigma1_sq,
            width: w,
            height: h,
        });
        lin1 = lin1_next;
        gw = w;
        gh = h;
        w = nw;
        h = nh;
    }
    Ok(out)
}

impl ReferenceCache {
    /// Precompute the official reference pipeline for `source`.
    ///
    /// # Errors
    /// - [`Ssimulacra2Error::InvalidImageSize`] if `width`/`height` < 8.
    pub fn new(source: &EncodedSrgb) -> Result<Self, Ssimulacra2Error> {
        Self::new_with_options(source, Kernel::Simd, &enough::Unstoppable)
    }

    pub(crate) fn new_with_options(
        source: &EncodedSrgb,
        kernel: Kernel,
        stop: &dyn enough::Stop,
    ) -> Result<Self, Ssimulacra2Error> {
        let (w, h) = (source.width, source.height);
        if w < 8 || h < 8 {
            return Err(Ssimulacra2Error::InvalidImageSize);
        }
        let stacks = if source.alpha.is_some() {
            vec![
                ref_scales(linearize(source, 0.1), w, h, kernel, stop)?,
                ref_scales(linearize(source, 0.9), w, h, kernel, stop)?,
            ]
        } else {
            vec![ref_scales(linearize(source, 0.5), w, h, kernel, stop)?]
        };
        Ok(Self {
            stacks,
            width: w,
            height: h,
            has_alpha: source.alpha.is_some(),
        })
    }

    /// Build from already-linear RGB planes (non-encoded inputs — f32
    /// linear sources): skips `linearize`. `lin_sets` holds one entry per
    /// compositing background (1 opaque / 2 alpha, matching [`Self::new`]'s
    /// dual-bg convention); premultiply alpha upstream.
    ///
    /// # Errors
    /// - [`Ssimulacra2Error::InvalidImageSize`] if `width`/`height` < 8.
    pub fn new_linear_sets(
        lin_sets: Vec<[Vec<f32>; 3]>,
        width: usize,
        height: usize,
        has_alpha: bool,
    ) -> Result<Self, Ssimulacra2Error> {
        Self::new_linear_sets_with_options(
            lin_sets,
            width,
            height,
            has_alpha,
            Kernel::Simd,
            &enough::Unstoppable,
        )
    }

    pub(crate) fn new_linear_sets_with_options(
        lin_sets: Vec<[Vec<f32>; 3]>,
        width: usize,
        height: usize,
        has_alpha: bool,
        kernel: Kernel,
        stop: &dyn enough::Stop,
    ) -> Result<Self, Ssimulacra2Error> {
        if width < 8 || height < 8 {
            return Err(Ssimulacra2Error::InvalidImageSize);
        }
        Ok(Self {
            stacks: lin_sets
                .into_iter()
                .map(|l| ref_scales(l, width, height, kernel, stop))
                .collect::<Result<_, _>>()?,
            width,
            height,
            has_alpha,
        })
    }

    /// Compare already-linear planes against stack `si` — same
    /// distorted-side pipeline, no linearization step.
    /// `stop` is checked once per scale.
    pub(crate) fn compare_linear_stack_stop(
        &self,
        si: usize,
        lin2: [Vec<f32>; 3],
        w: usize,
        h: usize,
        stop: &dyn enough::Stop,
        kernel: Kernel,
    ) -> Result<f64, Ssimulacra2Error> {
        if w != self.width || h != self.height {
            return Err(Ssimulacra2Error::NonMatchingImageDimensions);
        }
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let rg = create_recursive_gaussian(1.5);
        let scales = dist_scales(&rg, &self.stacks[si], lin2, w, h, stop, kernel)?;
        Ok(final_score(&scales))
    }

    /// Number of stacks (1 opaque / 2 alpha).
    pub(crate) fn num_stacks(&self) -> usize {
        self.stacks.len()
    }

    /// Whether the source carried alpha (dual-bg stacks).
    pub(crate) fn has_alpha(&self) -> bool {
        self.has_alpha
    }

    /// Source width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }
    /// Number of cached scales (the lagging-gate scale count from
    /// `ref_scales` — matches what `compute_opts` produces).
    pub(crate) fn num_scales(&self) -> usize {
        self.stacks[0].len()
    }

    /// Source height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Compute the SDR score of `distorted` against the
    /// precomputed reference — bit-identical to `compute_ssimulacra2` with whole-image evaluation, with the reference-side pipeline paid
    /// once in [`Self::new`].
    ///
    /// # Errors
    /// - [`Ssimulacra2Error::NonMatchingImageDimensions`] if sizes differ.
    pub fn compare(&self, distorted: &EncodedSrgb) -> Result<f64, Ssimulacra2Error> {
        self.compare_stop(distorted, &enough::Unstoppable)
    }

    /// [`Self::compare`] with cooperative cancellation — `stop` is
    /// checked once per stack (alpha yields two).
    pub fn compare_stop(
        &self,
        distorted: &EncodedSrgb,
        stop: &dyn enough::Stop,
    ) -> Result<f64, Ssimulacra2Error> {
        self.compare_stop_kernel(distorted, stop, Kernel::Simd)
    }
    pub(crate) fn compare_stop_kernel(
        &self,
        distorted: &EncodedSrgb,
        stop: &dyn enough::Stop,
        kernel: Kernel,
    ) -> Result<f64, Ssimulacra2Error> {
        if distorted.width != self.width || distorted.height != self.height {
            return Err(Ssimulacra2Error::NonMatchingImageDimensions);
        }
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let rg = create_recursive_gaussian(1.5);
        let count = if self.has_alpha || distorted.alpha.is_some() {
            2
        } else {
            1
        };
        let scores: Vec<f64> = (0..count)
            .map(|si| {
                let refstack = &self.stacks[if self.has_alpha { si } else { 0 }];
                let bg = if count == 2 {
                    if si == 0 { 0.1 } else { 0.9 }
                } else {
                    0.5
                };
                let lin2 = linearize(distorted, bg);
                let scales =
                    dist_scales(&rg, refstack, lin2, self.width, self.height, stop, kernel);
                Ok(final_score(&scales?))
            })
            .collect::<Result<Vec<f64>, Ssimulacra2Error>>()?;
        Ok(scores.into_iter().fold(f64::INFINITY, f64::min))
    }
}

/// Distorted-side scale walk against a cached reference stack —
/// produces the same `ScaleAggregates` list `compute_opts` yields.
fn dist_scales(
    rg: &super::gauss::RecursiveGaussian,
    refstack: &[RefScale],
    mut lin2: [Vec<f32>; 3],
    mut w: usize,
    mut h: usize,
    stop: &dyn enough::Stop,
    kernel: Kernel,
) -> Result<Vec<ScaleAggregates>, Ssimulacra2Error> {
    let mut scales = Vec::with_capacity(refstack.len());
    for rs in refstack {
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let (lin2_next, _nw, _nh) = downsample_planes(&lin2, w, h);
        let npix = w * h;
        let mut xyb2 = lin2;
        super::xyb_convert(
            &mut xyb2,
            npix,
            super::Opts {
                kernel,
                flavor: super::XybFlavor::CubeRoot,
            },
        );

        // Three independent blurs: σ2 = blur(xyb2·xyb2),
        // σ12 = blur(xyb1·xyb2), μ2 = blur(xyb2).
        let (sigma2_sq, sigma12, mu2);
        #[cfg(feature = "rayon")]
        {
            use rayon::prelude::*;
            let jobs: Vec<(usize, usize)> =
                (0..3).flat_map(|j| (0..3).map(move |c| (j, c))).collect();
            let flat: Vec<Vec<f32>> = jobs
                .into_par_iter()
                .map(|(j, c)| {
                    let mut o = vec![0f32; npix];
                    let mut t = vec![0f32; npix];
                    match j {
                        0 => {
                            blur_channel(kernel, rg, &xyb2[c], Some(&xyb2[c]), w, h, &mut o, &mut t)
                        }
                        1 => blur_channel(
                            kernel,
                            rg,
                            &rs.xyb1[c],
                            Some(&xyb2[c]),
                            w,
                            h,
                            &mut o,
                            &mut t,
                        ),
                        _ => blur_channel(kernel, rg, &xyb2[c], None, w, h, &mut o, &mut t),
                    }
                    o
                })
                .collect();
            let take3 =
                |f: &[Vec<f32>], i: usize| [f[i].clone(), f[i + 1].clone(), f[i + 2].clone()];
            sigma2_sq = take3(&flat, 0);
            sigma12 = take3(&flat, 3);
            mu2 = take3(&flat, 6);
        }
        #[cfg(not(feature = "rayon"))]
        {
            let mut mul = [vec![0f32; npix], vec![0f32; npix], vec![0f32; npix]];
            let mut tmp = vec![0f32; npix];
            let mut run = |a: &[f32], b: Option<&[f32]>| -> Vec<f32> {
                let mut o = vec![0f32; npix];
                blur_channel(kernel, rg, a, b, w, h, &mut o, &mut tmp);
                o
            };
            sigma2_sq = [
                run(&xyb2[0], Some(&xyb2[0])),
                run(&xyb2[1], Some(&xyb2[1])),
                run(&xyb2[2], Some(&xyb2[2])),
            ];
            sigma12 = [
                run(&rs.xyb1[0], Some(&xyb2[0])),
                run(&rs.xyb1[1], Some(&xyb2[1])),
                run(&rs.xyb1[2], Some(&xyb2[2])),
            ];
            mu2 = [
                run(&xyb2[0], None),
                run(&xyb2[1], None),
                run(&xyb2[2], None),
            ];
            let _ = &mut mul;
        }

        let (avg_ssim, avg_edgediff) = if kernel == Kernel::Scalar {
            super::maps::maps_fused(
                &rs.mu1,
                &mu2,
                &rs.sigma1_sq,
                &sigma2_sq,
                &sigma12,
                &rs.xyb1,
                &xyb2,
                w,
                h,
            )
        } else {
            #[cfg(feature = "rayon")]
            {
                use rayon::prelude::*;
                let n = w * h;
                let opp = 1.0 / n as f64;
                let parts: Vec<(f64, f64, [f64; 4])> = (0..3)
                    .into_par_iter()
                    .map(|c| {
                        let (mut s0, mut s1) = (0f64, 0f64);
                        incant!(
                            simd::ssim_map_inner(
                                &rs.mu1[c],
                                &mu2[c],
                                &rs.sigma1_sq[c],
                                &sigma2_sq[c],
                                &sigma12[c],
                                &mut s0,
                                &mut s1
                            ),
                            [v3, neon, wasm128, scalar]
                        );
                        let mut e = [0f64; 4];
                        simd::edge_sums_fast(
                            0,
                            h,
                            w,
                            &rs.xyb1[c],
                            &rs.mu1[c],
                            &xyb2[c],
                            &mu2[c],
                            &mut e,
                        );
                        (
                            opp * s0,
                            (opp * s1).sqrt().sqrt(),
                            [
                                opp * e[0],
                                (opp * e[1]).sqrt().sqrt(),
                                opp * e[2],
                                (opp * e[3]).sqrt().sqrt(),
                            ],
                        )
                    })
                    .collect();
                let (mut so, mut eo) = ([0f64; 6], [0f64; 12]);
                for c in 0..3 {
                    so[c * 2] = parts[c].0;
                    so[c * 2 + 1] = parts[c].1;
                    eo[c * 4..c * 4 + 4].copy_from_slice(&parts[c].2);
                }
                (so, eo)
            }
            #[cfg(not(feature = "rayon"))]
            {
                simd::maps_fused_simd(
                    &rs.mu1,
                    &mu2,
                    &rs.sigma1_sq,
                    &sigma2_sq,
                    &sigma12,
                    &rs.xyb1,
                    &xyb2,
                    w,
                    h,
                )
            }
        };
        scales.push(ScaleAggregates {
            avg_ssim,
            avg_edgediff,
        });
        lin2 = lin2_next;
        w = rs.width.div_ceil(2);
        h = rs.height.div_ceil(2);
    }
    Ok(scales)
}

impl ReferenceCache {
    /// Bounded-memory `compare`: the distorted side is processed
    /// strip-by-strip against the cached reference planes (which are
    /// sliced per strip and re-blurred with the strip's IIR handling —
    /// same boundary semantics as [`crate::pipeline::strip::compute_encoded_strip`]).
    ///
    /// `strip_height`/`halo` follow the strip walker's semantics
    /// (32-aligned interior, halo covering the Gaussian tail). When
    /// `parallel` is set the strips run on the rayon pool capped at
    /// `min(threads, 8)` — same contract as
    /// [`crate::StripConfig::parallel_strips`].
    ///
    /// # Errors
    /// Same as [`Self::compare`].
    pub fn compare_strip(
        &self,
        distorted: &EncodedSrgb,
        strip_height: usize,
        halo: usize,
        parallel: bool,
    ) -> Result<f64, Ssimulacra2Error> {
        self.compare_strip_stop(
            distorted,
            strip_height,
            halo,
            parallel,
            &enough::Unstoppable,
        )
    }

    /// [`Self::compare_strip`] with cooperative cancellation — `stop` is
    /// checked once per strip.
    pub fn compare_strip_stop(
        &self,
        distorted: &EncodedSrgb,
        strip_height: usize,
        halo: usize,
        parallel: bool,
        stop: &dyn enough::Stop,
    ) -> Result<f64, Ssimulacra2Error> {
        self.compare_strip_stop_kernel(distorted, strip_height, halo, parallel, stop, Kernel::Simd)
    }
    pub(crate) fn compare_strip_stop_kernel(
        &self,
        distorted: &EncodedSrgb,
        strip_height: usize,
        halo: usize,
        parallel: bool,
        stop: &dyn enough::Stop,
        kernel: Kernel,
    ) -> Result<f64, Ssimulacra2Error> {
        if distorted.width != self.width || distorted.height != self.height {
            return Err(Ssimulacra2Error::NonMatchingImageDimensions);
        }
        let opts = super::Opts {
            kernel,
            flavor: super::XybFlavor::CubeRoot,
        };
        let mut accs = Vec::with_capacity(self.num_stacks());
        let count = if self.has_alpha || distorted.alpha.is_some() {
            2
        } else {
            1
        };
        for stack in 0..count {
            let mut acc = super::strip::StripAcc::new(self.width, self.height);
            let bg = if count == 2 {
                if stack == 0 { 0.1 } else { 0.9 }
            } else {
                0.5
            };
            let refstack = &self.stacks[if self.has_alpha { stack } else { 0 }];
            super::strip::accumulate_strips_cached(
                self.width,
                self.height,
                strip_height,
                halo,
                refstack,
                |y0, y1| super::linearize(&distorted.strip_rows(y0, y1), bg),
                opts,
                parallel,
                stop,
                &mut acc,
            )?;
            accs.push(acc.finalise(opts));
        }
        Ok(accs.into_iter().fold(f64::INFINITY, f64::min))
    }

    /// [`Self::compare_strip`] against already-linear planes — the
    /// distorted side streams row-slices of `lin2` without the
    /// encoded-space linearization step. `si` selects the ref stack
    /// (alpha caches have two, one per background).
    pub(crate) fn compare_strip_linear_stack(
        &self,
        si: usize,
        lin2: &[Vec<f32>; 3],
        strip_height: usize,
        halo: usize,
        parallel: bool,
        stop: &dyn enough::Stop,
        kernel: Kernel,
    ) -> Result<f64, Ssimulacra2Error> {
        let (w, h) = (self.width, self.height);
        if lin2[0].len() != w * h {
            return Err(Ssimulacra2Error::NonMatchingImageDimensions);
        }
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let opts = super::Opts {
            kernel,
            flavor: super::XybFlavor::CubeRoot,
        };
        let mut acc = super::strip::StripAcc::new(w, h);
        super::strip::accumulate_strips_cached(
            w,
            h,
            strip_height,
            halo,
            &self.stacks[si],
            |y0, y1| {
                let (a, b) = (y0 * w, y1 * w);
                [
                    lin2[0][a..b].to_vec(),
                    lin2[1][a..b].to_vec(),
                    lin2[2][a..b].to_vec(),
                ]
            },
            opts,
            parallel,
            stop,
            &mut acc,
        )?;
        Ok(acc.finalise(opts))
    }
}

fn blur_channel(
    kernel: Kernel,
    rg: &super::gauss::RecursiveGaussian,
    a: &[f32],
    b: Option<&[f32]>,
    w: usize,
    h: usize,
    out: &mut [f32],
    tmp: &mut [f32],
) {
    if kernel == Kernel::Simd {
        simd::fast_gaussian_simd(rg, a, b, w, h, out, tmp);
    } else {
        let product;
        let input = if let Some(b) = b {
            product = a.iter().zip(b).map(|(&a, &b)| a * b).collect::<Vec<_>>();
            &product[..]
        } else {
            a
        };
        for y in 0..h {
            rg.fast_gaussian_1d(&input[y * w..(y + 1) * w], &mut tmp[y * w..(y + 1) * w]);
        }
        rg.fast_gaussian_vertical_1d(w, h, |y, x| tmp[y * w + x], &mut |y, x, value| {
            out[y * w + x] = value
        });
    }
}
