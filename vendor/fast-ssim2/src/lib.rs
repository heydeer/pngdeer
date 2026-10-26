#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod input;
#[cfg(feature = "unstable-internals")]
#[doc(hidden)]
pub mod pipeline;
#[cfg(not(feature = "unstable-internals"))]
#[allow(dead_code)]
mod pipeline;
mod precompute;
mod source;

mod strip;
mod weights;

use enough::Stop;

pub use precompute::Ssimulacra2Reference;
pub use strip::{HALO_ROWS_DEFAULT, MIN_STRIP_HEIGHT, StripConfig};
pub use zenpixels::{PixelBuffer, PixelDescriptor, PixelSlice, TransferFunction};

// sRGB→linear for callers: use `linear_srgb::default::*` directly.

/// SIMD implementation backend for all operations (blur, XYB conversion, SSIM computation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimdImpl {
    /// Scalar implementation (baseline, most portable)
    Scalar,
    /// Cross-platform SIMD via archmage (default, AVX2/AVX-512/NEON/WASM128)
    #[default]
    Simd,
}

impl SimdImpl {
    /// Returns the name of this implementation
    pub fn name(&self) -> &'static str {
        match self {
            SimdImpl::Scalar => "scalar",
            SimdImpl::Simd => "simd (archmage)",
        }
    }
}

/// Configuration for SSIMULACRA2 computation.
///
/// [`crate::SimdImpl`] selects scalar or runtime-dispatched SIMD kernels.
/// Input conversion follows the pixel descriptor; see the crate-level input
/// semantics for the scope of reference parity.
/// Per-call options. `Default` = SIMD kernels, whole-image, unstoppable
/// — identical to plain [`compute_ssimulacra2`].
///
/// `Debug` reports the presence of a cancellation token without inspecting it.
#[derive(Clone, Copy, Default)]
#[non_exhaustive]
pub struct Ssimulacra2Config<'a> {
    /// Kernel backend for all operations.
    pub impl_type: SimdImpl,
    /// Strip-wise evaluation for bounded memory on large images —
    /// `Some(StripConfig)` routes to the strip pipeline.
    pub strip: Option<StripConfig>,
    /// Cooperative cancellation token — checked once per scale or per
    /// strip, never per-pixel.
    pub stop: Option<&'a dyn enough::Stop>,
}

impl core::fmt::Debug for Ssimulacra2Config<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ssimulacra2Config")
            .field("impl_type", &self.impl_type)
            .field("strip", &self.strip)
            .field("stop", &self.stop.map(|_| "<cancellation token>"))
            .finish()
    }
}

impl<'a> Ssimulacra2Config<'a> {
    pub(crate) fn check_stop(&self) -> Result<(), Ssimulacra2Error> {
        if let Some(stop) = self.stop {
            stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        }
        Ok(())
    }

    /// Select the kernel backend while retaining other options.
    #[must_use]
    pub fn with_impl(mut self, impl_type: SimdImpl) -> Self {
        self.impl_type = impl_type;
        self
    }

    /// Enable strip evaluation while retaining other options.
    #[must_use]
    pub fn with_strip(mut self, strip: StripConfig) -> Self {
        self.strip = Some(strip);
        self
    }

    /// Create configuration with specified implementation.
    #[must_use]
    pub fn new(impl_type: SimdImpl) -> Self {
        Self {
            impl_type,
            strip: None,
            stop: None,
        }
    }

    /// Default configuration using SIMD kernels.
    #[must_use]
    pub fn simd() -> Self {
        Self::new(SimdImpl::Simd)
    }

    /// Scalar configuration — the reference-order oracle kernels.
    #[must_use]
    pub fn scalar() -> Self {
        Self::new(SimdImpl::Scalar)
    }

    /// Strip-wise evaluation (bounded memory): `strip_height` rows per
    /// strip interior, default halo, serial.
    #[must_use]
    pub fn strips(strip_height: usize) -> Self {
        Self {
            strip: Some(StripConfig {
                strip_height,
                ..Default::default()
            }),
            ..Self::default()
        }
    }

    /// Attach a cancellation token.
    #[must_use]
    pub fn with_stop(mut self, stop: &'a dyn enough::Stop) -> Self {
        self.stop = Some(stop);
        self
    }
}

/// Errors which can occur when attempting to calculate a SSIMULACRA2 score from two input images.
///
/// `#[non_exhaustive]`: downstream `match` arms must include a wildcard `_ =>`,
/// so future variants can be added without
/// breaking callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Ssimulacra2Error {
    /// An option is invalid or unsupported by the selected operation.
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(&'static str),

    /// An input [`crate::PixelSlice`]'s descriptor declares
    /// something the metric can't score honestly — HDR transfers
    /// (PQ/HLG), narrow/limited signal range, or a pixel layout with no
    /// mapping to the SDR sRGB pipeline.
    #[error("Unsupported input: {0}")]
    UnsupportedInput(&'static str),

    /// The two input images do not have the same width and height.
    #[error("Source and distorted image width and height must be equal")]
    NonMatchingImageDimensions,

    /// An input has zero width or height. Nonempty sub-8px images are padded.
    #[error("Image width and height must be nonzero")]
    InvalidImageSize,

    /// Original or padded pixel count exceeds [`crate::MAX_IMAGE_PIXELS`].
    #[error(
        "Image is too large: {actual} pixels exceeds limit of {} pixels",
        MAX_IMAGE_PIXELS
    )]
    ImageTooLarge {
        /// Pixel count of the offending original or padded image.
        actual: usize,
    },

    /// Gaussian blur operation failed.
    #[error("Gaussian blur operation failed")]
    GaussianBlurError,

    /// The computation was cooperatively cancelled via the
    /// [`enough::Stop`] token attached with [`crate::Ssimulacra2Config::with_stop`].
    ///
    /// The token is polled at the top of each multi-scale (and, in the
    /// strip APIs, per-strip) outer-loop iteration, never inside the
    /// per-pixel inner loops, so cancellation is responsive without
    /// adding overhead to the hot path.
    #[error("Computation cancelled: {0}")]
    Cancelled(enough::StopReason),
}

/// Maximum supported original or padded image size in pixels.
///
/// This is a pixel-count limit, not a peak-memory guarantee. Input conversion,
/// full-image processing, and cached references allocate image-sized buffers.
/// Applications should apply their own resource limits before scoring.
pub const MAX_IMAGE_PIXELS: usize = 16_384 * 16_384;

/// Computes the SSIMULACRA2 score from two [`PixelSlice`]s —
/// borrowed, self-describing views over pixel bytes (stride, format,
/// transfer, primaries, and alpha all live in the descriptor).
///
/// Wrap raw buffers with `PixelSlice::new(bytes, w, h, stride, descriptor)`;
/// `imgref`/`rgb`-crate images convert via `PixelSlice::from` (the
/// `imgref` feature forwards to `zenpixels/imgref`); `PixelBuffer`
/// callers pass `&buf.as_slice()`.
///
/// # Color space conventions
/// The descriptor declares transfer function and primaries explicitly.
/// SDR integer inputs must declare sRGB; f32 may declare sRGB or Linear.
/// Grayscale types are expanded to RGB (R=G=B).
///
/// # Example
/// ```
/// use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
/// # let (w, h) = (8u32, 8u32);
/// # let rgb = vec![128; 8 * 8 * 3];
/// # let rgb2 = rgb.clone();
///
/// let source = PixelSlice::new(&rgb, w, h, w as usize * 3, PixelDescriptor::RGB8_SRGB)?;
/// let distorted = PixelSlice::new(&rgb2, w, h, w as usize * 3, PixelDescriptor::RGB8_SRGB)?;
/// let score = compute_ssimulacra2(&source, &distorted)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn compute_ssimulacra2(
    source: &zenpixels::PixelSlice<'_>,
    distorted: &zenpixels::PixelSlice<'_>,
) -> Result<f64, Ssimulacra2Error> {
    compute_ssimulacra2_with_config(source, distorted, &Ssimulacra2Config::default())
}

/// Computes the SSIMULACRA2 score with custom configuration.
///
/// [`Ssimulacra2Config`] carries everything per-call: kernel family
/// (`impl_type`), bounded-memory strips (`strip`), and cooperative
/// cancellation (`stop` — checked once per scale or per strip, never
/// per-pixel; returns [`Ssimulacra2Error::Cancelled`]).
///
/// Use [`crate::Ssimulacra2Config::strips`] for default strip options,
/// or compose options with [`crate::Ssimulacra2Config::with_strip`].
pub fn compute_ssimulacra2_with_config(
    source: &zenpixels::PixelSlice<'_>,
    distorted: &zenpixels::PixelSlice<'_>,
    config: &Ssimulacra2Config<'_>,
) -> Result<f64, Ssimulacra2Error> {
    validate_pair(source, distorted)?;
    config.check_stop()?;
    if config.strip.is_some() {
        return crate::strip::compute_strip_inner(source, distorted, config);
    }
    let kernel = pipeline::Kernel::from_impl(config.impl_type);
    let stop: &dyn enough::Stop = config.stop.unwrap_or(&enough::Unstoppable);
    compute_pair(source, distorted, kernel, stop)
}

/// HDR variant (`hdr-pu` feature): SSIMULACRA2 with the cube-root opsin
/// replaced by **PU21** perceptual encoding of absolute luminance (cd/m²).
///
/// Accepted inputs:
/// - `TransferFunction::Linear` f32 — pixels are absolute nits.
/// - `TransferFunction::Pq`/`Hlg` (u8/u16/f32) — EOTF-decoded to nits
///   internally (PQ: ST 2084 → 0–10 000 cd/m²; HLG: inverse-OETF +
///   system-gamma 1.2 OOTF on a 1000 cd/m² reference display).
///
/// BT.2020 primaries are *not* gamut-converted — the opsin consumes the
/// declared primaries directly. Both inputs must use the same primaries.
///
/// **Scores are not comparable to [`compute_ssimulacra2`] scores** — this
/// is a different perceptual-encoding regime for HDR content.
#[cfg(feature = "hdr-pu")]
pub fn compute_ssimulacra2_pu(
    source: &zenpixels::PixelSlice<'_>,
    distorted: &zenpixels::PixelSlice<'_>,
) -> Result<f64, Ssimulacra2Error> {
    compute_ssimulacra2_pu_with_config(source, distorted, &Ssimulacra2Config::default())
}

/// [`compute_ssimulacra2_pu`] with custom configuration (kernel +
/// cancellation). Strip options return [`crate::Ssimulacra2Error::InvalidConfiguration`].
#[cfg(feature = "hdr-pu")]
pub fn compute_ssimulacra2_pu_with_config(
    source: &zenpixels::PixelSlice<'_>,
    distorted: &zenpixels::PixelSlice<'_>,
    config: &Ssimulacra2Config<'_>,
) -> Result<f64, Ssimulacra2Error> {
    validate_pair(source, distorted)?;
    config.check_stop()?;
    if config.strip.is_some() {
        return Err(Ssimulacra2Error::InvalidConfiguration(
            "HDR strip evaluation is not supported",
        ));
    }
    let stop: &dyn enough::Stop = config.stop.unwrap_or(&enough::Unstoppable);
    let stop = stop.may_stop().then_some(stop);
    let kernel = pipeline::Kernel::from_impl(config.impl_type);
    let p1 = source::funnel_nits(source)?;
    let p2 = source::funnel_nits(distorted)?;
    let (w1, h1) = p1.dims();
    let (w2, h2) = p2.dims();
    if w1 != w2 || h1 != h2 {
        return Err(Ssimulacra2Error::NonMatchingImageDimensions);
    }
    let opts = pipeline::Opts {
        kernel,
        flavor: pipeline::XybFlavor::Pu21,
    };
    // Alpha: composite onto the SDR dark/light-equivalent backgrounds
    // (20 / 200 cd/m²), mirroring the encoded path's 0.1/0.9 convention.
    let has_alpha = prepared_has_alpha(&p1) || prepared_has_alpha(&p2);
    let once = |bg: f32| -> Result<f64, Ssimulacra2Error> {
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let mut a = linearize_nits(&p1, bg);
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let mut b = linearize_nits(&p2, bg);
        let (w, h) = if w1 < 8 || h1 < 8 {
            let (pw, ph) = (w1.max(8), h1.max(8));
            a = pad_planes(a, w1, h1, pw, ph);
            b = pad_planes(b, w2, h2, pw, ph);
            (pw, ph)
        } else {
            (w1, h1)
        };
        pipeline::compute_planar_stop(a, b, w, h, opts, stop.unwrap_or(&enough::Unstoppable))
    };
    if has_alpha {
        Ok(once(20.0)?.min(once(200.0)?))
    } else {
        once(200.0)
    }
}

fn validate_pair(
    source: &PixelSlice<'_>,
    distorted: &PixelSlice<'_>,
) -> Result<(), Ssimulacra2Error> {
    if source.width() != distorted.width() || source.rows() != distorted.rows() {
        return Err(Ssimulacra2Error::NonMatchingImageDimensions);
    }
    Ok(())
}

#[cfg(feature = "hdr-pu")]
fn linearize_nits(p: &source::PreparedInput, background_nits: f32) -> [Vec<f32>; 3] {
    let source::PreparedInput::Linear { planes, alpha, .. } = p else {
        unreachable!("the HDR funnel produces linear nits")
    };
    composite_linear(planes, alpha.as_deref(), background_nits)
}

fn composite_linear(
    planes: &[Vec<f32>; 3],
    alpha: Option<&[f32]>,
    background: f32,
) -> [Vec<f32>; 3] {
    match alpha {
        None => planes.clone(),
        Some(alpha) => planes.clone().map(|channel| {
            channel
                .iter()
                .zip(alpha)
                .map(|(&value, &a)| a * value + (1.0 - a) * background)
                .collect()
        }),
    }
}

/// Rows between `stop` polls in [`composite_linear_stop`]'s per-channel
/// premultiply pass — the check stays out of the per-pixel map.
const COMPOSITE_STOP_ROWS: usize = 64;

/// [`composite_linear`] with cooperative cancellation — row-chunked
/// (`width`-sized slices), so the inner map still vectorizes.
fn composite_linear_stop(
    planes: &[Vec<f32>; 3],
    alpha: Option<&[f32]>,
    background: f32,
    width: usize,
    stop: &dyn enough::Stop,
) -> Result<[Vec<f32>; 3], enough::StopReason> {
    if !stop.may_stop() {
        return Ok(composite_linear(planes, alpha, background));
    }
    let stop = stop.may_stop().then_some(stop);
    match alpha {
        None => Ok(planes.clone()),
        Some(alpha) => {
            let mut out = [Vec::new(), Vec::new(), Vec::new()];
            for (c, channel) in planes.iter().enumerate() {
                if width == 0 || channel.len() % width != 0 || channel.len() > alpha.len() {
                    // Degenerate geometry — keep the non-stop path's
                    // zip-truncation semantics.
                    return Ok(composite_linear(planes, Some(alpha), background));
                }
                let mut dst = Vec::with_capacity(channel.len());
                for (r, row) in channel.chunks_exact(width).enumerate() {
                    if r & (COMPOSITE_STOP_ROWS - 1) == 0 {
                        stop.check()?;
                    }
                    dst.extend(
                        row.iter()
                            .zip(&alpha[r * width..])
                            .map(|(&value, &a)| a * value + (1.0 - a) * background),
                    );
                }
                out[c] = dst;
            }
            Ok(out)
        }
    }
}

/// Core pair-scoring entry: the reference SSIMULACRA2.1 pipeline
/// reproduced bit-for-bit.
///
/// Encoded sRGB inputs are linearized through the captured reference
/// LUTs; already-linear inputs are used as-is (the reference binary
/// never sees such inputs, so bit-exactness there is undefined).
fn compute_pair(
    source: &zenpixels::PixelSlice<'_>,
    distorted: &zenpixels::PixelSlice<'_>,
    kernel: pipeline::Kernel,
    stop: &dyn enough::Stop,
) -> Result<f64, Ssimulacra2Error> {
    use source::PreparedInput;

    let stop = stop.may_stop().then_some(stop);
    let p1 = source::funnel(source)?;
    let p2 = source::funnel(distorted)?;

    if let (PreparedInput::Encoded(e1), PreparedInput::Encoded(e2)) = (&p1, &p2) {
        // Sub-8px inputs: the reference binary refuses them, but the
        // crate's `compute_ssimulacra2` contract scores down to 1×1 via
        // mirror padding — apply it on encoded planes (per-pixel LUT ⇒
        // identical to padding post-linearization, and stays U8-exact).
        let q1 = e1.reflect_padded(8);
        let q2 = e2.reflect_padded(8);
        return pipeline::compute_encoded_stop(&q1, &q2, kernel, &stop);
    }

    let (w1, h1) = p1.dims();
    let (w2, h2) = p2.dims();
    if w1 != w2 || h1 != h2 {
        return Err(Ssimulacra2Error::NonMatchingImageDimensions);
    }

    let has_alpha = prepared_has_alpha(&p1) || prepared_has_alpha(&p2);

    // Linear-side mirror padding to the 8px pyramid floor (crate contract;
    // the reference binary refuses such inputs — padding is our extension).
    let (pw, ph) = (w1.max(8), h1.max(8));
    let opts = pipeline::Opts {
        kernel,
        flavor: pipeline::XybFlavor::CubeRoot,
    };

    // Reference alpha compositing: min over two backgrounds (0.1 / 0.9
    // encoded). Linear inputs replicate it with the linearized bg.
    let eval = |bg: f32, p1: &PreparedInput, p2: &PreparedInput| -> Result<f64, Ssimulacra2Error> {
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let l1 = linearize_prepared_stop(p1, bg, &stop).map_err(Ssimulacra2Error::Cancelled)?;
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let l2 = linearize_prepared_stop(p2, bg, &stop).map_err(Ssimulacra2Error::Cancelled)?;
        stop.check().map_err(Ssimulacra2Error::Cancelled)?;
        let (l1, l2) = if pw != w1 || ph != h1 {
            (
                pad_planes(l1, w1, h1, pw, ph),
                pad_planes(l2, w2, h2, pw, ph),
            )
        } else {
            (l1, l2)
        };
        pipeline::compute_planar_stop(l1, l2, pw, ph, opts, &stop)
    };

    if has_alpha {
        Ok(eval(0.1, &p1, &p2)?.min(eval(0.9, &p1, &p2)?))
    } else {
        eval(0.5, &p1, &p2)
    }
}

pub(crate) fn prepared_has_alpha(p: &source::PreparedInput) -> bool {
    match p {
        source::PreparedInput::Encoded(e) => e.alpha.is_some(),
        source::PreparedInput::Linear { alpha, .. } => alpha.is_some(),
    }
}

/// Linearize a funnelled input: encoded data goes through the reference
/// LUTs (alpha premultiplied onto `bg` in encoded space), linear planes
/// premultiply alpha onto `srgb_to_linear(bg)` for parity.
pub(crate) fn linearize_prepared(p: &source::PreparedInput, _w: usize, bg: f32) -> [Vec<f32>; 3] {
    match p {
        source::PreparedInput::Encoded(e) => pipeline::linearize(e, bg),
        source::PreparedInput::Linear { planes, alpha, .. } => {
            composite_linear(planes, alpha.as_deref(), input::srgb_to_linear(bg))
        }
    }
}

/// [`linearize_prepared`] with cooperative cancellation — the encoded
/// path polls between row chunks in [`pipeline::linearize_stop`], the
/// linear path in [`composite_linear_stop`].
pub(crate) fn linearize_prepared_stop(
    p: &source::PreparedInput,
    bg: f32,
    stop: &dyn enough::Stop,
) -> Result<[Vec<f32>; 3], enough::StopReason> {
    match p {
        source::PreparedInput::Encoded(e) => pipeline::linearize_stop(e, bg, stop),
        source::PreparedInput::Linear { planes, alpha, .. } => {
            let (w, _) = p.dims();
            composite_linear_stop(planes, alpha.as_deref(), input::srgb_to_linear(bg), w, stop)
        }
    }
}

/// Reflect-101 pad linear planes to `pw × ph` (original at top-left).
pub(crate) fn pad_planes(
    planes: [Vec<f32>; 3],
    w: usize,
    h: usize,
    pw: usize,
    ph: usize,
) -> [Vec<f32>; 3] {
    planes.map(|data| {
        let mut out = Vec::with_capacity(pw * ph);
        for y in 0..ph {
            let row = reflect_index(y, h) * w;
            for x in 0..pw {
                out.push(data[row + reflect_index(x, w)]);
            }
        }
        out
    })
}

/// Reflect-101 index map (OpenCV `BORDER_REFLECT_101`): fold an
/// out-of-range index `i` back into `[0, n)` by mirroring at the borders
/// without repeating the edge sample. Identity for `i < n`; `n <= 1`
/// collapses to 0.
#[inline]
pub(crate) fn reflect_index(i: usize, n: usize) -> usize {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Wrap `&[[f32; 3]]` as an encoded-sRGB `PixelSlice`.
    fn srgb_f32(data: &[[f32; 3]], w: usize, h: usize) -> zenpixels::PixelSlice<'_> {
        zenpixels::PixelSlice::new(
            bytemuck::cast_slice(data),
            w as u32,
            h as u32,
            w * 12,
            zenpixels::PixelDescriptor::RGBF32.with_transfer(zenpixels::TransferFunction::Srgb),
        )
        .unwrap()
    }

    /// Wrap `&[[f32; 3]]` as a linear `PixelSlice`.
    fn lin_f32(data: &[[f32; 3]], w: usize, h: usize) -> zenpixels::PixelSlice<'_> {
        zenpixels::PixelSlice::new(
            bytemuck::cast_slice(data),
            w as u32,
            h as u32,
            w * 12,
            zenpixels::PixelDescriptor::RGBF32_LINEAR,
        )
        .unwrap()
    }

    /// Wrap `&[[u8; 3]]` as `RGB8_SRGB`.
    fn rgb8_slice(data: &[[u8; 3]], w: usize, h: usize) -> zenpixels::PixelSlice<'_> {
        zenpixels::PixelSlice::new(
            bytemuck::cast_slice(data),
            w as u32,
            h as u32,
            w * 3,
            zenpixels::PixelDescriptor::RGB8_SRGB,
        )
        .unwrap()
    }

    /// sRGB-encoded f32 raster — the on-grid encoded-input path.
    fn tank_rgb() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, u32, u32) {
        let mk = |name: &str| {
            let img = image::open(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("test_data")
                    .join(name),
            )
            .unwrap();
            (
                img.to_rgb32f().as_chunks::<3>().0.to_vec(),
                img.width(),
                img.height(),
            )
        };
        let (d1, w1, h1) = mk("tank_source.png");
        let (d2, w2, h2) = mk("tank_distorted.png");
        assert_eq!((w1, h1), (w2, h2));
        (d1, d2, w1, h1)
    }

    #[test]
    fn test_ssimulacra2() {
        let (s, d, w, h) = tank_rgb();
        let score = compute_ssimulacra2(
            &srgb_f32(&s, w as usize, h as usize),
            &srgb_f32(&d, w as usize, h as usize),
        )
        .unwrap();
        assert!(
            (17.0..=18.0).contains(&score),
            "tank score {score} outside expected band (ref-decoder ~17.4)"
        );
    }

    #[test]
    fn test_u8_and_f32_encoded_nearly_agree() {
        // u8 (captured LUT) vs f32-encoded sRGB (rational poly): the
        // general f32 path approximates the LUT within ~1e-7/pixel, so
        // scores stay within a small tolerance — they're *sane-equal*,
        // not bit-exact (quantized callers get exactness via u8).
        let s = image::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("test_data")
                .join("tank_source.png"),
        )
        .unwrap()
        .to_rgb8();
        let d = image::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("test_data")
                .join("tank_distorted.png"),
        )
        .unwrap()
        .to_rgb8();
        let (w, h) = s.dimensions();
        let mk = |img: &image::RgbImage| {
            img.pixels()
                .map(|p| [p[0], p[1], p[2]])
                .collect::<Vec<[u8; 3]>>()
        };
        let (a8, b8) = (mk(&s), mk(&d));
        let mkr = |img: &image::RgbImage| {
            img.pixels()
                .map(|p| {
                    [
                        p[0] as f32 / 255.0,
                        p[1] as f32 / 255.0,
                        p[2] as f32 / 255.0,
                    ]
                })
                .collect::<Vec<[f32; 3]>>()
        };
        let (af, bf) = (mkr(&s), mkr(&d));
        let score_u8 = compute_ssimulacra2(
            &rgb8_slice(&a8, w as usize, h as usize),
            &rgb8_slice(&b8, w as usize, h as usize),
        )
        .unwrap();
        let score_f32 = compute_ssimulacra2(
            &srgb_f32(&af, w as usize, h as usize),
            &srgb_f32(&bf, w as usize, h as usize),
        )
        .unwrap();
        assert!(
            (score_u8 - score_f32).abs() < 0.15,
            "u8 {score_u8} vs f32-encoded {score_f32} — poly-vs-LUT drift"
        );
    }

    /// Construct a mid-gray linear image.
    fn make_linear_rgb(width: usize, height: usize) -> Vec<[f32; 3]> {
        vec![[0.5f32, 0.5, 0.5]; width * height]
    }

    #[test]
    fn test_compute_accepts_small_input() {
        let img = make_linear_rgb(16, 16);
        let score = compute_ssimulacra2_with_config(
            &lin_f32(&img, 16, 16),
            &lin_f32(&img, 16, 16),
            &Ssimulacra2Config::default(),
        )
        .expect("16x16 grey image must be accepted");
        assert!(
            (score - 100.0).abs() < 0.01,
            "identical images should score 100, got {score}"
        );
    }

    #[test]
    fn test_sub_8_reflect_pads_instead_of_rejecting() {
        // Sub-8px inputs reflect(mirror)-pad up to the pyramid floor and
        // score (down to 1×1) rather than erroring. Identical pairs
        // still score ~100.
        for (w, h) in [(4usize, 4usize), (1, 1), (3, 7), (7, 3)] {
            let img = make_linear_rgb(w, h);
            let score = compute_ssimulacra2_with_config(
                &lin_f32(&img, w, h),
                &lin_f32(&img, w, h),
                &Ssimulacra2Config::default(),
            )
            .unwrap_or_else(|e| panic!("{w}x{h} must score, got {e:?}"));
            assert!(
                (score - 100.0).abs() < 0.01,
                "identical {w}x{h} should score ~100, got {score}"
            );
        }
        // A real sub-8 difference yields a finite score below 100.
        let a = make_linear_rgb(5, 5);
        let b = vec![[0.9f32, 0.1, 0.2]; 25];
        let s = compute_ssimulacra2_with_config(
            &lin_f32(&a, 5, 5),
            &lin_f32(&b, 5, 5),
            &Ssimulacra2Config::default(),
        )
        .expect("5x5 differing pair must score");
        assert!(s.is_finite() && s < 100.0, "5x5 differing score {s}");
    }

    #[test]
    fn test_scalar_kernels_bit_identical() {
        let (sa, sb, w, h) = tank_rgb();
        let (w, h) = (w as usize, h as usize);
        let simd = compute_ssimulacra2_with_config(
            &srgb_f32(&sa, w, h),
            &srgb_f32(&sb, w, h),
            &Ssimulacra2Config::simd(),
        )
        .unwrap();
        let scalar = compute_ssimulacra2_with_config(
            &srgb_f32(&sa, w, h),
            &srgb_f32(&sb, w, h),
            &Ssimulacra2Config::scalar(),
        )
        .unwrap();
        assert_eq!(simd, scalar, "scalar vs simd kernel mismatch");
    }
}
