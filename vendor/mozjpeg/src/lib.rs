//! mozjpeg 4.1.5's encoder (mozjpeg/, IJG and BSD-3-Clause) for pngdeer: RGB or gray pixels to a
//! JPEG file. Built the same way on every target (build.rs), so the command line and the browser
//! build write the same bytes. Decoding is pngdeer's (a pure Rust decoder): libjpeg ends on corrupt
//! input through a longjmp, which the browser build can't make.

use std::ffi::{c_int, c_uchar, c_void};
use std::fmt;

// the C library functions mozjpeg calls, the same on every target
use pngdeer_clib as _;

/// libjpeg's limit on either side.
pub const MAX_SIDE: usize = 65500;

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// How to encode: what cjpeg's -quality, -quant-table, -sample 1x1 and -baseline set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// 1-100.
    pub quality: u8,
    /// mozjpeg's base quantization table (0-8), None for its default (3).
    pub quant_table: Option<u8>,
    /// Full-resolution chroma (4:4:4) instead of 4:2:0.
    pub full_chroma: bool,
    /// mozjpeg's progressive scans, chosen per image; false for one baseline scan.
    pub progressive: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { quality: 75, quant_table: None, full_chroma: false, progressive: true }
    }
}

#[repr(C)]
struct COptions {
    quality: c_int,
    quant_table: c_int,
    subsampling: c_int,
    progressive: c_int,
}

extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn pngdeer_jpeg_encode(
        pixels: *const c_uchar,
        width: c_int,
        height: c_int,
        components: c_int,
        options: *const COptions,
        icc: *const c_uchar,
        icc_size: usize,
        exif: *const c_uchar,
        exif_size: usize,
        out: *mut *mut c_uchar,
    ) -> usize;
    fn pngdeer_jpeg_free(pointer: *mut c_void);
}

/// A JPEG file of `pixels`: `height` rows of `width` pixels of `components` bytes (1: gray, 3: RGB),
/// with `icc` (an ICC profile, in APP2 markers) and `exif` (an APP1 payload, "Exif\0\0" first) if
/// given. Checks everything libjpeg would end on.
pub fn encode(
    pixels: &[u8],
    width: usize,
    height: usize,
    components: usize,
    options: &Options,
    icc: Option<&[u8]>,
    exif: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
        return Err(Error(format!("JPEG can't hold {width} × {height} pixels: at most {MAX_SIDE} a side")));
    }
    if components != 1 && components != 3 {
        return Err(Error(format!("JPEG encoding takes gray or RGB pixels, not {components} components")));
    }
    if pixels.len() != width * height * components {
        return Err(Error("the pixels don't match the size given".into()));
    }
    if !(1..=100).contains(&options.quality) || options.quant_table.is_some_and(|t| t > 8) {
        return Err(Error(format!("no such JPEG quality or table: {options:?}")));
    }
    let icc = icc.unwrap_or_default();
    let exif = exif.unwrap_or_default();
    // an APP2 marker holds 65519 bytes of profile, and there can be 255 of them; APP1 holds 65533
    if icc.len() > 255 * 65519 || exif.len() > 65533 {
        return Err(Error("too much metadata for JPEG markers".into()));
    }
    let c = COptions {
        quality: options.quality as c_int,
        quant_table: options.quant_table.map_or(-1, |t| t as c_int),
        subsampling: options.full_chroma as c_int,
        progressive: options.progressive as c_int,
    };
    let mut out: *mut c_uchar = std::ptr::null_mut();
    // SAFETY: the arguments are checked above; mozjpeg reads `pixels` and the metadata, and
    // allocates the file with malloc, freed here with its own free.
    unsafe {
        let size = pngdeer_jpeg_encode(
            pixels.as_ptr(),
            width as c_int,
            height as c_int,
            components as c_int,
            &c,
            icc.as_ptr(),
            icc.len(),
            exif.as_ptr(),
            exif.len(),
            &mut out,
        );
        let file = std::slice::from_raw_parts(out, size).to_vec();
        pngdeer_jpeg_free(out.cast());
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize) -> Vec<u8> {
        (0..width * height).flat_map(|i| [(i % width * 255 / width) as u8, (i / width * 255 / height) as u8, ((i % width + i / width) % 256) as u8]).collect()
    }

    #[test]
    fn writes_a_jpeg_that_shrinks_with_quality() {
        let rgb = gradient(97, 61);
        let sizes: Vec<usize> = [20, 60, 95]
            .iter()
            .map(|&quality| {
                let file = encode(&rgb, 97, 61, 3, &Options { quality, ..Default::default() }, None, None).unwrap();
                assert!(file.starts_with(&[0xff, 0xd8]) && file.ends_with(&[0xff, 0xd9]));
                file.len()
            })
            .collect();
        assert!(sizes[0] < sizes[1] && sizes[1] < sizes[2], "{sizes:?}");
    }

    #[test]
    fn carries_metadata() {
        let exif = b"Exif\0\0MM\0*\0\0\0\x08\0\0";
        let icc = vec![7u8; 70_000]; // two APP2 markers
        let file = encode(&gradient(16, 16), 16, 16, 3, &Options::default(), Some(&icc), Some(exif)).unwrap();
        let find = |needle: &[u8]| file.windows(needle.len()).filter(|w| *w == needle).count();
        assert_eq!((find(b"Exif\0\0MM"), find(b"ICC_PROFILE\0")), (1, 2));
    }

    #[test]
    fn gray_and_settings() {
        let gray: Vec<u8> = (0..40 * 30).map(|i| (i * 7 % 256) as u8).collect();
        assert!(encode(&gray, 40, 30, 1, &Options::default(), None, None).is_ok());
        let rgb = gradient(40, 30);
        for options in [Options { quant_table: Some(2), ..Default::default() }, Options { full_chroma: true, progressive: false, ..Default::default() }] {
            assert!(encode(&rgb, 40, 30, 3, &options, None, None).is_ok());
        }
    }

    #[test]
    fn refuses_what_libjpeg_would_end_on() {
        assert!(encode(&[], 0, 1, 3, &Options::default(), None, None).is_err());
        assert!(encode(&[0; 12], 2, 2, 3, &Options { quality: 0, ..Default::default() }, None, None).is_err());
        assert!(encode(&[0; 12], 2, 2, 4, &Options::default(), None, None).is_err());
        assert!(encode(&[0; 11], 2, 2, 3, &Options::default(), None, None).is_err());
    }
}
