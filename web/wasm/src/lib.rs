//! pngdeer in the browser: pngdeer::compress, compress_webp and compress_jpeg compiled to WebAssembly,
//! single-threaded or (feature `threads`) with a rayon pool on Web Workers. Same output as the
//! command line.

use wasm_bindgen::prelude::*;

/// Starts the rayon thread pool (threaded build only): `await initThreadPool(n)` before compressing.
#[cfg(feature = "threads")]
pub use wasm_bindgen_rayon::init_thread_pool;

#[wasm_bindgen]
pub struct Compressed {
    png: Vec<u8>,
    report: String,
}

#[wasm_bindgen]
impl Compressed {
    /// The compressed PNG file.
    #[wasm_bindgen(getter)]
    pub fn png(&self) -> Vec<u8> {
        self.png.clone()
    }

    /// JSON: target, colors, variant, score, evals, miss, outcome, bytes, and a time breakdown.
    #[wasm_bindgen(getter)]
    pub fn report(&self) -> String {
        self.report.clone()
    }
}

fn settings(target: Option<f64>, preview_pixels: Option<u32>) -> pngdeer::Settings {
    let defaults = pngdeer::Settings::default();
    pngdeer::Settings { target, preview_pixels: preview_pixels.map_or(defaults.preview_pixels, |p| p as usize), clock: Some(js_sys::Date::now), ..defaults }
}

/// Compresses one PNG file to the smallest palette PNG scoring at least `target` (SSIMULACRA2), or
/// with no target at a quality picked per image (auto mode). Images over `preview_pixels` (default
/// pngdeer::PREVIEW_PIXELS, as on the command line) are searched on a downscaled preview and checked
/// at full size; 0 searches at full size, several times slower on big images.
#[wasm_bindgen]
pub fn compress(input: &[u8], target: Option<f64>, preview_pixels: Option<u32>) -> Result<Compressed, JsError> {
    let result = pngdeer::compress(input, &settings(target, preview_pixels)).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(Compressed { report: result.json(), png: result.png })
}

#[wasm_bindgen]
pub struct CompressedWebp {
    webp: Vec<u8>,
    report: String,
}

#[wasm_bindgen]
impl CompressedWebp {
    /// The WebP file.
    #[wasm_bindgen(getter)]
    pub fn webp(&self) -> Vec<u8> {
        self.webp.clone()
    }

    /// JSON: target, mode (lossy or lossless), quality, score, both candidates' sizes, the PNG's
    /// report, bytes, and a time breakdown.
    #[wasm_bindgen(getter)]
    pub fn report(&self) -> String {
        self.report.clone()
    }
}

/// Compresses one PNG file to WebP (pngdeer::compress_webp): the smaller of its palette image stored
/// losslessly and the lowest lossy quality reaching the target; with no target, TinyPNG's WebP
/// quality for the image. `preview_pixels` as for `compress`. A JPEG becomes lossy WebP, at JPEG
/// output's automatic quality.
#[wasm_bindgen]
pub fn compress_webp(input: &[u8], target: Option<f64>, preview_pixels: Option<u32>) -> Result<CompressedWebp, JsError> {
    let result = pngdeer::compress_webp(input, &settings(target, preview_pixels)).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(CompressedWebp { report: result.json(), webp: result.webp })
}

#[wasm_bindgen]
pub struct CompressedJpeg {
    jpeg: Vec<u8>,
    report: String,
}

#[wasm_bindgen]
impl CompressedJpeg {
    /// The JPEG file.
    #[wasm_bindgen(getter)]
    pub fn jpeg(&self) -> Vec<u8> {
        self.jpeg.clone()
    }

    /// JSON: target, quality, score, tries, rounds, outcome, bytes, and a time breakdown.
    #[wasm_bindgen(getter)]
    pub fn report(&self) -> String {
        self.report.clone()
    }
}

/// Compresses one JPEG file to the lowest quality JPEG scoring at least `target` (pngdeer::compress_jpeg),
/// never bigger than the input; with no target, at a quality picked per image.
#[wasm_bindgen]
pub fn compress_jpeg(input: &[u8], target: Option<f64>) -> Result<CompressedJpeg, JsError> {
    let result = pngdeer::compress_jpeg(input, &settings(target, None)).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(CompressedJpeg { report: result.json(), jpeg: result.jpeg })
}
