//! pngdeer in the browser: pngdeer::compress compiled to WebAssembly, single-threaded or (feature
//! `threads`) with a rayon pool on Web Workers. Same output as the command line.

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

/// Compresses one PNG file to the smallest palette PNG scoring at least `target` (SSIMULACRA2), or
/// with no target at a quality picked per image (auto mode). Images over `preview_pixels` (default
/// pngdeer::PREVIEW_PIXELS, as on the command line) are searched on a downscaled preview and checked
/// at full size; 0 searches at full size, several times slower on big images.
#[wasm_bindgen]
pub fn compress(input: &[u8], target: Option<f64>, preview_pixels: Option<u32>) -> Result<Compressed, JsError> {
    let defaults = pngdeer::Settings::default();
    let settings = pngdeer::Settings {
        target,
        preview_pixels: preview_pixels.map_or(defaults.preview_pixels, |p| p as usize),
        clock: Some(js_sys::Date::now),
        ..defaults
    };
    let result = pngdeer::compress(input, &settings).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(Compressed { report: result.json(), png: result.png })
}
