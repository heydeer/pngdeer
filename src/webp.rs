//! WebP output: per image, the smaller of two WebP files that keep a quality target.
//!
//! The target is `--target`'s, or automatically TinyPNG's WebP quality for the image (round 3 of
//! experiments/webp/FINDINGS.md). TinyPNG's WebP is its palette PNG stored losslessly, or where that
//! is bigger, libwebp at about quality 70: so pngdeer's PNG (whose automatic target is TinyPNG's PNG
//! quality) is compared with a lossy file at quality 70 (WebpOptions::reference_quality, libwebp's
//! default settings), and where that is smaller, its score becomes the target (at most 90, as for
//! PNG) and the PNG search runs again for it where its palette might win (PALETTE_REACH).
//!
//! - Lossless: the PNG's own pixels (the palette image, or the source where the PNG kept it) as
//!   lossless WebP. Same pixels, same score; libwebp's palette coding beats oxipng's PNG by 2–31%.
//! - Lossy: the lowest libwebp quality (0–100) whose file scores at least the target, as the
//!   search scores, at libwebp's slowest method with sharp YUV and its auto filter (round 4), or
//!   the quality-70 file where that is smaller. It wins big on photos, cut-outs, 3D renders and 2×
//!   screenshots, and loses on flat icons and 1× UI, where it can need more than twice the bytes or
//!   never get there.
//!
//! Colors: WebP carries color information only as an ICC profile. An iCCP chunk becomes the file's
//! ICCP, an image in sRGB needs none, and the rest (cICP, or gAMA and cHRM that aren't sRGB's) is
//! refused rather than shown in the wrong colors. Animated PNGs are refused too.

use rgb::{ComponentBytes, FromSlice};

use crate::codec::{self, Error};
use crate::par;
use crate::quality::{self, Found, Tried};
use crate::search::{Scorer, AUTO_CAP};
use crate::{Compressed, Settings, WebpOptions, RGBA};

/// The first guess at the quality that reaches the target, without a reference to go by: targets
/// from TinyPNG's PNG quality mostly land between 80 and 95.
const FIRST_QUALITY: f64 = 90.0;
/// About how many points a lossy file's score gains per quality step there, to aim from the one
/// side of the target known so far.
const SLOPE: f64 = 0.5;
/// Above this many pixels the PNG searches and the lossy work take turns rather than run alongside,
/// for memory: wasm32 has 4 GB, and each one's scoring holds much of it at these sizes. (A round's
/// files are encoded at once at any size: a few bytes a pixel each, and never during a scoring.)
const PARALLEL_PIXELS: usize = 8_000_000;
/// Lossless levels (cwebp -z) every image gets, the smaller file kept: on the benchmark each wins
/// about half the time, and between them they come within 1.6% of level 9 on the median image (3%
/// in total), which takes about 20 times as long: 15 s on a 5 MP screenshot.
const FAST_LEVELS: [u8; 2] = [3, 6];
/// Automatic quality: the PNG search runs again at the reference's score only where the palette
/// might win there, its lossless file at the PNG's target at most this many times the reference
/// lossy file. On the benchmark it won up to 2.5 times (16 of 113 images, 5–44% smaller than lossy
/// WebP), and the search costs as much again as the first.
const PALETTE_REACH: f64 = 3.0;
/// What a slow level (WebpOptions::slow_lossless_level) saves at most over FAST_LEVELS' best,
/// about: it runs only where the lossy file isn't smaller by more than this.
const SLOW_GAIN: f64 = 0.10;

pub struct CompressedWebp {
    pub webp: Vec<u8>,
    /// The score the file had to reach (see the module).
    pub target: f64,
    /// The PNG whose pixels made the lossless candidate (`compress`'s, at `target` where the PNG
    /// search ran for it).
    pub png: Compressed,
    /// "lossy" or "lossless"
    pub mode: &'static str,
    /// The lossy file's quality (also when the lossless one won): the lowest that reaches the
    /// target, or where none does, the best-scoring one tried.
    pub quality: Option<u32>,
    pub score: f64,
    pub lossless_bytes: usize,
    /// The level of the lossless candidate kept.
    pub lossless_level: u8,
    pub lossy_bytes: Option<usize>,
    /// Lossy files encoded and scored, and in how many rounds.
    pub tries: usize,
    pub rounds: usize,
    /// Automatic quality: the reference lossy file's quality, score and size, and the lossless
    /// file's size at the PNG's target it was weighed against (see the module).
    pub reference: Option<(u32, f64, usize, usize)>,
    pub profile: Profile,
}

/// Where the time went, in milliseconds (zeros when Settings::clock is None). The lossy search runs
/// alongside the PNG search for the reference's target, when there is one: `lossy` is them both.
#[derive(Default)]
pub struct Profile {
    pub png: f64,
    pub lossless: f64,
    pub reference: f64,
    pub lossy: f64,
}

impl CompressedWebp {
    pub fn json(&self) -> String {
        let or_null = |v: Option<String>| v.unwrap_or_else(|| "null".into());
        let p = &self.profile;
        format!(
            r#"{{"format": "webp", "target": {:.2}, "mode": "{}", "quality": {}, "score": {:.3}, "tries": {}, "rounds": {}, "lossless_bytes": {}, "lossless_level": {}, "lossy_bytes": {}, "reference": {}, "ms": {{"png": {:.0}, "lossless": {:.0}, "reference": {:.0}, "lossy": {:.0}}}, "png": {}, "bytes": {}}}"#,
            self.target,
            self.mode,
            or_null(self.quality.map(|q| q.to_string())),
            self.score,
            self.tries,
            self.rounds,
            self.lossless_bytes,
            self.lossless_level,
            or_null(self.lossy_bytes.map(|b| b.to_string())),
            or_null(self.reference.map(|(q, s, b, l)| format!(r#"{{"quality": {q}, "score": {s:.3}, "bytes": {b}, "lossless_bytes": {l}}}"#))),
            p.png,
            p.lossless,
            p.reference,
            p.lossy,
            self.png.json(),
            self.webp.len()
        )
    }
}

/// Compresses one PNG file to WebP: `compress` first, for a target and its pixels (twice where the
/// automatic target moves, see the module), then the lossless and lossy candidates; the smaller one
/// that reaches the target wins.
pub fn compress_webp(input: &[u8], settings: &Settings) -> Result<CompressedWebp, Error> {
    let icc = icc_profile(input)?;
    let bitmap = codec::decode(input)?;
    let (w, h) = (bitmap.width, bitmap.height);
    let tick = || settings.clock.map_or(0.0, |c| c());
    let options = settings.webp;
    let mut profile = Profile::default();
    let t = tick();
    // the PNG search, and alongside it the lossy files' YUV and compressed alpha plane (on images up
    // to PARALLEL_PIXELS: memory)
    let png_search = || crate::compress_decoded(input, &bitmap, settings).map_err(|e| e.to_string());
    let alpha_effort = if w * h > PARALLEL_PIXELS { options.large_alpha_effort } else { options.method };
    let lossy_source = || Lossies::new(&bitmap.pixels, w, h, options, alpha_effort).map_err(|e| e.to_string());
    let (png, lossies) = if w * h <= PARALLEL_PIXELS { par::join(png_search, lossy_source) } else { (png_search(), lossy_source()) };
    let (mut png, lossies) = (png?, lossies?);
    profile.png = tick() - t;

    let t = tick();
    let mut lossless = Lossless::of(&png, w, h)?;
    profile.lossless = tick() - t;

    let t = tick();
    let (mut known, mut reference, mut target, mut estimate) = (Vec::new(), None, png.report.target, FIRST_QUALITY);
    let (mut fallback, mut again) = (None, false);
    if let (None, Some(quality)) = (settings.target, options.reference_quality) {
        // TinyPNG's file: libwebp's own settings, whatever the search's
        let probe = lossies.tries(&[quality as u32], libwebp::Tuning::default())?.remove(0);
        reference = Some((probe.quality, probe.score, probe.size, lossless.file.len()));
        if probe.size < lossless.file.len() {
            // TinyPNG would ship the lossy file: its quality is the target, for a palette too
            target = probe.score.min(AUTO_CAP);
            again = lossless.file.len() as f64 <= probe.size as f64 * PALETTE_REACH;
        }
        // the search starts there, from this very file if its settings are the search's, and
        // otherwise keeps it in reserve: the search's settings don't win on every image
        estimate = probe.quality as f64;
        if probe.tuning == tuning(&options) {
            known.push(probe);
        } else if probe.score >= target {
            fallback = Some(probe);
        }
    }
    profile.reference = tick() - t;

    let t = tick();
    // where the palette misses the target, no size can rule lossy out; the palette made again for
    // a lower target (alongside) is smaller still, so this one's size bounds the search safely
    let limit = if lossless.score >= target { lossless.file.len() } else { usize::MAX };
    let png_search = || {
        again.then(|| crate::compress_decoded(input, &bitmap, &Settings { target: Some(target), ..settings.clone() }).map_err(|e| e.to_string())).transpose()
    };
    let lossy_search = || lossies.search(target, limit, estimate, known, tuning(&options)).map_err(|e| e.to_string());
    let (png_again, found) = if w * h <= PARALLEL_PIXELS { par::join(png_search, lossy_search) } else { (png_search(), lossy_search()) };
    let found = found?;
    if let Some(again) = png_again? {
        png = again;
        lossless = Lossless::of(&png, w, h)?;
    }
    // the lossless file falls short where the PNG search missed the target (or ran for a higher
    // one); if no lossy file reaches it either, the closest one does better than the palette
    let lossy = found.best.or(found.closest.filter(|c| lossless.score < target && c.score > lossless.score));
    // the alpha plane is the same in both, so their VP8 data decides
    let lossy = match (lossy, fallback) {
        (Some(l), Some(f)) if f.vp8_size() < l.vp8_size() || l.score < target => Some(f),
        (None, f) => f,
        (l, _) => l,
    };
    let lossy = lossy.map(|l| lossies.finished(l));
    profile.lossy = tick() - t;

    let t = tick();
    let close = lossy.as_ref().is_none_or(|l| l.file.len() as f64 > lossless.file.len() as f64 * (1.0 - SLOW_GAIN));
    if let (true, Some(level)) = (close, options.slow_lossless_level) {
        lossless.try_level(&png, w, h, level)?;
    }
    profile.lossless += tick() - t;

    let (quality, lossy_bytes, lossless_bytes, lossless_level) =
        (lossy.as_ref().map(|l| l.quality), lossy.as_ref().map(|l| l.file.len()), lossless.file.len(), lossless.level);
    let (file, mode, score) = match lossy {
        Some(l) if l.file.len() < lossless.file.len() || lossless.score < target => (l.file, "lossy", l.score),
        _ => (lossless.file, "lossless", lossless.score),
    };
    let webp = match icc {
        Some(icc) => with_icc(&file, &icc, w, h),
        None => file,
    };
    Ok(CompressedWebp {
        webp,
        target,
        png,
        mode,
        quality,
        score,
        lossless_bytes,
        lossless_level,
        lossy_bytes,
        tries: found.tries,
        rounds: found.rounds,
        reference,
        profile,
    })
}

/// The lossless candidate: a PNG's pixels as lossless WebP.
struct Lossless {
    file: Vec<u8>,
    level: u8,
    /// The PNG's pixels are the source's, unchanged, unless it is a palette the search picked.
    score: f64,
}

impl Lossless {
    fn of(png: &Compressed, w: usize, h: usize) -> Result<Self, Error> {
        let score = if matches!(png.outcome, "hit" | "miss") { png.report.score } else { 100.0 };
        let mut lossless = Lossless { file: Vec::new(), level: 0, score };
        for level in FAST_LEVELS {
            lossless.try_level(png, w, h, level)?;
        }
        Ok(lossless)
    }

    /// Keeps the file at this level if it is smaller.
    fn try_level(&mut self, png: &Compressed, w: usize, h: usize, level: u8) -> Result<(), Error> {
        let pixels = codec::decode(&png.png)?.pixels;
        let file = libwebp::encode_lossless(pixels.as_bytes(), w, h, level)?;
        if self.file.is_empty() || file.len() < self.file.len() {
            (self.file, self.level) = (file, level);
        }
        Ok(())
    }
}

/// A WebP file as RGBA, decoded as browsers decode it.
pub fn decode(webp: &[u8]) -> Result<codec::Bitmap, Error> {
    let image = libwebp::decode(webp)?;
    Ok(codec::Bitmap { pixels: image.rgba.as_rgba().to_vec(), width: image.width, height: image.height })
}

/// A lossy file, scored.
struct Lossy {
    quality: u32,
    file: Vec<u8>,
    score: f64,
    /// The file stores its alpha plane uncompressed (a search candidate, see `Lossies`).
    raw_alpha: bool,
    /// The file's size as it ships, with the alpha plane compressed.
    size: usize,
    /// The finer settings it was encoded with.
    tuning: libwebp::Tuning,
}

impl Lossy {
    /// The file's size without its alpha plane, compressed or not.
    fn vp8_size(&self) -> usize {
        self.file.len() - chunk_size(&self.file, b"ALPH")
    }
}

/// The search's finer lossy settings.
fn tuning(options: &WebpOptions) -> libwebp::Tuning {
    libwebp::Tuning { autofilter: options.autofilter.then_some(true), ..Default::default() }
}

/// What a lossy search found: the file at the lowest quality reaching the target, the best-scoring
/// file of those falling short, and how many files it encoded, in how many rounds.
/// Lossy files of one image, with its YUV converted once, and scored as the PNG search scores.
///
/// With translucent pixels, candidates store their alpha plane uncompressed: compressing it is much of
/// an encode's work (at method 6 libwebp searches as hard as cwebp -z 9 does a whole file), and the
/// compressed plane is the same at every quality and setting (it depends on the alpha values and
/// alpha settings alone; the VP8 data doesn't depend on it either). So it is compressed once
/// (`alpha`; at the lossy method, or WebpOptions::large_alpha_effort over PARALLEL_PIXELS), counted
/// in each candidate's size, and put in the file that ships (`finished`): libwebp's own file where
/// the effort is the method.
struct Lossies {
    source: libwebp::Source,
    scorer: Scorer,
    raw_alpha: bool,
    /// The compressed alpha plane: an ALPH chunk, header included (None without translucency).
    alpha: Option<Vec<u8>>,
    options: WebpOptions,
}

impl Lossies {
    fn new(pixels: &[RGBA], w: usize, h: usize, options: WebpOptions, alpha_effort: u8) -> Result<Self, Error> {
        let source = libwebp::Source::new(pixels.as_bytes(), w, h, options.sharp_yuv)?;
        let scorer = Scorer::new(pixels, w, h)?;
        let raw_alpha = pixels.iter().any(|p| p.a < 255);
        let mut lossies = Lossies { source, scorer, raw_alpha, alpha: None, options };
        if raw_alpha {
            // any quality: only the VP8 data differs; libwebp's alpha effort is its method
            let settings = libwebp::Lossy { method: alpha_effort, ..lossies.settings(FIRST_QUALITY as u32, libwebp::Tuning::default(), true) };
            let file = lossies.source.encode(&settings)?;
            lossies.alpha = riff_chunks(&file).find(|(k, _)| k == b"ALPH").map(|(kind, data)| chunk(&kind, data));
        }
        Ok(lossies)
    }

    fn settings(&self, quality: u32, tuning: libwebp::Tuning, alpha_compression: bool) -> libwebp::Lossy {
        let o = &self.options;
        libwebp::Lossy {
            quality: quality as f32,
            method: o.method,
            sharp_yuv: o.sharp_yuv,
            alpha_quality: 100,
            alpha_filtering: o.alpha_filtering,
            alpha_compression,
            tuning,
        }
    }

    /// The files at these qualities, scored: encoded at once, then decoded and scored one after
    /// another (scoring is parallel itself, and holds much memory on large images).
    fn tries(&self, qualities: &[u32], tuning: libwebp::Tuning) -> Result<Vec<Lossy>, Error> {
        let settings: Vec<libwebp::Lossy> = qualities.iter().map(|&q| self.settings(q, tuning, !self.raw_alpha)).collect();
        let source = &self.source;
        let files = par::map(&settings, |s| source.encode(s));
        qualities
            .iter()
            .zip(files)
            .map(|(&quality, file)| {
                let file = file?;
                let score = self.scorer.score(libwebp::decode(&file)?.rgba.as_rgba())?;
                let size = file.len() - if self.raw_alpha { chunk_size(&file, b"ALPH") } else { 0 } + self.alpha.as_ref().map_or(0, Vec::len);
                Ok(Lossy { quality, file, score, raw_alpha: self.raw_alpha, size, tuning })
            })
            .collect()
    }

    /// A search candidate as it ships: its alpha plane, if any, compressed (`alpha` in place of the
    /// uncompressed one). The VP8 data, and so the decoded image and its score, are the same.
    fn finished(&self, lossy: Lossy) -> Lossy {
        let (true, Some(alpha)) = (lossy.raw_alpha, &self.alpha) else { return lossy };
        let mut file = lossy.file[..12].to_vec();
        for (kind, data) in riff_chunks(&lossy.file) {
            if &kind == b"ALPH" {
                file.extend_from_slice(alpha);
            } else {
                file.extend(chunk(&kind, data));
            }
        }
        let riff = (file.len() - 8) as u32;
        file[4..8].copy_from_slice(&riff.to_le_bytes());
        Lossy { size: file.len(), file, raw_alpha: false, ..lossy }
    }

    /// The lowest quality whose file scores at least `target` (quality::search), aiming first at
    /// `estimate`; `known` files count as tried, and a file that falls short and is already `limit`
    /// bytes ends it.
    fn search(&self, target: f64, limit: usize, estimate: f64, known: Vec<Lossy>, tuning: libwebp::Tuning) -> Result<Found<Lossy>, Error> {
        quality::search(target, limit, estimate, SLOPE, known, |qualities| self.tries(qualities, tuning))
    }
}

impl Tried for Lossy {
    fn quality(&self) -> u32 {
        self.quality
    }
    fn score(&self) -> f64 {
        self.score
    }
    fn size(&self) -> usize {
        self.size
    }
}

/// A RIFF chunk: type, size, contents and a pad byte to an even size.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + data.len());
    out.extend_from_slice(kind);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// The size of a WebP file's chunk of this kind, header and padding included (0 if it has none).
fn chunk_size(webp: &[u8], kind: &[u8; 4]) -> usize {
    riff_chunks(webp).find(|(k, _)| k == kind).map_or(0, |(_, data)| 8 + data.len() + data.len() % 2)
}

/// The chunks of a WebP file (after "RIFF", the size and "WEBP"): type and contents.
fn riff_chunks(webp: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut rest = webp.get(12..).unwrap_or_default();
    std::iter::from_fn(move || {
        let kind: [u8; 4] = rest.get(..4)?.try_into().unwrap();
        let len = u32::from_le_bytes(rest.get(4..8)?.try_into().unwrap()) as usize;
        let data = rest.get(8..8 + len)?;
        rest = rest.get(8 + len + len % 2..).unwrap_or_default();
        Some((kind, data))
    })
}

/// The ICC profile a WebP version of this PNG needs to show the same colors: None for one in sRGB.
fn icc_profile(png: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    if chunks(png.get(8..).unwrap_or_default()).take_while(|(kind, _)| kind != b"IDAT").any(|(kind, _)| &kind == b"acTL") {
        return Err("animated PNGs can't be written as WebP yet".into());
    }
    let color = codec::color_chunks(png);
    let kinds: Vec<[u8; 4]> = chunks(&color).map(|(kind, _)| kind).collect();
    if kinds.is_empty() {
        return Ok(None);
    }
    // cICP takes precedence over iCCP, as iCCP does over gAMA and cHRM
    if kinds.contains(b"cICP") {
        return Err("this PNG's color space is set by cICP, which WebP can't carry; keep it as PNG".into());
    }
    let Some((_, iccp)) = chunks(&color).find(|(kind, _)| kind == b"iCCP") else {
        let names: Vec<String> = kinds.iter().map(|k| String::from_utf8_lossy(k).into_owned()).collect();
        return Err(format!("this PNG's color space is set by {} without an ICC profile, which WebP can't carry; keep it as PNG", names.join(" and ")).into());
    };
    // a profile name, a zero, the compression method (0, zlib), then the zlib stream
    let name = iccp.iter().position(|&b| b == 0).ok_or("bad iCCP chunk")?;
    let compressed = iccp.get(name + 2..).ok_or("bad iCCP chunk")?;
    let icc = miniz_oxide::inflate::decompress_to_vec_zlib(compressed).map_err(|_| "bad iCCP chunk")?;
    Ok(Some(icc))
}

/// The chunks of PNG chunk data (from after the signature, or `color_chunks`'s): type and contents.
fn chunks(mut bytes: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let len = u32::from_be_bytes(bytes.get(..4)?.try_into().unwrap()) as usize;
        let kind: [u8; 4] = bytes.get(4..8)?.try_into().unwrap();
        let data = bytes.get(8..8 + len)?;
        bytes = bytes.get(12 + len..).unwrap_or_default();
        Some((kind, data))
    })
}

/// A WebP file with an ICC profile: the extended format's VP8X header (flagging the profile, and
/// alpha if the image has any), the ICCP chunk, then the image's own chunks.
fn with_icc(webp: &[u8], icc: &[u8], w: usize, h: usize) -> Vec<u8> {
    let image: Vec<([u8; 4], &[u8])> = riff_chunks(webp).collect();
    let mut flags = 0x20; // ICC profile
    let has_alpha = |(kind, data): &([u8; 4], &[u8])| match kind {
        b"VP8X" => data[0] & 0x10 != 0,
        b"ALPH" => true,
        // the lossless header: signature, 14 + 14 bits of size, then alpha_is_used
        b"VP8L" => data.len() >= 5 && u32::from_le_bytes(data[1..5].try_into().unwrap()) >> 28 & 1 == 1,
        _ => false,
    };
    if image.iter().any(has_alpha) {
        flags |= 0x10;
    }
    let (w, h) = ((w - 1) as u32, (h - 1) as u32);
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend_from_slice(&w.to_le_bytes()[..3]);
    vp8x.extend_from_slice(&h.to_le_bytes()[..3]);
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend(chunk(b"ICCP", icc));
    for (kind, data) in image.iter().filter(|(kind, _)| kind != b"VP8X") {
        body.extend(chunk(kind, data));
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spliced_alpha_plane_makes_libwebps_own_file() {
        let pixels: Vec<RGBA> = (0..120 * 70usize)
            .map(|i| RGBA::new((i % 120 * 2) as u8, (i / 120 * 3) as u8, (i * 7 % 256) as u8, ((i % 120 + i / 120) * 2 % 256) as u8))
            .collect();
        let vp8 = |file: &[u8]| riff_chunks(file).find(|(k, _)| k == b"VP8 ").unwrap().1.to_vec();
        let options = WebpOptions::default();
        for alpha_effort in [options.method, options.large_alpha_effort] {
            let lossies = Lossies::new(&pixels, 120, 70, options, alpha_effort).unwrap();
            for (quality, tuning) in [(30, tuning(&options)), (75, libwebp::Tuning::default()), (95, tuning(&options))] {
                let shipped = lossies.finished(lossies.tries(&[quality], tuning).unwrap().remove(0));
                let direct = libwebp::encode_lossy(pixels.as_bytes(), 120, 70, &lossies.settings(quality, tuning, true)).unwrap();
                assert_eq!(shipped.size, shipped.file.len());
                if alpha_effort == options.method {
                    assert_eq!(shipped.file, direct); // libwebp's own file
                } else {
                    // libwebp's VP8 data, and its pixels
                    assert_eq!(vp8(&shipped.file), vp8(&direct));
                    assert_eq!(decode(&shipped.file).unwrap().pixels, decode(&direct).unwrap().pixels);
                }
            }
        }
    }
}
