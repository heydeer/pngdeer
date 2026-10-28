//! pngdeer: lossy PNG compression that finds, per image, the smallest palette PNG keeping a
//! SSIMULACRA2 quality. The same code runs the command-line tool (src/main.rs) and the browser build
//! (web/wasm), and both write the same bytes for the same input.
//!
//! - color: sRGB, Oklab, and colors as seen over a dark and a light background
//! - palette: palettes (weighted median cut, k-means, error feedback)
//! - remap: pixels onto a palette (Floyd–Steinberg), neighbour smoothing
//! - search: the palette size and remapping that reach the target in the fewest bytes
//! - codec: PNG decoding, a quick palette PNG writer, and oxipng for the final file
//! - webp: WebP output, the smaller of a palette image losslessly and a lossy file of the same quality
//! - compress (here): the whole pipeline for one file

// Index loops over a color's few coordinates read better here than zipped iterators.
#![allow(clippy::needless_range_loop)]

pub mod codec;
mod color;
mod palette;
mod par;
mod remap;
pub mod search;
#[cfg(feature = "webp")]
pub mod webp;

use codec::Error;
use search::{Report, Variant, DEFAULT_VARIANTS};
#[cfg(feature = "webp")]
pub use webp::{compress_webp, CompressedWebp};

/// An 8-bit RGBA pixel (the `rgb` crate's).
pub use rgb::RGBA8 as RGBA;

/// The default preview size, for the browser and the command line alike: 5 MP screenshots in
/// seconds, for files about 1% bigger than a full-size search's.
pub const PREVIEW_PIXELS: usize = 350_000;
/// An image of at most 256 colors is kept lossless when that costs at most this much more than
/// the search's file.
const EXACT_SLACK: f64 = 0.15;

#[derive(Clone)]
pub struct Settings {
    /// SSIMULACRA2 score to reach; None picks one per image, as TinyPNG would (search.rs).
    pub target: Option<f64>,
    /// The remapping variants tried at each palette size (search::DEFAULT_VARIANTS).
    pub variants: Vec<Variant>,
    /// Search images over this many pixels on a downscaled preview, then check at full size
    /// (PREVIEW_PIXELS by default; 0 searches at full size). See search.rs.
    pub preview_pixels: usize,
    /// Milliseconds since some fixed point, for the report's time breakdown (wasm has no std clock).
    pub clock: Option<fn() -> f64>,
    /// How compress_webp encodes (the defaults are what pngdeer ships; for experiments).
    pub webp: WebpOptions,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { target: None, variants: DEFAULT_VARIANTS.to_vec(), preview_pixels: PREVIEW_PIXELS, clock: None, webp: WebpOptions::default() }
    }
}

/// compress_webp's encoder settings (webp.rs).
#[derive(Clone, Copy, Debug)]
pub struct WebpOptions {
    /// A slower lossless level (cwebp -z 7-9) to try where the lossless file may win, besides
    /// webp.rs's FAST_LEVELS: none by default, as level 9 saves about 2% for 20 times the time.
    pub slow_lossless_level: Option<u8>,
    /// The lossy candidates' method, as cwebp -m (0-6).
    pub method: u8,
    /// The lossy candidates' sharper RGB to YUV conversion (cwebp -sharp_yuv).
    pub sharp_yuv: bool,
    /// The lossy candidates' alpha plane prediction filtering: 0 none, 1 fast, 2 best (cwebp -alpha_filter).
    pub alpha_filtering: u8,
    /// Automatic quality only: the lossy quality whose file TinyPNG's WebP is (webp.rs). None keeps
    /// the PNG's automatic target instead.
    pub reference_quality: Option<u8>,
    /// The lossy candidates' auto filter (cwebp -af), which adjusts the deblocking filter per
    /// segment: 3% fewer bytes for the same score on the median image, 6% in total.
    pub autofilter: bool,
    /// The effort for the alpha plane of lossy files over 8 MP, compressed losslessly (smaller
    /// images use `method`). At method 6 libwebp runs its slowest lossless search: on a 24 MP
    /// cut-out 13 s natively for 0.25% smaller files than 5. Below 8 MP that runs alongside the PNG
    /// search, and 5 would make files 0.6% bigger on the benchmark's translucent images (up to 11%).
    pub large_alpha_effort: u8,
}

impl Default for WebpOptions {
    fn default() -> Self {
        WebpOptions {
            slow_lossless_level: None,
            method: 6,
            sharp_yuv: true,
            alpha_filtering: 1,
            reference_quality: Some(70),
            autofilter: true,
            large_alpha_effort: 5,
        }
    }
}

pub struct Compressed {
    pub png: Vec<u8>,
    pub report: Report,
    /// "hit", "miss" (target out of reach, closest kept), "exact" (lossless palette), "lossless"
    /// (a palette came out bigger than the input, so the input optimized losslessly instead), or
    /// "original" (nothing smaller than the input was found, so it is returned unchanged).
    pub outcome: &'static str,
}

impl Compressed {
    pub fn json(&self) -> String {
        let report = self.report.json();
        format!(r#"{}, "outcome": "{}", "bytes": {}}}"#, &report[..report.len() - 1], self.outcome, self.png.len())
    }
}

/// Compresses one PNG file: the search, oxipng -o 2 on its result, the input optimized losslessly
/// if the palette came out bigger, a lossless palette for images of few colors when it costs little
/// more (EXACT_SLACK), and the input itself if nothing beats it.
pub fn compress(input: &[u8], settings: &Settings) -> Result<Compressed, Error> {
    compress_decoded(input, &codec::decode(input)?, settings)
}

/// `compress` of a file already decoded.
fn compress_decoded(input: &[u8], bitmap: &codec::Bitmap, settings: &Settings) -> Result<Compressed, Error> {
    let (w, h) = (bitmap.width, bitmap.height);
    let (searched, mut report) = search::run(&bitmap.pixels, w, h, settings)?;
    let tick = || settings.clock.map_or(0.0, |c| c());
    let start = tick();
    // every output gets the input's color chunks (the search scored the pixel values as they are, so
    // the same color space applies), before any size comparison
    let color = codec::color_chunks(input);
    let mut png = codec::with_chunks(&codec::optimize(&searched)?, &color);
    let mut outcome = if report.miss { "miss" } else { "hit" };
    if png.len() > input.len() {
        // smooth gradients: the palette costs more than the source itself
        png = codec::with_chunks(&codec::optimize(input)?, &color);
        outcome = "lossless";
    } else if let Some((palette, indices)) = palette::exact_palette(&bitmap.pixels) {
        let budget = png.len() as f64 * (1.0 + EXACT_SLACK);
        let exact = codec::encode_palette(&palette, &indices, w, h);
        // oxipng shrinks this writer's palette files by 14% at most on the benchmark (10% median),
        // so skip it where even 25% couldn't bring the lossless file within budget
        if exact.len() as f64 * 0.75 <= budget {
            let exact = codec::with_chunks(&codec::optimize(&exact)?, &color);
            if exact.len() as f64 <= budget {
                png = exact;
                outcome = "exact";
            }
        }
    }
    report.profile.optimize += tick() - start;
    if png.len() >= input.len() {
        png = input.to_vec();
        outcome = "original";
    }
    Ok(Compressed { png, report, outcome })
}
