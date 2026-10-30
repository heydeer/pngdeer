//! libwebp 1.6.0 (libwebp/, BSD-3-Clause) for pngdeer: RGBA to lossy or lossless WebP, and WebP
//! back to RGBA as browsers decode it. Built the same way on every target (build.rs), so the
//! command line and the browser build write the same bytes; the C library functions that would
//! differ between targets are here, below.

use std::ffi::{c_int, c_void};

// the C library functions libwebp calls, the same on every target
use pngdeer_clib as _;
use std::fmt;

/// Lossy (VP8) settings; see libwebp's WebPConfig.
#[derive(Clone, Copy, Debug)]
pub struct Lossy {
    /// 0-100
    pub quality: f32,
    /// 0 (fast) to 6 (smallest)
    pub method: u8,
    /// The slower, sharper RGB to YUV conversion (cwebp -sharp_yuv)
    pub sharp_yuv: bool,
    /// 0-100; 100 keeps the alpha plane lossless
    pub alpha_quality: u8,
    /// 0 (none), 1 (fast) or 2 (best) prediction filtering of the alpha plane
    pub alpha_filtering: u8,
    /// Compress the alpha plane (losslessly at alpha_quality 100); without, it is stored as it is.
    /// Either way the VP8 data is the same, which makes a quick stand-in for a search: at method 6
    /// and alpha_quality 100 libwebp compresses the alpha plane as hard as cwebp -z 9 does a file.
    pub alpha_compression: bool,
    /// libwebp's finer settings; None keeps its default.
    pub tuning: Tuning,
}

/// WebPConfig's finer lossy settings (cwebp's -segments, -sns, -f, -sharpness, -strong/-nostrong,
/// -af, -pre); None keeps libwebp's default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tuning {
    pub segments: Option<u8>,
    pub sns_strength: Option<u8>,
    pub filter_strength: Option<u8>,
    pub filter_sharpness: Option<u8>,
    pub filter_type: Option<u8>,
    pub autofilter: Option<bool>,
    pub preprocessing: Option<u8>,
}

/// Why an encode or a decode failed.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// An image decoded from WebP.
pub struct Decoded {
    /// RGBA, `width` * 4 bytes a row
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Mirrors PngdeerWebpOptions in glue/pngdeer_webp.c.
#[repr(C)]
struct Options {
    lossless: c_int,
    level: c_int,
    quality: f32,
    method: c_int,
    sharp_yuv: c_int,
    alpha_quality: c_int,
    alpha_filtering: c_int,
    alpha_compression: c_int,
    exact: c_int,
    segments: c_int,
    sns_strength: c_int,
    filter_strength: c_int,
    filter_sharpness: c_int,
    filter_type: c_int,
    autofilter: c_int,
    preprocessing: c_int,
    near_lossless: c_int,
}

/// A setting for the glue: libwebp's default is -1 (glue/pngdeer_webp.c).
fn or_default<T: Into<c_int>>(value: Option<T>) -> c_int {
    value.map_or(-1, Into::into)
}

extern "C" {
    fn pngdeer_webp_source_new(rgba: *const u8, width: c_int, height: c_int, sharp_yuv: c_int, error: *mut c_int) -> *mut c_void;
    fn pngdeer_webp_source_encode(source: *const c_void, options: *const Options, out: *mut *mut u8, error: *mut c_int) -> usize;
    fn pngdeer_webp_source_free(source: *mut c_void);
    fn pngdeer_webp_encode(rgba: *const u8, width: c_int, height: c_int, options: *const Options, out: *mut *mut u8, error: *mut c_int) -> usize;
    fn pngdeer_webp_decode(data: *const u8, size: usize, width: *mut c_int, height: *mut c_int) -> *mut u8;
    fn pngdeer_webp_free(pointer: *mut c_void);
}

/// RGBA (`width` * 4 bytes a row) as lossy WebP: VP8, and a separate alpha plane if any pixel is
/// translucent. Transparent pixels' colors may change (libwebp smooths them to save bytes).
pub fn encode_lossy(rgba: &[u8], width: usize, height: usize, settings: &Lossy) -> Result<Vec<u8>, Error> {
    warm_up();
    encode(rgba, width, height, &lossy_options(settings))
}

/// An image converted to YUV once (with sharp YUV or without), for any number of lossy encodes,
/// from any number of threads at once: the same files as `encode_lossy`'s, without converting the
/// image again for each.
pub struct Source {
    picture: *mut c_void,
    sharp_yuv: bool,
}

// SAFETY: encoding only reads the source's picture (WebPPictureCopy of a const picture).
unsafe impl Send for Source {}
unsafe impl Sync for Source {}

impl Source {
    pub fn new(rgba: &[u8], width: usize, height: usize, sharp_yuv: bool) -> Result<Self, Error> {
        check(rgba, width, height)?;
        warm_up();
        let mut error = 0;
        // SAFETY: rgba holds width * height * 4 bytes (checked)
        let picture = unsafe { pngdeer_webp_source_new(rgba.as_ptr(), width as c_int, height as c_int, sharp_yuv as c_int, &mut error) };
        if picture.is_null() {
            return Err(Error(encode_error(error)));
        }
        Ok(Source { picture, sharp_yuv })
    }

    /// The image as lossy WebP (`settings.sharp_yuv` must be the source's; preprocessing stays off).
    pub fn encode(&self, settings: &Lossy) -> Result<Vec<u8>, Error> {
        if settings.sharp_yuv != self.sharp_yuv || settings.tuning.preprocessing.is_some_and(|p| p != 0) {
            return Err(Error("settings that need another YUV conversion than the source's".into()));
        }
        let (mut out, mut error) = (std::ptr::null_mut(), 0);
        // SAFETY: the picture is alive while self is; `out` as in `encode`
        unsafe {
            let size = pngdeer_webp_source_encode(self.picture, &lossy_options(settings), &mut out, &mut error);
            take(out, size, error)
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: allocated by pngdeer_webp_source_new, freed once
        unsafe { pngdeer_webp_source_free(self.picture) }
    }
}

/// libwebp, built without threads, sets up its function tables on first use without a lock: one
/// encode and decode of each kind before any could run on several threads at once.
fn warm_up() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let rgba: Vec<u8> = (0..16 * 16 * 4).map(|i| (i * 37 % 256) as u8).collect();
        let lossy = Lossy {
            quality: 50.0,
            method: 6,
            sharp_yuv: true,
            alpha_quality: 100,
            alpha_filtering: 1,
            alpha_compression: true,
            tuning: Tuning { autofilter: Some(true), ..Tuning::default() },
        };
        for settings in [lossy, Lossy { sharp_yuv: false, tuning: Tuning::default(), ..lossy }] {
            if let Ok(file) = encode(&rgba, 16, 16, &lossy_options(&settings)) {
                let _ = decode_any(&file);
            }
        }
        if let Ok(file) = encode(&rgba, 16, 16, &lossless_options(6, 100)) {
            let _ = decode_any(&file);
        }
    });
}

fn lossy_options(settings: &Lossy) -> Options {
    Options {
        lossless: 0,
        level: 0,
        quality: settings.quality,
        method: settings.method as c_int,
        sharp_yuv: settings.sharp_yuv as c_int,
        alpha_quality: settings.alpha_quality as c_int,
        alpha_filtering: settings.alpha_filtering as c_int,
        alpha_compression: settings.alpha_compression as c_int,
        exact: 0,
        segments: or_default(settings.tuning.segments),
        sns_strength: or_default(settings.tuning.sns_strength),
        filter_strength: or_default(settings.tuning.filter_strength),
        filter_sharpness: or_default(settings.tuning.filter_sharpness),
        filter_type: or_default(settings.tuning.filter_type),
        autofilter: or_default(settings.tuning.autofilter),
        preprocessing: or_default(settings.tuning.preprocessing),
        near_lossless: -1,
    }
}

/// RGBA (`width` * 4 bytes a row) as lossless WebP (VP8L) at a cwebp -z `level` (0-9), every pixel
/// kept exactly, transparent ones' colors included.
pub fn encode_lossless(rgba: &[u8], width: usize, height: usize, level: u8) -> Result<Vec<u8>, Error> {
    encode_near_lossless(rgba, width, height, level, 100)
}

/// Lossless WebP of the pixels after libwebp's near-lossless preprocessing (cwebp -near_lossless:
/// 100 changes nothing; lower values move pixel values further, away from smooth areas).
pub fn encode_near_lossless(rgba: &[u8], width: usize, height: usize, level: u8, near_lossless: u8) -> Result<Vec<u8>, Error> {
    warm_up();
    encode(rgba, width, height, &lossless_options(level, near_lossless))
}

fn lossless_options(level: u8, near_lossless: u8) -> Options {
    Options {
        lossless: 1,
        level: level as c_int,
        quality: 0.0,
        method: 0,
        sharp_yuv: 0,
        alpha_quality: 100,
        alpha_filtering: 0,
        alpha_compression: 1,
        exact: 1,
        segments: -1,
        sns_strength: -1,
        filter_strength: -1,
        filter_sharpness: -1,
        filter_type: -1,
        autofilter: -1,
        preprocessing: -1,
        near_lossless: near_lossless as c_int,
    }
}

fn check(rgba: &[u8], width: usize, height: usize) -> Result<(), Error> {
    if rgba.len() != width * height * 4 || width == 0 || height == 0 {
        return Err(Error(format!("{} bytes of RGBA for {width}×{height} pixels", rgba.len())));
    }
    if width > 16383 || height > 16383 {
        return Err(Error(format!("{width}×{height} pixels: WebP is limited to 16383 pixels a side")));
    }
    Ok(())
}

fn encode(rgba: &[u8], width: usize, height: usize, options: &Options) -> Result<Vec<u8>, Error> {
    check(rgba, width, height)?;
    let (mut out, mut error) = (std::ptr::null_mut(), 0);
    // SAFETY: rgba holds width * height * 4 bytes (checked above)
    unsafe {
        let size = pngdeer_webp_encode(rgba.as_ptr(), width as c_int, height as c_int, options, &mut out, &mut error);
        take(out, size, error)
    }
}

/// The file libwebp encoded into its own allocation of `size` bytes, copied and then freed with its
/// own free.
///
/// # Safety
/// `out` is null or libwebp's allocation of `size` bytes.
unsafe fn take(out: *mut u8, size: usize, error: c_int) -> Result<Vec<u8>, Error> {
    if size == 0 || out.is_null() {
        return Err(Error(encode_error(error)));
    }
    let file = std::slice::from_raw_parts(out, size).to_vec();
    pngdeer_webp_free(out.cast());
    Ok(file)
}

fn encode_error(code: c_int) -> String {
    let reason = match code {
        -1 => "invalid settings",
        1 => "out of memory",
        2 => "out of memory while writing",
        3 => "a null parameter",
        4 => "invalid configuration",
        5 => "bad dimensions",
        6 => "partition 0 overflow",
        7 => "partition overflow",
        8 => "write error",
        9 => "file too big",
        10 => "user abort",
        _ => "unknown error",
    };
    format!("WebP encoding failed: {reason} ({code})")
}

/// A WebP file as RGBA, with libwebp's fancy upsampling of the color planes, as browsers show it.
pub fn decode(webp: &[u8]) -> Result<Decoded, Error> {
    warm_up();
    decode_any(webp)
}

fn decode_any(webp: &[u8]) -> Result<Decoded, Error> {
    let (mut width, mut height) = (0, 0);
    // SAFETY: libwebp reads `webp.len()` bytes; the result is width * height * 4 bytes of its
    // allocation, copied and then freed with its own free.
    unsafe {
        let pixels = pngdeer_webp_decode(webp.as_ptr(), webp.len(), &mut width, &mut height);
        if pixels.is_null() {
            return Err(Error("not a WebP file libwebp can decode".into()));
        }
        let (width, height) = (width as usize, height as usize);
        let rgba = std::slice::from_raw_parts(pixels, width * height * 4).to_vec();
        pngdeer_webp_free(pixels.cast());
        Ok(Decoded { rgba, width, height })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize, alpha: bool) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [(x * 255 / width) as u8, (y * 255 / height) as u8, ((x + y) % 256) as u8, if alpha { (x * 7 % 256) as u8 } else { 255 }]
            })
            .collect()
    }

    #[test]
    fn lossless_round_trips() {
        let rgba = gradient(37, 23, true);
        let file = encode_lossless(&rgba, 37, 23, 6).unwrap();
        assert_eq!(&file[..4], b"RIFF");
        let back = decode(&file).unwrap();
        assert_eq!((back.width, back.height), (37, 23));
        assert_eq!(back.rgba, rgba);
    }

    #[test]
    fn lossy_shrinks_with_quality() {
        let rgba = gradient(64, 48, false);
        let settings =
            |quality| Lossy { quality, method: 4, sharp_yuv: true, alpha_quality: 100, alpha_filtering: 1, alpha_compression: true, tuning: Tuning::default() };
        let low = encode_lossy(&rgba, 64, 48, &settings(20.0)).unwrap();
        let high = encode_lossy(&rgba, 64, 48, &settings(95.0)).unwrap();
        assert!(low.len() < high.len());
        let back = decode(&high).unwrap();
        assert_eq!((back.width, back.height, back.rgba.len()), (64, 48, 64 * 48 * 4));
    }

    #[test]
    fn a_source_encodes_as_encode_lossy_does() {
        let rgba = gradient(61, 37, true);
        for sharp_yuv in [true, false] {
            let source = Source::new(&rgba, 61, 37, sharp_yuv).unwrap();
            for quality in [5.0, 60.0, 97.0] {
                let settings = Lossy {
                    quality,
                    method: 6,
                    sharp_yuv,
                    alpha_quality: 100,
                    alpha_filtering: 1,
                    alpha_compression: true,
                    tuning: Tuning { autofilter: Some(true), ..Tuning::default() },
                };
                assert_eq!(source.encode(&settings).unwrap(), encode_lossy(&rgba, 61, 37, &settings).unwrap());
            }
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"RIFF\0\0\0\0WEBPnope").is_err());
        assert!(encode_lossless(&[0; 12], 2, 2, 6).is_err());
    }
}
