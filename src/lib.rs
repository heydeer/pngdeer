//! pngdeer: lossy PNG compression that finds, per image, the smallest palette PNG keeping a
//! SSIMULACRA2 quality. The same code runs the command-line tool (src/main.rs) and the browser build
//! (web/wasm), and both write the same bytes for the same input.
//!
//! - color: sRGB, Oklab, and colors as seen over a dark and a light background
//! - palette: palettes (weighted median cut, k-means, error feedback)
//! - remap: pixels onto a palette (Floyd–Steinberg), neighbour smoothing
//! - search: the palette size and remapping that reach the target in the fewest bytes
//! - codec: PNG decoding, a quick palette PNG writer, and oxipng for the final file
//! - compress (here): the whole pipeline for one file

// Index loops over a color's few coordinates read better here than zipped iterators.
#![allow(clippy::needless_range_loop)]

pub mod codec;
mod color;
mod palette;
mod par;
mod remap;
pub mod search;

use codec::Error;
use search::{Report, Variant, DEFAULT_VARIANTS};

/// An 8-bit RGBA pixel (the `rgb` crate's).
pub use rgb::RGBA8 as RGBA;

/// The default preview size, for the browser and the command line alike: 5 MP screenshots in
/// seconds, for files about 1% bigger than a full-size search's.
pub const PREVIEW_PIXELS: usize = 350_000;
/// An image of at most 256 colors is kept lossless when that costs at most this much more than
/// the search's file.
const EXACT_SLACK: f64 = 0.15;

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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { target: None, variants: DEFAULT_VARIANTS.to_vec(), preview_pixels: PREVIEW_PIXELS, clock: None }
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
    let bitmap = codec::decode(input)?;
    let (w, h) = (bitmap.width, bitmap.height);
    let (searched, mut report) = search::run(&bitmap.pixels, w, h, settings)?;
    let tick = || settings.clock.map_or(0.0, |c| c());
    let start = tick();
    let mut png = codec::optimize(&searched)?;
    let mut outcome = if report.miss { "miss" } else { "hit" };
    if png.len() > input.len() {
        // smooth gradients: the palette costs more than the source itself
        png = codec::optimize(input)?;
        outcome = "lossless";
    } else if let Some((palette, indices)) = palette::exact_palette(&bitmap.pixels) {
        let budget = png.len() as f64 * (1.0 + EXACT_SLACK);
        let exact = codec::encode_palette(&palette, &indices, w, h);
        // oxipng shrinks this writer's palette files by 14% at most on the benchmark (10% median),
        // so skip it where even 25% couldn't bring the lossless file within budget
        if exact.len() as f64 * 0.75 <= budget {
            let exact = codec::optimize(&exact)?;
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
