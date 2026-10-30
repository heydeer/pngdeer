//! JPEG output (`compress_jpeg`): a JPEG photo re-encoded at the lowest quality whose SSIMULACRA2
//! score, against the photo as it decodes, reaches the target.
//!
//! - Decoding: zune-jpeg without its SIMD code, so the pixels, and so the scores and the file, are
//!   the same on every target, the browser build's too. Its errors come back as errors (libjpeg's
//!   decoder ends on a longjmp, which the browser build can't make).
//! - Encoding: the vendored mozjpeg 4.1.5 (vendor/mozjpeg): progressive scans with trellis
//!   quantization, its quantization table 2 and 4:2:0 chroma. On 42 photos at TinyPNG's quality
//!   (experiments/jpeg/FINDINGS.md), mozjpeg's default table needs 5% more bytes, 4:4:4 15% more,
//!   and jpegli at 4:2:0 ties it, but is C++, which the browser build can't link.
//! - Metadata: the ICC profile unless it is sRGB's (what viewers assume anyway), and the EXIF
//!   orientation; the rest of EXIF (camera, place, time) is dropped, as TinyPNG drops all of it.
//! - Never bigger: if no quality reaches the target in fewer bytes than the input, the input.
//! - Automatic quality: about TinyPNG's JPEG quality, which follows the photo's size rather than
//!   its content: TinyPNG gives big photos (seen scaled down) a lower quality than small ones. On 60
//!   photos (experiments/jpeg/FINDINGS.md), its SSIMULACRA2 scores follow AUTO_AT_1MP − AUTO_SLOPE ×
//!   ln(megapixels) as closely as anything tried (a reference file's score, the input's bit rate),
//!   at most AUTO_CAP.

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::codec::Error;
use crate::par;
use crate::quality::{self, Tried};
use crate::search::{Scorer, AUTO_CAP};
use crate::{Settings, RGBA};

/// The automatic target at one megapixel, and how much lower it is per natural-log step in size:
/// 74.5 at 0.5 MP, 70.1 at 2 MP, 64.4 at 12 MP.
const AUTO_AT_1MP: f64 = 72.3;
const AUTO_SLOPE: f64 = 3.2;
/// Where the search aims first: the median quality automatic targets took on the 60 photos.
const FIRST_QUALITY: f64 = 64.0;
/// About how many points a file's score gains per quality step there.
const SLOPE: f64 = 0.6;
/// A round's files are scored at once while their scored pixels fit in this, one after another
/// beyond it: each scoring at once adds about 64 bytes a pixel at the peak (a 12 MP photo peaked at
/// 2.7 GB scoring three at once, 1.3 GB one at a time), and one at a time is about 25% slower.
const SCORED_AT_ONCE: usize = 8_000_000;

/// A JPEG photo, decoded.
pub struct Photo {
    /// The pixels as RGBA (gray ones as gray RGBA), for scoring.
    pub pixels: Vec<RGBA>,
    pub width: usize,
    pub height: usize,
    /// One channel rather than three.
    pub gray: bool,
    /// Its ICC profile, if it has one.
    pub icc: Option<Vec<u8>>,
    /// Its EXIF orientation (1 if none).
    pub orientation: u16,
}

/// Whether a file is a JPEG: its start-of-image marker and the next marker's.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

pub fn decode(bytes: &[u8]) -> Result<Photo, Error> {
    let options = DecoderOptions::default().set_max_width(mozjpeg::MAX_SIDE).set_max_height(mozjpeg::MAX_SIDE);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(unreadable)?;
    let gray = match decoder.input_colorspace() {
        Some(ColorSpace::Luma) => true,
        Some(ColorSpace::YCbCr | ColorSpace::RGB) => false,
        Some(ColorSpace::CMYK | ColorSpace::YCCK) => return Err("CMYK JPEGs aren't supported".into()),
        other => return Err(format!("JPEGs in {other:?} aren't supported").into()),
    };
    let icc = decoder.icc_profile();
    let orientation = decoder.exif().and_then(|exif| exif_orientation(exif)).unwrap_or(1);
    let options = decoder.options().jpeg_set_out_colorspace(if gray { ColorSpace::Luma } else { ColorSpace::RGB });
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let samples = decoder.decode().map_err(unreadable)?;
    let (width, height) = decoder.dimensions().ok_or("not a JPEG pngdeer can read")?;
    let pixels = if gray {
        samples.iter().map(|&g| RGBA::new(g, g, g, 255)).collect()
    } else {
        samples.as_chunks::<3>().0.iter().map(|p| RGBA::new(p[0], p[1], p[2], 255)).collect()
    };
    Ok(Photo { pixels, width, height, gray, icc, orientation })
}

fn unreadable(e: zune_jpeg::errors::DecodeErrors) -> Error {
    format!("not a JPEG pngdeer can read: {}", e.to_string().trim()).into()
}

/// A compressed JPEG and what the search found.
pub struct CompressedJpeg {
    pub jpeg: Vec<u8>,
    /// The target searched for (Settings::target, or the automatic one).
    pub target: f64,
    /// The quality the file was encoded at; None if the input itself came back.
    pub quality: Option<u32>,
    /// Its score against the input as decoded (100 for the input itself).
    pub score: f64,
    pub tries: usize,
    pub rounds: usize,
    /// "hit", "miss" (no quality reaches the target; the closest), or "original" (nothing smaller
    /// than the input reaches it, so the input).
    pub outcome: &'static str,
    /// Milliseconds: decoding and scoring the input, and the search (zero without Settings::clock).
    pub ms: (f64, f64),
}

impl CompressedJpeg {
    pub fn json(&self) -> String {
        let quality = self.quality.map_or("null".into(), |q| q.to_string());
        format!(
            r#"{{"format": "jpeg", "target": {:.2}, "quality": {quality}, "score": {:.3}, "tries": {}, "rounds": {}, "outcome": "{}", "ms": {{"decode": {:.0}, "search": {:.0}}}, "bytes": {}}}"#,
            self.target,
            self.score,
            self.tries,
            self.rounds,
            self.outcome,
            self.ms.0,
            self.ms.1,
            self.jpeg.len()
        )
    }
}

/// One file the search tried.
struct Attempt {
    quality: u32,
    file: Vec<u8>,
    score: f64,
}

impl Tried for Attempt {
    fn quality(&self) -> u32 {
        self.quality
    }
    fn score(&self) -> f64 {
        self.score
    }
    fn size(&self) -> usize {
        self.file.len()
    }
}

/// Compresses one JPEG file: the lowest quality that reaches the target (see the module docs).
pub fn compress_jpeg(input: &[u8], settings: &Settings) -> Result<CompressedJpeg, Error> {
    let tick = || settings.clock.map_or(0.0, |c| c());
    let t = tick();
    let photo = decode(input)?;
    let (w, h) = (photo.width, photo.height);
    let scorer = Scorer::new(&photo.pixels, w, h)?;
    let samples: Vec<u8> = if photo.gray { photo.pixels.iter().map(|p| p.r).collect() } else { photo.pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect() };
    let components = if photo.gray { 1 } else { 3 };
    let icc = photo.icc.as_deref().filter(|icc| !is_srgb(icc));
    let exif = (photo.orientation != 1).then(|| orientation_exif(photo.orientation));
    // A round's files are encoded and decoded at once (each on one thread), then scored as many at
    // once as fit in SCORED_AT_ONCE (scoring is parallel itself).
    let ours = mozjpeg::Options { quality: 75, quant_table: Some(2), full_chroma: false, progressive: true };
    let attempts = |qualities: &[u32]| -> Result<Vec<Attempt>, Error> {
        let files = par::map(qualities, |&q| -> Result<_, String> {
            let file =
                mozjpeg::encode(&samples, w, h, components, &mozjpeg::Options { quality: q as u8, ..ours }, icc, exif.as_deref()).map_err(|e| e.to_string())?;
            let pixels = decode(&file).map_err(|e| e.to_string())?.pixels;
            Ok((q, file, pixels))
        });
        let files = files.into_iter().collect::<Result<Vec<_>, _>>()?;
        let at_once = (SCORED_AT_ONCE / scorer.scored_pixels()).max(1);
        let scores: Vec<_> = files.chunks(at_once).flat_map(|some| par::map(some, |(_, _, pixels)| scorer.score(pixels).map_err(|e| e.to_string()))).collect();
        files.into_iter().zip(scores).map(|((quality, file, _), score)| Ok(Attempt { quality, file, score: score? })).collect()
    };
    let target = match settings.target {
        Some(target) => target,
        // libm: the same bits on every target
        None => (AUTO_AT_1MP - AUTO_SLOPE * libm::log((w * h) as f64 / 1e6)).min(AUTO_CAP),
    };
    let decode_ms = tick() - t;
    let t = tick();
    let found = quality::search(target, input.len(), FIRST_QUALITY, SLOPE, Vec::new(), attempts)?;
    let ms = (decode_ms, tick() - t);
    let (tries, rounds) = (found.tries, found.rounds);
    let compressed = |a: Attempt, outcome| CompressedJpeg { quality: Some(a.quality), score: a.score, jpeg: a.file, target, tries, rounds, outcome, ms };
    Ok(match (found.best, found.closest) {
        (Some(best), _) if best.file.len() < input.len() => compressed(best, "hit"),
        (None, Some(closest)) if closest.file.len() < input.len() => compressed(closest, "miss"),
        _ => CompressedJpeg { jpeg: input.to_vec(), target, quality: None, score: 100.0, tries, rounds, outcome: "original", ms },
    })
}

/// The orientation tag (0x0112) in IFD0 of EXIF data starting at its TIFF header.
fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    let big = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| tiff.get(at..at + 2).map(|b| if big { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) });
    let u32_at =
        |at: usize| tiff.get(at..at + 4).map(|b| if big { u32::from_be_bytes(b.try_into().unwrap()) } else { u32::from_le_bytes(b.try_into().unwrap()) });
    let ifd = u32_at(4)? as usize;
    (0..u16_at(ifd)? as usize)
        .map(|i| ifd + 2 + i * 12)
        .find(|&entry| u16_at(entry) == Some(0x0112))
        .and_then(|entry| u16_at(entry + 8))
        .filter(|o| (1..=8).contains(o))
}

/// An EXIF APP1 payload holding only an orientation.
fn orientation_exif(orientation: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec(); // big-endian TIFF, IFD0 right after
    exif.extend_from_slice(&1u16.to_be_bytes()); // one entry:
    exif.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]); // orientation, SHORT, one value
    exif.extend_from_slice(&orientation.to_be_bytes());
    exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // value padding, no next IFD
    exif
}

/// Whether an ICC profile is sRGB's: its description says so (the profile cameras, editors and
/// Wikimedia embed: "sRGB IEC61966-2.1" and the like). Viewers show an untagged JPEG as sRGB.
fn is_srgb(icc: &[u8]) -> bool {
    let u32_at = |at: usize| icc.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let Some(count) = u32_at(128) else { return false };
    (0..count.min(256) as usize).any(|i| {
        let at = 132 + i * 12;
        if icc.get(at..at + 4) != Some(b"desc") {
            return false;
        }
        let (Some(offset), Some(size)) = (u32_at(at + 4), u32_at(at + 8)) else { return false };
        let Some(tag) = icc.get(offset as usize..(offset as usize).saturating_add(size as usize)) else { return false };
        // 'desc' holds ASCII; 'mluc' (ICC v4) UTF-16: either way, look for the letters
        let ascii: Vec<u8> = tag.iter().copied().filter(|&b| b != 0).collect();
        ascii.windows(4).any(|w| w == b"sRGB")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_an_orientation() {
        let exif = orientation_exif(6);
        assert_eq!(exif_orientation(&exif[6..]), Some(6));
        // little-endian, two entries, orientation second
        let mut tiff = b"II\x2a\0\x08\0\0\0\x02\0".to_vec();
        tiff.extend_from_slice(&[0x0f, 0x01, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(exif_orientation(&tiff), Some(8));
        assert_eq!(exif_orientation(b"nope"), None);
    }

    #[test]
    fn tells_srgb_profiles() {
        let profile = |desc: &[u8]| {
            let mut icc = vec![0u8; 128];
            icc.extend_from_slice(&1u32.to_be_bytes());
            icc.extend_from_slice(b"desc");
            icc.extend_from_slice(&144u32.to_be_bytes());
            icc.extend_from_slice(&(desc.len() as u32).to_be_bytes());
            icc.extend_from_slice(desc);
            icc
        };
        assert!(is_srgb(&profile(b"desc\0\0\0\0\0\0\0\x12sRGB IEC61966-2.1\0")));
        assert!(!is_srgb(&profile(b"desc\0\0\0\0\0\0\0\x0aAdobe RGB\0")));
        assert!(!is_srgb(b"short"));
    }
}
