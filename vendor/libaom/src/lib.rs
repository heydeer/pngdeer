//! libaom 3.15.2's AV1 encoder (libaom/, BSD-2-Clause with AOM's patent license) for pngdeer: a
//! still image's planes to AV1 data, and the frame as a decoder shows it. Built the same way on
//! every target (build.rs), so the command line and the browser build write the same bytes.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::fmt;
use std::sync::Once;

// the C library functions libaom calls, the same on every target
use pngdeer_clib as _;

/// AV1's limit on either side (16 bits in the sequence header).
pub const MAX_SIDE: usize = 65536;

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// libaom's tunings (aom_tune_metric) pngdeer uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tune {
    Psnr = 0,
    Ssim = 1,
    /// For still images: quantization matrices, chroma and perceptual delta-q, and more.
    Iq = 10,
    Ssimulacra2 = 11,
}

/// A sequence header's color description (ITU-T H.273 code points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cicp {
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
}

impl Cicp {
    /// sRGB, with BT.601's matrix: what libavif's encoder writes for an sRGB image.
    pub const SRGB: Cicp = Cicp { color_primaries: 1, transfer_characteristics: 13, matrix_coefficients: 6 };
}

/// How to encode: what libavif's quality, speed and codec options set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// libaom's cq-level, 0-63; 0 is lossless.
    pub quantizer: u8,
    /// libaom's cpu-used, 0-9.
    pub speed: u8,
    /// None for libaom's default (PSNR).
    pub tune: Option<Tune>,
    /// libaom's screen content tools (tune-content=screen).
    pub screen: bool,
    /// Tile rows and columns, log2 (0–6 each).
    pub tiles_log2: (u8, u8),
    /// Color planes only: the sequence header's color description (alpha's is unspecified).
    pub cicp: Cicp,
}

/// An encoded image: libaom's temporal unit (temporal delimiter, sequence header, frame), and the
/// frame as a decoder shows it, plane after plane.
pub struct Encoded {
    pub data: Vec<u8>,
    pub decoded: Vec<u16>,
}

#[repr(C)]
struct COptions {
    quantizer: c_int,
    speed: c_int,
    tune: c_int,
    screen: c_int,
    tile_rows_log2: c_int,
    tile_cols_log2: c_int,
    color_primaries: c_int,
    transfer_characteristics: c_int,
    matrix_coefficients: c_int,
}

extern "C" {
    fn pngdeer_aom_init();
    #[allow(clippy::too_many_arguments)]
    fn pngdeer_aom_encode(
        planes: *const *const u16,
        count: c_int,
        width: c_int,
        height: c_int,
        depth: c_int,
        options: *const COptions,
        out: *mut *mut u8,
        out_size: *mut usize,
        decoded: *mut u16,
        message: *mut c_char,
        message_size: usize,
    ) -> c_int;
    fn pngdeer_aom_free(pointer: *mut c_void);
}

/// Encodes one still image: `planes` holds 3 planes (Y, U and V, 4:4:4) or 1 (an alpha plane,
/// encoded as monochrome) of `height` rows of `width` samples of `depth` bits (8 or 10), as
/// libavif's encoder encodes a still AVIF's color or alpha (all-intra, one frame, full range).
/// Checks everything libaom would refuse. Any number can run at once, each on one thread.
pub fn encode(planes: &[&[u16]], width: usize, height: usize, depth: u8, options: &Options) -> Result<Encoded, Error> {
    if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
        return Err(Error(format!("AV1 can't hold {width} × {height} pixels: at most {MAX_SIDE} a side")));
    }
    if planes.len() != 1 && planes.len() != 3 {
        return Err(Error(format!("AV1 encoding takes 1 or 3 planes, not {}", planes.len())));
    }
    if depth != 8 && depth != 10 {
        return Err(Error(format!("no {depth}-bit AV1 encoding here: 8 or 10")));
    }
    if planes.iter().any(|p| p.len() != width * height) {
        return Err(Error("the planes don't match the size given".into()));
    }
    if planes.iter().any(|p| p.iter().any(|&s| s >> depth != 0)) {
        return Err(Error(format!("samples beyond {depth} bits")));
    }
    if options.quantizer > 63 || options.speed > 9 || options.tiles_log2.0 > 6 || options.tiles_log2.1 > 6 {
        return Err(Error(format!("no such AV1 quantizer or speed: {options:?}")));
    }
    static INIT: Once = Once::new();
    // SAFETY: sets up libaom's tables, once, before any encoder runs
    INIT.call_once(|| unsafe { pngdeer_aom_init() });
    let c = COptions {
        quantizer: options.quantizer as c_int,
        speed: options.speed as c_int,
        tune: options.tune.map_or(-1, |t| t as c_int),
        screen: options.screen as c_int,
        tile_rows_log2: options.tiles_log2.0 as c_int,
        tile_cols_log2: options.tiles_log2.1 as c_int,
        color_primaries: options.cicp.color_primaries as c_int,
        transfer_characteristics: options.cicp.transfer_characteristics as c_int,
        matrix_coefficients: options.cicp.matrix_coefficients as c_int,
    };
    let pointers: Vec<*const u16> = planes.iter().map(|p| p.as_ptr()).collect();
    let mut decoded = vec![0u16; planes.len() * width * height];
    let mut out: *mut u8 = std::ptr::null_mut();
    let mut size = 0usize;
    let mut message = [0 as c_char; 200];
    // SAFETY: the arguments are checked above; libaom reads the planes, writes `decoded` (as many
    // samples as the planes) and the message, and allocates the data with malloc, freed here with
    // its own free.
    unsafe {
        let ok = pngdeer_aom_encode(
            pointers.as_ptr(),
            planes.len() as c_int,
            width as c_int,
            height as c_int,
            depth as c_int,
            &c,
            &mut out,
            &mut size,
            decoded.as_mut_ptr(),
            message.as_mut_ptr(),
            message.len(),
        );
        if ok == 0 {
            return Err(Error(format!("AV1 encoding failed: {}", CStr::from_ptr(message.as_ptr()).to_string_lossy())));
        }
        let data = std::slice::from_raw_parts(out, size).to_vec();
        pngdeer_aom_free(out.cast());
        Ok(Encoded { data, decoded })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize) -> Vec<Vec<u16>> {
        let plane = |f: &dyn Fn(usize, usize) -> usize| (0..width * height).map(|i| (f(i % width, i / width) % 1024) as u16).collect();
        vec![plane(&|x, y| x * 1023 / width + y), plane(&|x, _| 300 + x), plane(&|_, y| 700 - y)]
    }

    fn options(quantizer: u8) -> Options {
        Options { quantizer, speed: 8, tune: Some(Tune::Iq), screen: false, tiles_log2: (0, 0), cicp: Cicp::SRGB }
    }

    #[test]
    fn encodes_smaller_as_the_quantizer_rises() {
        let planes = gradient(67, 45);
        let planes: Vec<&[u16]> = planes.iter().map(|p| p.as_slice()).collect();
        let sizes: Vec<usize> = [10, 30, 50]
            .iter()
            .map(|&q| {
                let encoded = encode(&planes, 67, 45, 10, &options(q)).unwrap();
                assert_eq!(encoded.decoded.len(), 3 * 67 * 45);
                encoded.data.len()
            })
            .collect();
        assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2], "{sizes:?}");
    }

    #[test]
    fn lossless_decodes_to_the_input() {
        let planes = gradient(33, 17);
        let refs: Vec<&[u16]> = planes.iter().map(|p| p.as_slice()).collect();
        let encoded = encode(&refs, 33, 17, 10, &Options { tune: None, ..options(0) }).unwrap();
        assert_eq!(encoded.decoded, planes.concat());
        let alpha = encode(&refs[..1], 33, 17, 10, &Options { tune: None, ..options(0) }).unwrap();
        assert_eq!(alpha.decoded, planes[0]);
    }

    #[test]
    fn refuses_what_libaom_would() {
        let plane = vec![0u16; 16];
        assert!(encode(&[&plane], 4, 4, 9, &options(30)).is_err());
        assert!(encode(&[&plane, &plane], 4, 4, 10, &options(30)).is_err());
        assert!(encode(&[&plane], 4, 5, 10, &options(30)).is_err());
        assert!(encode(&[&[1024; 16]], 4, 4, 10, &options(30)).is_err());
        assert!(encode(&[&plane], 4, 4, 10, &options(64)).is_err());
    }
}
