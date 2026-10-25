//! Owned reference-side pyramid for repeated SDR comparisons.
//! Construction and comparisons honor backend selection and cancellation.
//! Strip options apply to comparisons only; reference storage remains full-image.

use crate::pipeline::precompute::ReferenceCache;
use crate::{MAX_IMAGE_PIXELS, Ssimulacra2Config, Ssimulacra2Error};
use zenpixels::PixelSlice;

/// Precomputed SSIMULACRA2 reference state for fast repeated comparisons.
///
/// Stores the reference-side per-scale planes (XYB, μ, σ) so comparing
/// multiple distorted images against the same source skips that work
/// entirely. `Clone` deep-copies the planes — keep one instance per
/// batch loop. `Send`/`Sync`: one reference can be shared across worker
/// threads.
///
/// Images below the metric's 8×8 pyramid floor are reflect-padded at
/// construction (the crate-level extension — the reference binary
/// refuses them), matching [`crate::compute_ssimulacra2`].
#[derive(Clone, Debug)]
pub struct Ssimulacra2Reference {
    cache: ReferenceCache,
    /// Dimensions of the source as the caller supplied it (before any
    /// sub-8px reflect-padding) — `compare*` validates the distorted
    /// side against these.
    original_width: usize,
    original_height: usize,
}

impl Ssimulacra2Reference {
    /// Precompute the reference pipeline for `source`.
    ///
    /// Inputs use the same descriptor-driven conversion as
    /// [`crate::compute_ssimulacra2`].
    ///
    /// # Errors
    /// - [`Ssimulacra2Error::InvalidImageSize`] if the image is empty
    ///   (0×0).
    /// - [`Ssimulacra2Error::ImageTooLarge`] if the (padded) image
    ///   exceeds [`MAX_IMAGE_PIXELS`].
    pub fn new(source: &PixelSlice<'_>) -> Result<Self, Ssimulacra2Error> {
        Self::new_with_config(source, &Ssimulacra2Config::default())
    }

    /// [`Self::new`] with explicit options — `impl_type` chooses the
    /// scalar-oracle or SIMD kernels (bit-identical output); `stop`
    /// allows cooperative cancellation of the precompute itself.
    /// `strip` must be `None`; otherwise returns
    /// [`crate::Ssimulacra2Error::InvalidConfiguration`].
    pub fn new_with_config(
        source: &PixelSlice<'_>,
        config: &Ssimulacra2Config<'_>,
    ) -> Result<Self, Ssimulacra2Error> {
        config.check_stop()?;
        if config.strip.is_some() {
            return Err(Ssimulacra2Error::InvalidConfiguration(
                "reference construction stores full-image scales; strips apply to comparison only",
            ));
        }
        let kernel = crate::pipeline::Kernel::from_impl(config.impl_type);
        let stop = config.stop.unwrap_or(&enough::Unstoppable);
        use crate::source::PreparedInput;
        match crate::source::funnel(source)? {
            PreparedInput::Encoded(e) => {
                let ow = e.width;
                let oh = e.height;
                let padded = e.reflect_padded(8);
                if padded.width * padded.height > MAX_IMAGE_PIXELS {
                    return Err(Ssimulacra2Error::ImageTooLarge {
                        actual: padded.width * padded.height,
                    });
                }
                Ok(Self {
                    cache: ReferenceCache::new_with_options(&padded, kernel, stop)?,
                    original_width: ow,
                    original_height: oh,
                })
            }
            p @ PreparedInput::Linear { .. } => {
                let (ow, oh) = p.dims();
                let (w, h) = (ow.max(8), oh.max(8));
                if w * h > MAX_IMAGE_PIXELS {
                    return Err(Ssimulacra2Error::ImageTooLarge { actual: w * h });
                }
                // Reference alpha compositing — two backgrounds on the ref
                // side; linear premult uses the linearized bg.
                let has_alpha = crate::prepared_has_alpha(&p);
                let sets = if has_alpha {
                    vec![
                        pad_or_pass(
                            crate::linearize_prepared_stop(&p, 0.1, stop)
                                .map_err(Ssimulacra2Error::Cancelled)?,
                            ow,
                            oh,
                            w,
                            h,
                        ),
                        pad_or_pass(
                            crate::linearize_prepared_stop(&p, 0.9, stop)
                                .map_err(Ssimulacra2Error::Cancelled)?,
                            ow,
                            oh,
                            w,
                            h,
                        ),
                    ]
                } else {
                    vec![pad_or_pass(
                        crate::linearize_prepared_stop(&p, 0.5, stop)
                            .map_err(Ssimulacra2Error::Cancelled)?,
                        ow,
                        oh,
                        w,
                        h,
                    )]
                };
                Ok(Self {
                    cache: ReferenceCache::new_linear_sets_with_options(
                        sets, w, h, has_alpha, kernel, stop,
                    )?,
                    original_width: ow,
                    original_height: oh,
                })
            }
        }
    }

    /// Compare a distorted image against the precomputed reference.
    ///
    /// Bit-identical to [`crate::compute_ssimulacra2`] on the same pair
    /// (encoded and linear inputs alike).
    ///
    /// # Errors
    /// - [`Ssimulacra2Error::NonMatchingImageDimensions`] if dimensions
    ///   differ from the source used at construction.
    pub fn compare(&self, distorted: &PixelSlice<'_>) -> Result<f64, Ssimulacra2Error> {
        self.compare_with_config(distorted, &Ssimulacra2Config::default())
    }

    /// [`Self::compare`] with per-call options — `strip` bounds the
    /// distorted side's memory, `stop` enables cooperative cancellation
    /// (checked at scale boundaries / per strip).
    ///
    /// # Errors
    /// As [`Self::compare`], plus [`Ssimulacra2Error::Cancelled`].
    pub fn compare_with_config(
        &self,
        distorted: &PixelSlice<'_>,
        config: &Ssimulacra2Config<'_>,
    ) -> Result<f64, Ssimulacra2Error> {
        config.check_stop()?;
        if config.strip.is_some() {
            return self.compare_strip_inner(distorted, config);
        }
        use crate::source::PreparedInput;
        let stop: &dyn enough::Stop = config.stop.unwrap_or(&enough::Unstoppable);
        let p = crate::source::funnel(distorted)?;
        if p.dims() != (self.original_width, self.original_height) {
            return Err(Ssimulacra2Error::NonMatchingImageDimensions);
        }
        match p {
            PreparedInput::Encoded(e) => {
                let padded = e.reflect_padded(8);
                self.cache.compare_stop_kernel(
                    &padded,
                    stop,
                    crate::pipeline::Kernel::from_impl(config.impl_type),
                )
            }
            p @ PreparedInput::Linear { .. } => {
                let (w, h) = (self.cache.width(), self.cache.height());
                // Evaluate dist against each ref stack; the stack's bg
                // (0.1/0.9 for alpha caches, 0.5 otherwise) drives the
                // dist-side premultiply — same pairing as compare_stop.
                let bgs: &[f32] = if self.cache.has_alpha() || crate::prepared_has_alpha(&p) {
                    &[0.1, 0.9]
                } else {
                    &[0.5]
                };
                let mut best = f64::INFINITY;
                for (si, &bg) in bgs.iter().enumerate() {
                    let planes = pad_or_pass(
                        crate::linearize_prepared_stop(&p, bg, stop)
                            .map_err(Ssimulacra2Error::Cancelled)?,
                        self.original_width,
                        self.original_height,
                        w,
                        h,
                    );
                    best = best.min(self.cache.compare_linear_stack_stop(
                        if self.cache.has_alpha() { si } else { 0 },
                        planes,
                        w,
                        h,
                        stop,
                        crate::pipeline::Kernel::from_impl(config.impl_type),
                    )?);
                }
                Ok(best)
            }
        }
    }

    /// Source width in pixels (as supplied at construction).
    #[must_use]
    pub fn width(&self) -> usize {
        self.original_width
    }

    /// Source height in pixels (as supplied at construction).
    #[must_use]
    pub fn height(&self) -> usize {
        self.original_height
    }

    /// Number of precomputed scales (image-size dependent, 1–6).
    #[must_use]
    pub fn num_scales(&self) -> usize {
        self.cache.num_scales()
    }

    /// The underlying pipeline cache — shared with the strip module.
    pub(crate) fn cache(&self) -> &ReferenceCache {
        &self.cache
    }
}

/// Mirror-pad linear planes to the 8px pyramid floor when needed.
fn pad_or_pass(planes: [Vec<f32>; 3], w: usize, h: usize, pw: usize, ph: usize) -> [Vec<f32>; 3] {
    if pw == w && ph == h {
        planes
    } else {
        crate::pad_planes(planes, w, h, pw, ph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compute_ssimulacra2, compute_ssimulacra2_with_config};
    use zenpixels::{PixelDescriptor, PixelSlice, TransferFunction};

    fn srgb_f32(data: &[[f32; 3]], w: usize, h: usize) -> PixelSlice<'_> {
        PixelSlice::new(
            bytemuck::cast_slice(data),
            w as u32,
            h as u32,
            w * 12,
            PixelDescriptor::RGBF32.with_transfer(TransferFunction::Srgb),
        )
        .unwrap()
    }

    fn lin_f32(data: &[[f32; 3]], w: usize, h: usize) -> PixelSlice<'_> {
        PixelSlice::new(
            bytemuck::cast_slice(data),
            w as u32,
            h as u32,
            w * 12,
            PixelDescriptor::RGBF32_LINEAR,
        )
        .unwrap()
    }

    /// sRGB-encoded f32 raster (values k/255 — on the u8 grid).
    fn rgb8(img: &image::RgbImage) -> (Vec<[f32; 3]>, usize, usize) {
        let (w, h) = img.dimensions();
        let px: Vec<[f32; 3]> = img
            .pixels()
            .map(|p| {
                [
                    p[0] as f32 / 255.0,
                    p[1] as f32 / 255.0,
                    p[2] as f32 / 255.0,
                ]
            })
            .collect();
        (px, w as usize, h as usize)
    }

    fn tank() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, usize, usize) {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let mk = |name: &str| rgb8(&image::open(base.join(name)).unwrap().to_rgb8());
        let (a, wa, ha) = mk("tank_source.png");
        let (b, wb, hb) = mk("tank_distorted.png");
        assert_eq!((wa, ha), (wb, hb));
        (a, b, wa, ha)
    }

    #[test]
    fn test_precompute_matches_full_compute() {
        let (a, b, w, h) = tank();
        let full = compute_ssimulacra2(&srgb_f32(&a, w, h), &srgb_f32(&b, w, h)).unwrap();
        let ref_ = Ssimulacra2Reference::new(&srgb_f32(&a, w, h)).unwrap();
        let cached = ref_.compare(&srgb_f32(&b, w, h)).unwrap();
        // bit-identical — same kernel family on both paths
        assert_eq!(full, cached, "one-shot {full} != cached {cached}");
    }

    #[test]
    fn test_precompute_scalar_matches_default() {
        let (a, b, w, h) = tank();
        let scalar = Ssimulacra2Reference::new_with_config(
            &srgb_f32(&a, w, h),
            &Ssimulacra2Config::scalar(),
        )
        .unwrap();
        let simd = Ssimulacra2Reference::new(&srgb_f32(&a, w, h)).unwrap();
        assert_eq!(
            scalar.compare(&srgb_f32(&b, w, h)).unwrap(),
            simd.compare(&srgb_f32(&b, w, h)).unwrap(),
        );
    }

    #[test]
    fn test_precompute_dimension_mismatch() {
        let (a, _b, w, h) = tank();
        let ref_ = Ssimulacra2Reference::new(&srgb_f32(&a, w, h)).unwrap();
        // 1×1 — smaller than the reference; still mismatched vs the
        // 512² source.
        let tiny = vec![[0.5f32; 3]];
        assert_eq!(
            ref_.compare(&lin_f32(&tiny, 1, 1)),
            Err(Ssimulacra2Error::NonMatchingImageDimensions)
        );
    }

    #[test]
    fn test_sub_8_reference_pads_and_matches_one_shot() {
        // 7×5 source — below the pyramid floor: `new` reflect-pads it to
        // 8×8, `compute_ssimulacra2` pads identically.
        let mk = |seed: u32| {
            let px: Vec<[f32; 3]> = (0..7 * 5)
                .map(|i| {
                    let v = (((i as u32).wrapping_mul(2654435761u32) ^ seed) >> 20) as u8;
                    [v as f32 / 255.0, v as f32 / 510.0, 1.0 - v as f32 / 255.0]
                })
                .collect();
            px
        };
        let a = mk(0xabcdef);
        let b = mk(0x123456);
        let full = compute_ssimulacra2(&srgb_f32(&a, 7, 5), &srgb_f32(&b, 7, 5)).unwrap();
        let ref_ = Ssimulacra2Reference::new(&srgb_f32(&a, 7, 5)).unwrap();
        assert_eq!(full, ref_.compare(&srgb_f32(&b, 7, 5)).unwrap());
        assert_eq!(ref_.width(), 7);
        assert_eq!(ref_.height(), 5);
    }

    #[test]
    fn test_sub_8_reference_rejects_mismatched_dims() {
        let tiny = vec![[0.5f32; 3]; 7 * 5];
        let ref_ = Ssimulacra2Reference::new(&lin_f32(&tiny, 7, 5)).unwrap();
        let larger = vec![[0.5f32; 3]; 16 * 16];
        assert_eq!(
            ref_.compare(&lin_f32(&larger, 16, 16)),
            Err(Ssimulacra2Error::NonMatchingImageDimensions)
        );
    }

    #[test]
    fn test_precompute_metadata() {
        let (a, _b, w, h) = tank();
        let ref_ = Ssimulacra2Reference::new(&srgb_f32(&a, w, h)).unwrap();
        assert_eq!(ref_.width(), w);
        assert_eq!(ref_.height(), h);
        assert!((1..=6).contains(&ref_.num_scales()));
    }

    #[test]
    fn test_precompute_linear_source() {
        // LinearRgbImage input — no encoded form, cache built from
        // linear planes; must match one-shot compute bit-for-bit.
        let px: Vec<[f32; 3]> = (0..64 * 64)
            .map(|i| {
                let f = (i as f32 * 0.6180339) % 1.0;
                [f, f * 0.7, f * 0.3]
            })
            .collect();
        let px2: Vec<[f32; 3]> = px.iter().map(|p| [p[0] * 0.95, p[1], p[2]]).collect();
        let full = compute_ssimulacra2_with_config(
            &lin_f32(&px, 64, 64),
            &lin_f32(&px2, 64, 64),
            &Ssimulacra2Config::default(),
        )
        .unwrap();
        let ref_ = Ssimulacra2Reference::new(&lin_f32(&px, 64, 64)).unwrap();
        assert_eq!(full, ref_.compare(&lin_f32(&px2, 64, 64)).unwrap());
    }
}
