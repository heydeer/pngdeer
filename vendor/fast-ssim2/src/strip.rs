//! Strip evaluation reduces intermediate working buffers; input conversion
//! still materializes whole images. Cached comparisons also retain the full
//! reference pyramid. Select strips through [`crate::Ssimulacra2Config::with_strip`].

use crate::pipeline::{Kernel, Opts, XybFlavor};
use crate::precompute::Ssimulacra2Reference;
use crate::{Ssimulacra2Config, Ssimulacra2Error};
use zenpixels::PixelSlice;

/// Default number of halo rows above and below each strip.
///
/// Halo rows warm up the recursive blur and are excluded from reductions.
/// Larger halos cost more work and reduce boundary effects; whole-image
/// evaluation remains the choice for exact whole-image scores.
pub const HALO_ROWS_DEFAULT: usize = 96;

/// Minimum supported strip height (in scale-0 rows).
///
/// SSIMULACRA2's minimum scale-0 input is 8×8; a strip interior below
/// 8 rows would degenerate the per-scale halo accounting.
pub const MIN_STRIP_HEIGHT: usize = 8;

/// Strip-wise evaluation parameters, selected via
/// [`Ssimulacra2Config::strip`]. `strip_height` is in scale-0 rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StripConfig {
    /// Interior rows per strip (min [`MIN_STRIP_HEIGHT`]).
    pub strip_height: usize,
    /// Number of rows above and below each strip's "interior" that
    /// are processed but excluded from the per-pixel reductions.
    pub halo_rows: usize,
    /// Process strips in parallel (requires the `rayon` feature).
    ///
    /// Off by default because parallelism multiplies the memory bound:
    /// intermediate buffers are replicated per active strip. The concurrency cap is `min(threads, 8)` — at most eight strips run concurrently. On low-RAM machines keep
    /// this `false` — the memory bound is the strip path's contract.
    /// Scores are bit-identical either way (sums merge in fixed strip
    /// order).
    pub parallel_strips: bool,
}

impl Default for StripConfig {
    fn default() -> Self {
        Self {
            strip_height: 256,
            halo_rows: HALO_ROWS_DEFAULT,
            parallel_strips: false,
        }
    }
}

impl StripConfig {
    /// Set the requested number of interior rows; validated when scoring.
    #[must_use]
    pub fn new(strip_height: usize) -> Self {
        Self {
            strip_height,
            ..Self::default()
        }
    }

    /// Set the number of halo rows used to warm up each strip's blur.
    #[must_use]
    pub fn with_halo_rows(mut self, halo_rows: usize) -> Self {
        self.halo_rows = halo_rows;
        self
    }

    pub(crate) fn validate(&self) -> Result<(), Ssimulacra2Error> {
        if self.strip_height < MIN_STRIP_HEIGHT {
            return Err(Ssimulacra2Error::InvalidConfiguration(
                "strip_height must be at least 8",
            ));
        }
        #[cfg(not(feature = "rayon"))]
        if self.parallel_strips {
            return Err(Ssimulacra2Error::InvalidConfiguration(
                "parallel strips require the rayon feature",
            ));
        }
        Ok(())
    }

    /// Enable parallel strip processing (requires `rayon`; multiplies
    /// peak memory by the thread count — see [`Self::parallel_strips`]).
    #[must_use]
    pub fn with_parallel_strips(mut self, parallel: bool) -> Self {
        self.parallel_strips = parallel;
        self
    }
}

/// Internal SDR strip runner. Materializes inputs before processing strips.
pub(crate) fn compute_strip_inner(
    source: &PixelSlice<'_>,
    distorted: &PixelSlice<'_>,
    config: &Ssimulacra2Config<'_>,
) -> Result<f64, Ssimulacra2Error> {
    use crate::source::PreparedInput;

    let sc = config.strip.expect("strip config required");
    sc.validate()?;
    let stop: &dyn enough::Stop = config.stop.unwrap_or(&enough::Unstoppable);
    let opts = Opts {
        kernel: Kernel::from_impl(config.impl_type),
        flavor: XybFlavor::CubeRoot,
    };
    let strip_height = sc.strip_height;
    let p1 = crate::source::funnel(source)?;
    let p2 = crate::source::funnel(distorted)?;
    if let (PreparedInput::Encoded(e1), PreparedInput::Encoded(e2)) = (&p1, &p2) {
        // Same sub-8px crate contract as the pair path — pad encoded
        // planes (per-pixel LUT ⇒ U8-exact, identical to padding
        // post-linearization).
        let p1 = e1.reflect_padded(8);
        let p2 = e2.reflect_padded(8);
        return crate::pipeline::strip::compute_encoded_strip_stop(
            &p1,
            &p2,
            strip_height,
            sc.halo_rows,
            opts,
            sc.parallel_strips,
            stop,
        );
    }
    let (w1, h1) = p1.dims();
    let (w2, h2) = p2.dims();
    if w1 != w2 || h1 != h2 {
        return Err(Ssimulacra2Error::NonMatchingImageDimensions);
    }
    let has_alpha = crate::prepared_has_alpha(&p1) || crate::prepared_has_alpha(&p2);
    let strip_once = |bg: f32| -> Result<f64, Ssimulacra2Error> {
        let mut a = crate::linearize_prepared(&p1, w1, bg);
        let mut b = crate::linearize_prepared(&p2, w2, bg);
        let (w, h) = if w1 < 8 || h1 < 8 {
            let (pw, ph) = (w1.max(8), h1.max(8));
            a = crate::pad_planes(a, w1, h1, pw, ph);
            b = crate::pad_planes(b, w2, h2, pw, ph);
            (pw, ph)
        } else {
            (w1, h1)
        };
        crate::pipeline::strip::compute_linear_strip_stop(
            &a,
            &b,
            w,
            h,
            strip_height,
            sc.halo_rows,
            opts,
            sc.parallel_strips,
            stop,
        )
    };
    if has_alpha {
        Ok(strip_once(0.1)?.min(strip_once(0.9)?))
    } else {
        strip_once(0.5)
    }
}

impl Ssimulacra2Reference {
    /// Strip-bounded comparison — crate-internal; the public surface is
    /// [`Ssimulacra2Reference::compare_with_config`] with `config.strip`.
    pub(crate) fn compare_strip_inner(
        &self,
        distorted: &PixelSlice<'_>,
        config: &Ssimulacra2Config<'_>,
    ) -> Result<f64, Ssimulacra2Error> {
        if distorted.width() as usize != self.width() || distorted.rows() as usize != self.height()
        {
            return Err(Ssimulacra2Error::NonMatchingImageDimensions);
        }
        config.check_stop()?;
        let sc = config.strip.expect("strip config required");
        sc.validate()?;
        let stop: &dyn enough::Stop = config.stop.unwrap_or(&enough::Unstoppable);
        let strip_height = sc.strip_height;
        let cache = self.cache();
        let p = crate::source::funnel(distorted)?;
        match p {
            crate::source::PreparedInput::Encoded(e2) => {
                let p2 = e2.reflect_padded(8);
                if p2.width != cache.width() || p2.height != cache.height() {
                    return Err(Ssimulacra2Error::NonMatchingImageDimensions);
                }
                cache.compare_strip_stop_kernel(
                    &p2,
                    strip_height,
                    sc.halo_rows,
                    sc.parallel_strips,
                    stop,
                    Kernel::from_impl(config.impl_type),
                )
            }
            crate::source::PreparedInput::Linear { .. } => {
                // Linear side — compare against every ref stack; the
                // stack's bg drives the dist-side premultiply.
                let bgs: &[f32] = if cache.has_alpha() || crate::prepared_has_alpha(&p) {
                    &[0.1, 0.9]
                } else {
                    &[0.5]
                };
                let mut best = f64::INFINITY;
                for (si, &bg) in bgs.iter().enumerate() {
                    let planes = crate::linearize_prepared(&p, cache.width(), bg);
                    let (pw, ph) = (cache.width(), cache.height());
                    let planes = if p.dims() != (pw, ph) {
                        crate::pad_planes(planes, p.dims().0, p.dims().1, pw, ph)
                    } else {
                        planes
                    };
                    best = best.min(cache.compare_strip_linear_stack(
                        if cache.has_alpha() { si } else { 0 },
                        &planes,
                        strip_height,
                        sc.halo_rows,
                        sc.parallel_strips,
                        stop,
                        Kernel::from_impl(config.impl_type),
                    )?);
                }
                Ok(best)
            }
        }
    }
}
