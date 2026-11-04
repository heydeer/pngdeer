//! AVIF output (`compress_avif`): per image, the smallest AVIF file whose SSIMULACRA2 score, as a
//! browser shows it, reaches the target, encoded with libaom (vendor/libaom) by the rules
//! experiments/avif/FINDINGS.md round 8 measured: at TinyPNG's AVIF's score, image by image, 19%
//! smaller than TinyPNG's AVIF (smaller and no worse on 52 of 54 images).
//!
//! - Encoding: as avifenc encodes a still image (all-intra, one frame; libavif's quality 0–100 mapped
//!   to libaom's quantizer; tune=iq for color, PSNR for alpha, at the color's quality; but one tile),
//!   4:4:4 at 10 bits unless a setting says 8, from RGB as libavif converts it (BT.601's matrix, full
//!   range).
//! - Scoring: the frame as decoded (libaom's own copy, which is what every AV1 decoder outputs),
//!   converted to RGB as Chrome converts it (`shown`), against the source.
//! - The search: an image over SMALL_PIXELS gets the lowest quality whose speed-8 file reaches the
//!   target, then the lowest at speed 4 from one step below that (`final_search`). A smaller image,
//!   where encodes are cheap, gets each of SMALL_SETTINGS searched, the others near the first's
//!   answer (`search_near`), and the smallest file reaching the target is kept.
//! - Automatic target: about TinyPNG's AVIF's quality, which can't be predicted image by image
//!   (TinyPNG writes every AVIF at one quantizer, so its score follows the image): a JPEG photo
//!   PHOTO_TARGET, TinyPNG's median score on 30 photos; a PNG its PNG output's automatic target less
//!   PNG_GAP, the median gap on 24 PNGs, but no lower than a photo's. Files come out 21–24% smaller
//!   than TinyPNG's AVIF and as good on the median.
//! - Colors: a PNG's ICC profile goes into the file, and its cICP into the AV1 sequence header; an
//!   image in sRGB needs neither; gAMA and cHRM that aren't sRGB's are refused, as for WebP. A JPEG
//!   photo (with the jpeg feature) is turned as its EXIF orientation says, and keeps its ICC profile
//!   unless it is sRGB's. Animated PNGs are refused.
//! - The file: a small AVIF container of slimdeer's own (`container`).

use crate::codec::{self, Error};
use crate::par;
use crate::quality::{self, Found, Tried};
use crate::search::{self, Scorer};
use crate::{Settings, RGBA};

/// A JPEG photo's automatic target: TinyPNG's AVIF's median score on round 8's 30 photos.
const PHOTO_TARGET: f64 = 62.3;
/// A PNG's automatic target is its PNG output's less this: the median of TinyPNG's AVIF's score less
/// that target on round 8's 24 PNGs (whose PNG targets were 74 to 90). At least PHOTO_TARGET, though:
/// where a palette does badly (a smooth gradient's PNG target can be 30), TinyPNG's AVIF, at its one
/// quantizer, doesn't.
const PNG_GAP: f64 = 9.0;
/// Images up to this many pixels search all of SMALL_SETTINGS (round 8: 0.5 MP).
const SMALL_PIXELS: usize = 500_000;
/// For small images: each setting's lowest quality reaching the target is encoded, and the smallest
/// file wins. On round 8's 13 small images speed 2 won 11 times, screen content tools twice; 8 bits
/// wins now and then on flat icons. The first, the cheapest, is searched first, the others near its
/// answer: on the benchmark's 89 small images that took 45% fewer encodes than searching each
/// alone, and made the same files but for 6, 0.5–8% smaller.
const SMALL_SETTINGS: [Setting; 4] = [
    Setting { depth: 10, screen: false, speed: 4 },
    Setting { depth: 10, screen: true, speed: 4 },
    Setting { depth: 8, screen: false, speed: 4 },
    Setting { depth: 10, screen: false, speed: 2 },
];
/// Larger images: the quality is searched at speed 8 (a third of speed 6's time, 1.6% bigger), and
/// the file is made at speed 4 (14% smaller than speed 8's at the same score; 2.2 to 4.7 times speed
/// 6's time).
const SEARCH: Setting = Setting { depth: 10, screen: false, speed: 8 };
const FINAL: Setting = Setting { depth: 10, screen: false, speed: 4 };
/// Where a search aims first at PHOTO_TARGET, and about how many points a file's score gains per
/// quality step there: speed 8's lowest qualities on round 8's photos were 35 to 58, median 49.
const FIRST_QUALITY: f64 = 49.0;
const SLOPE: f64 = 0.6;
/// A round's files are encoded at once while their pixels fit in this, in turns beyond it: an encode
/// takes about 35 bytes a pixel at speed 8 and 50 at speed 4 (a 12 MP photo: 0.4 and 0.6 GB), and
/// the rest of a compression about 70 (a 21.8 MP photo, one encode at a time, peaked at 2.6 GB).
/// wasm32 has 4 GB: less at once there, so up to 24 MP stays under 3 GB. The files don't depend on it.
const ENCODED_AT_ONCE: usize = if cfg!(target_arch = "wasm32") { 24_000_000 } else { 40_000_000 };
/// As for JPEG output: files are scored at once while their scored pixels fit in this.
const SCORED_AT_ONCE: usize = 8_000_000;

/// libavif's quality (0–100) to libaom's quantizer (0–63) under tune=iq, the color's: a table libavif
/// tuned so tune=iq's sizes follow quality as tune=ssim's do (60 is quantizer 30; 100 is lossless).
const IQ_QUANTIZER: [u8; 101] = [
    63, 63, 63, 62, 62, 62, 61, 61, 60, 60, 59, 59, 58, 58, 57, 57, 56, 56, 55, 55, 54, 54, 53, 53, 52, 52, 51, 51, 50, 50, 49, 49, 48, 48, 47, 46, 46, 45, 45,
    44, 43, 43, 42, 42, 41, 40, 40, 39, 39, 38, 37, 37, 36, 36, 35, 34, 33, 33, 32, 31, 30, 30, 29, 28, 27, 27, 26, 25, 24, 24, 23, 22, 21, 21, 20, 19, 18, 18,
    17, 16, 15, 15, 14, 13, 12, 12, 11, 10, 9, 9, 8, 7, 6, 6, 5, 4, 3, 3, 2, 1, 0,
];

/// libavif's quality to libaom's quantizer under any other tuning, alpha's: linear.
fn linear_quantizer(quality: u32) -> u8 {
    (((100 - quality) * 63 + 50) / 100) as u8
}

/// An encoder setting: the bit depth, libaom's screen content tools, and libaom's speed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Setting {
    depth: u8,
    screen: bool,
    speed: u8,
}

impl Setting {
    fn name(&self) -> String {
        format!("{}bit{}-s{}", self.depth, if self.screen { "-screen" } else { "" }, self.speed)
    }
}

/// A compressed AVIF and what the search found.
pub struct CompressedAvif {
    pub avif: Vec<u8>,
    /// The target searched for (Settings::target, or the automatic one).
    pub target: f64,
    /// The setting and quality (libavif's, 0–100) the file was encoded with.
    pub setting: String,
    pub quality: u32,
    /// Its score against the input, as Chrome shows it.
    pub score: f64,
    /// Files encoded and scored, in how many rounds.
    pub tries: usize,
    pub rounds: usize,
    /// "hit", or "miss" (no quality reaches the target; the closest).
    pub outcome: &'static str,
    /// Images over SMALL_PIXELS: the speed-8 search's quality, file size and score.
    pub search: Option<(u32, usize, f64)>,
    /// Milliseconds: decoding the input (and its automatic target), and the search.
    pub ms: (f64, f64),
}

impl CompressedAvif {
    pub fn json(&self) -> String {
        let search = self.search.map_or("null".into(), |(q, b, s)| format!(r#"{{"quality": {q}, "bytes": {b}, "score": {s:.3}}}"#));
        format!(
            r#"{{"format": "avif", "target": {:.2}, "setting": "{}", "quality": {}, "score": {:.3}, "tries": {}, "rounds": {}, "outcome": "{}", "search": {search}, "ms": {{"decode": {:.0}, "search": {:.0}}}, "bytes": {}}}"#,
            self.target,
            self.setting,
            self.quality,
            self.score,
            self.tries,
            self.rounds,
            self.outcome,
            self.ms.0,
            self.ms.1,
            self.avif.len()
        )
    }
}

/// The image to encode: its pixels as YUV for each bit depth tried, its alpha plane if any pixel is
/// translucent, and its color information.
struct Source {
    w: usize,
    h: usize,
    /// Y, U and V at 10 bits, and at 8 bits if a small setting uses them.
    yuv10: [Vec<u16>; 3],
    yuv8: Option<[Vec<u16>; 3]>,
    alpha10: Option<Vec<u16>>,
    alpha8: Option<Vec<u16>>,
    color: Color,
    scorer: Scorer,
}

/// The color information an AVIF carries: an ICC profile, and the sequence header's (and the colr
/// box's) code points.
struct Color {
    icc: Option<Vec<u8>>,
    cicp: aom::Cicp,
}

/// One file the search tried.
struct Attempt {
    setting: Setting,
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

/// Counts what the searches did.
#[derive(Default)]
struct Work {
    tries: usize,
    rounds: usize,
}

impl Work {
    fn add<T>(&mut self, found: &Found<T>) {
        self.tries += found.tries;
        self.rounds += found.rounds;
    }
}

/// Compresses one PNG file, or a JPEG photo (with the jpeg feature), to AVIF (see the module docs).
pub fn compress_avif(input: &[u8], settings: &Settings) -> Result<CompressedAvif, Error> {
    let tick = || settings.clock.map_or(0.0, |c| c());
    let t = tick();
    let (pixels, w, h, color, photo) = decode(input)?;
    let target = match settings.target {
        Some(target) => target,
        None if photo => PHOTO_TARGET,
        None => (search::auto_target(&pixels, w, h, settings)? - PNG_GAP).max(PHOTO_TARGET),
    };
    let small = w * h <= SMALL_PIXELS;
    let source = Source::new(&pixels, w, h, color, small)?;
    drop(pixels);
    let decode_ms = tick() - t;

    let t = tick();
    let mut work = Work::default();
    let (best, search) = if small {
        // the first setting, the cheapest, scouts; the others land within a few steps of its quality
        let (scout, others) = SMALL_SETTINGS.split_first().unwrap();
        let first = source.search(*scout, target, Vec::new())?;
        let near = first.best.as_ref().map(|a| a.quality);
        let found = par::map(others, |&setting| {
            match near {
                Some(quality) => source.search_near(setting, target, quality),
                None => source.search(setting, target, Vec::new()),
            }
            .map_err(|e| e.to_string())
        });
        let mut kept = Vec::new();
        for found in [Ok(first)].into_iter().chain(found) {
            let found = found?;
            work.add(&found);
            kept.extend(found.best.map(|a| (a, true)).or(found.closest.map(|a| (a, false))));
        }
        // the smallest file that reaches the target, else the best score short of it
        let reaching = kept.iter().any(|(_, hit)| *hit);
        let best = kept
            .into_iter()
            .filter(|(_, hit)| *hit == reaching)
            .min_by(|(a, _), (b, _)| if reaching { a.file.len().cmp(&b.file.len()) } else { b.score.total_cmp(&a.score) })
            .map(|(a, _)| a);
        (best, None)
    } else {
        let found = source.search(SEARCH, target, Vec::new())?;
        work.add(&found);
        let search = found.best.as_ref().map(|a| (a.quality, a.file.len(), a.score));
        (source.final_search(found, target, &mut work)?, search)
    };
    let best = best.ok_or("no AVIF quality to try")?;
    let outcome = if best.score >= target { "hit" } else { "miss" };
    Ok(CompressedAvif {
        target,
        setting: best.setting.name(),
        quality: best.quality,
        score: best.score,
        avif: best.file,
        tries: work.tries,
        rounds: work.rounds,
        outcome,
        search,
        ms: (decode_ms, tick() - t),
    })
}

/// The input as RGBA as shown, its size, its color information, and whether it is a JPEG photo.
fn decode(input: &[u8]) -> Result<(Vec<RGBA>, usize, usize, Color, bool), Error> {
    #[cfg(feature = "jpeg")]
    if crate::jpeg::is_jpeg(input) {
        let photo = crate::jpeg::decode(input)?;
        let (pixels, w, h) = crate::jpeg::oriented(photo.pixels, photo.width, photo.height, photo.orientation);
        let icc = photo.icc.filter(|icc| !crate::jpeg::is_srgb(icc));
        return Ok((pixels, w, h, Color::with_icc(icc), true));
    }
    let color = png_color(input)?;
    let bitmap = codec::decode(input)?;
    Ok((bitmap.pixels, bitmap.width, bitmap.height, color, false))
}

impl Color {
    fn with_icc(icc: Option<Vec<u8>>) -> Self {
        // with a profile the primaries and transfer function are its own; the matrix is still the
        // YUV conversion's
        let cicp = if icc.is_some() { aom::Cicp { color_primaries: 2, transfer_characteristics: 2, ..aom::Cicp::SRGB } } else { aom::Cicp::SRGB };
        Color { icc, cicp }
    }
}

/// What an AVIF of this PNG needs to show the same colors (see the module docs).
fn png_color(png: &[u8]) -> Result<Color, Error> {
    if codec::chunks(png.get(8..).unwrap_or_default()).take_while(|(kind, _)| kind != b"IDAT").any(|(kind, _)| &kind == b"acTL") {
        return Err("animated PNGs can't be written as AVIF yet".into());
    }
    let color = codec::color_chunks(png);
    let kinds: Vec<[u8; 4]> = codec::chunks(&color).map(|(kind, _)| kind).collect();
    if kinds.is_empty() {
        return Ok(Color::with_icc(None));
    }
    // cICP takes precedence over iCCP, as iCCP does over gAMA and cHRM; its matrix is RGB's (0),
    // and the file's is the YUV conversion's
    if let Some((_, cicp)) = codec::chunks(&color).find(|(kind, _)| kind == b"cICP") {
        if cicp.len() != 4 || cicp[2] != 0 || cicp[3] != 1 {
            return Err("this PNG's cICP chunk isn't full-range RGB; keep it as PNG".into());
        }
        return Ok(Color { icc: None, cicp: aom::Cicp { color_primaries: cicp[0], transfer_characteristics: cicp[1], ..aom::Cicp::SRGB } });
    }
    let Some((_, iccp)) = codec::chunks(&color).find(|(kind, _)| kind == b"iCCP") else {
        let names: Vec<String> = kinds.iter().map(|k| String::from_utf8_lossy(k).into_owned()).collect();
        return Err(format!("this PNG's color space is set by {} without an ICC profile, which AVIF can't carry; keep it as PNG", names.join(" and ")).into());
    };
    // a profile name, a zero, the compression method (0, zlib), then the zlib stream
    let name = iccp.iter().position(|&b| b == 0).ok_or("bad iCCP chunk")?;
    let compressed = iccp.get(name + 2..).ok_or("bad iCCP chunk")?;
    let icc = miniz_oxide::inflate::decompress_to_vec_zlib(compressed).map_err(|_| "bad iCCP chunk")?;
    Ok(Color::with_icc(Some(icc)))
}

impl Source {
    fn new(pixels: &[RGBA], w: usize, h: usize, color: Color, small: bool) -> Result<Self, Error> {
        let translucent = pixels.iter().any(|p| p.a != 255);
        Ok(Source {
            w,
            h,
            yuv10: to_yuv(pixels, 10),
            yuv8: small.then(|| to_yuv(pixels, 8)),
            alpha10: translucent.then(|| pixels.iter().map(|p| alpha_at(p.a, 10)).collect()),
            alpha8: (translucent && small).then(|| pixels.iter().map(|p| p.a as u16).collect()),
            color,
            scorer: Scorer::new(pixels, w, h)?,
        })
    }

    /// The lowest quality at `setting` whose file reaches `target`, rounds of files encoded at
    /// once; `known` files count as tried.
    fn search(&self, setting: Setting, target: f64, known: Vec<Attempt>) -> Result<Found<Attempt>, Error> {
        let estimate = (FIRST_QUALITY + (target - PHOTO_TARGET) / SLOPE).clamp(10.0, 90.0);
        quality::search(target, usize::MAX, estimate, SLOPE, known, |qualities| self.attempts(setting, qualities))
    }

    /// `search` for a setting whose answer should be near `quality` (another setting's): rounds of
    /// neighbouring qualities from one step below it, as speed 2 and 8 bits tend to land a step or
    /// two lower than 10-bit speed 4 and screen content tools either way (round 8's 13 small images:
    /// within 6 steps).
    fn search_near(&self, setting: Setting, target: f64, quality: u32) -> Result<Found<Attempt>, Error> {
        let estimate = (quality as f64 - 1.0).max(0.0);
        quality::search_spread(target, usize::MAX, estimate, SLOPE, 1, Vec::new(), |qualities| self.attempts(setting, qualities))
    }

    /// Speed 4's file for an image over SMALL_PIXELS, from the speed-8 search (round 8's FINAL=margin):
    /// one step below speed 8's quality, as speed 4's file usually lands there, stepping up while it
    /// falls short, unless it beats the target by more than a step's worth of points (the speed-8
    /// files' slope), when it searches downward from there. The first two of those qualities are
    /// encoded at once where they fit.
    fn final_search(&self, found: Found<Attempt>, target: f64, work: &mut Work) -> Result<Option<Attempt>, Error> {
        let Some(fast) = found.best else {
            // speed 8 never reaches it: speed 4's own search, for the closest it can get
            let found = self.search(FINAL, target, Vec::new())?;
            work.add(&found);
            return Ok(found.best.or(found.closest));
        };
        let per_step = slope_near(&found.scores, fast.quality);
        let start = fast.quality.saturating_sub(1);
        let mut tried: Vec<Attempt> = Vec::new();
        // the next quality too where both fit at once (ENCODED_AT_ONCE); beyond, it waits to be
        // needed, which on round 8's photos it was 17 times in 41
        let both = ENCODED_AT_ONCE / (self.w * self.h) >= 2;
        let qualities: Vec<u32> = (start..=(start + both as u32).min(100)).collect();
        tried.extend(self.attempts(FINAL, &qualities)?);
        (work.tries, work.rounds) = (work.tries + qualities.len(), work.rounds + 1);
        if tried[0].score >= target + per_step {
            let found = quality::search(target, usize::MAX, start as f64, per_step, tried, |q| self.attempts(FINAL, q))?;
            work.add(&found);
            return Ok(found.best.or(found.closest));
        }
        let mut quality = start;
        loop {
            let at = tried.iter().position(|a| a.quality == quality).unwrap();
            if tried[at].score >= target || quality == 100 {
                return Ok(Some(tried.swap_remove(at)));
            }
            quality += 1;
            if !tried.iter().any(|a| a.quality == quality) {
                tried.extend(self.attempts(FINAL, &[quality])?);
                (work.tries, work.rounds) = (work.tries + 1, work.rounds + 1);
            }
        }
    }

    /// Files at `setting`, one per quality, encoded at once (as many as fit in ENCODED_AT_ONCE),
    /// then scored as SCORED_AT_ONCE allows.
    fn attempts(&self, setting: Setting, qualities: &[u32]) -> Result<Vec<Attempt>, Error> {
        let encode_at_once = (ENCODED_AT_ONCE / (self.w * self.h)).max(1);
        let mut encoded = Vec::new();
        for some in qualities.chunks(encode_at_once) {
            encoded.extend(par::map(some, |&quality| self.encode(setting, quality).map_err(|e| e.to_string())));
        }
        let encoded = encoded.into_iter().collect::<Result<Vec<_>, _>>()?;
        let score_at_once = (SCORED_AT_ONCE / self.scorer.scored_pixels()).max(1);
        let scores: Vec<_> =
            encoded.chunks(score_at_once).flat_map(|some| par::map(some, |(_, shown)| self.scorer.score(shown).map_err(|e| e.to_string()))).collect();
        encoded.into_iter().zip(scores).map(|((attempt, _), score)| Ok(Attempt { score: score?, ..attempt })).collect()
    }

    /// One file, and its pixels as shown (its score still to come).
    fn encode(&self, setting: Setting, quality: u32) -> Result<(Attempt, Vec<RGBA>), Error> {
        let (yuv, alpha) = match setting.depth {
            8 => (self.yuv8.as_ref().ok_or("no 8-bit YUV")?, self.alpha8.as_ref()),
            _ => (&self.yuv10, self.alpha10.as_ref()),
        };
        let planes: Vec<&[u16]> = yuv.iter().map(|p| p.as_slice()).collect();
        // quality 100 is lossless, where libavif sets no tuning
        let tune = (quality < 100).then_some(aom::Tune::Iq);
        let options = aom::Options {
            quantizer: IQ_QUANTIZER[quality as usize],
            speed: setting.speed,
            tune,
            screen: setting.screen,
            // one tile: avifenc's default tiling (up to 8 tiles of at least 512 × 512) changes
            // tune=iq's choices, not just the boundaries, and untiled, round 8's 54 images came out
            // 0.5% smaller in total at the same scores (3.6% on the PNGs; FINDINGS.md round 9)
            tiles_log2: (0, 0),
            cicp: self.color.cicp,
        };
        let color = aom::encode(&planes, self.w, self.h, setting.depth, &options)?;
        let alpha = match alpha {
            Some(plane) => {
                let options = aom::Options { quantizer: linear_quantizer(quality), tune: Some(aom::Tune::Psnr), ..options };
                Some(aom::encode(&[plane], self.w, self.h, setting.depth, &options)?)
            }
            None => None,
        };
        let shown = shown(&color.decoded, alpha.as_ref().map(|a| a.decoded.as_slice()), self.w * self.h, setting.depth);
        let file = container(&color.data, alpha.as_ref().map(|a| a.data.as_slice()), self.w, self.h, setting.depth, &self.color)?;
        Ok((Attempt { setting, quality, file, score: 0.0 }, shown))
    }
}

/// Points per quality step around `quality`, from the files within 10 steps of it (a least-squares
/// line through their qualities and scores), at least 0.3; 0.7 without two qualities to go by.
fn slope_near(scores: &[(u32, f64)], quality: u32) -> f64 {
    let near: Vec<(f64, f64)> = scores.iter().filter(|(q, _)| q.abs_diff(quality) <= 10).map(|&(q, s)| (q as f64, s)).collect();
    let n = near.len() as f64;
    let (mx, my) = (near.iter().map(|p| p.0).sum::<f64>() / n, near.iter().map(|p| p.1).sum::<f64>() / n);
    let var: f64 = near.iter().map(|p| (p.0 - mx) * (p.0 - mx)).sum();
    if near.len() < 2 || var == 0.0 {
        return 0.7;
    }
    (near.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / var).max(0.3)
}

/// RGB to Y, U and V planes at `depth` bits, as libavif converts (in single precision): BT.601's
/// matrix, full range.
fn to_yuv(pixels: &[RGBA], depth: u8) -> [Vec<u16>; 3] {
    const KR: f32 = 0.299;
    const KB: f32 = 0.114;
    const KG: f32 = 1.0 - KR - KB;
    let max = ((1u32 << depth) - 1) as f32;
    let bias = (1u32 << (depth - 1)) as f32;
    let unorm = |v: f32, bias: f32| (v * max + bias + 0.5).floor().clamp(0.0, max) as u16;
    let mut planes = [Vec::with_capacity(pixels.len()), Vec::with_capacity(pixels.len()), Vec::with_capacity(pixels.len())];
    for p in pixels {
        let (r, g, b) = (p.r as f32 / 255.0, p.g as f32 / 255.0, p.b as f32 / 255.0);
        let y = KR * r + KG * g + KB * b;
        planes[0].push(unorm(y, 0.0));
        planes[1].push(unorm((b - y) / (2.0 * (1.0 - KB)), bias));
        planes[2].push(unorm((r - y) / (2.0 * (1.0 - KR)), bias));
    }
    planes
}

/// An 8-bit alpha value at `depth` bits, as libavif scales it.
fn alpha_at(a: u8, depth: u8) -> u16 {
    let max = ((1u32 << depth) - 1) as f32;
    (0.5 + a as f32 / 255.0 * max) as u16
}

/// A decoded frame as Chrome shows it: libyuv's fixed-point BT.601 full-range conversion (Chrome's
/// libavif converts with libyuv), where 10-bit chroma is cut to 8 bits first and 10-bit luma scaled
/// to 16; alpha cut to 8 bits. `color` holds the Y, U and V planes, `pixels` samples each.
fn shown(color: &[u16], alpha: Option<&[u16]>, pixels: usize, depth: u8) -> Vec<RGBA> {
    // libyuv's kYuvJPEGConstants
    const YG: u32 = 16320;
    const YB: i32 = 32;
    const UB: i32 = 113;
    const UG: i32 = 22;
    const VG: i32 = 46;
    const VR: i32 = 90;
    let (y, u, v) = (&color[..pixels], &color[pixels..2 * pixels], &color[2 * pixels..]);
    let clamp = |x: i32| (x >> 6).clamp(0, 255) as u8;
    (0..pixels)
        .map(|i| {
            let (y32, u, v, a) = if depth > 8 {
                let y = y[i] as u32;
                ((y << 6) | (y >> 4), (u[i] >> 2).min(255) as i32, (v[i] >> 2).min(255) as i32, alpha.map_or(255, |a| (a[i] >> 2).min(255) as u8))
            } else {
                (y[i] as u32 * 0x0101, u[i] as i32, v[i] as i32, alpha.map_or(255, |a| a[i] as u8))
            };
            let y1 = ((y32 * YG) >> 16) as i32 + YB;
            let (u, v) = (u - 128, v - 128);
            RGBA::new(clamp(y1 + v * VR), clamp(y1 - (u * UG + v * VG)), clamp(y1 + u * UB), a)
        })
        .collect()
}

/// An OBU: its type, all of it, and its payload.
type Obu<'a> = (u8, &'a [u8], &'a [u8]);

/// The OBUs of AV1 data.
fn obus(data: &[u8]) -> Result<Vec<Obu<'_>>, Error> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let header = data[at];
        let kind = header >> 3 & 15;
        // header, extension, then (libaom always writes it) the size as LEB128
        let mut end = at + 1 + (header >> 2 & 1) as usize;
        let (mut size, mut shift) = (0usize, 0);
        loop {
            let byte = *data.get(end).ok_or("truncated AV1 data")?;
            end += 1;
            size |= ((byte & 127) as usize) << shift;
            shift += 7;
            if byte & 128 == 0 || shift > 56 {
                break;
            }
        }
        let start = end;
        let end = end.checked_add(size).filter(|&e| e <= data.len()).ok_or("truncated AV1 data")?;
        out.push((kind, &data[at..end], &data[start..end]));
        at = end;
    }
    Ok(out)
}

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;

/// An image item's AV1 data (libaom's temporal unit without its temporal delimiter, which AVIF
/// leaves out) and its av1C property (its sequence header's profile and level; a still picture's
/// header is the reduced one, with tier 0).
fn item(data: &[u8], depth: u8, monochrome: bool) -> Result<(Vec<u8>, [u8; 4]), Error> {
    let obus = obus(data)?;
    let (_, _, header) = obus.iter().find(|(kind, _, _)| *kind == OBU_SEQUENCE_HEADER).ok_or("no AV1 sequence header")?;
    // seq_profile (3 bits), still_picture, reduced_still_picture_header, seq_level_idx[0] (5 bits)
    let bits = u16::from_be_bytes(header.get(..2).ok_or("short sequence header")?.try_into().unwrap());
    if bits >> 11 & 1 == 0 {
        return Err("libaom wrote a full sequence header for a still picture".into());
    }
    let (profile, level) = ((bits >> 13) as u8, (bits >> 6 & 31) as u8);
    let subsampled = monochrome as u8; // 4:4:4, or monochrome (4:2:0's subsampling flags)
    let av1c = [0x81, profile << 5 | level, ((depth > 8) as u8) << 6 | (monochrome as u8) << 4 | subsampled << 3 | subsampled << 2, 0];
    let data = obus.iter().filter(|(kind, _, _)| *kind != OBU_TEMPORAL_DELIMITER).flat_map(|(_, obu, _)| obu.iter().copied()).collect();
    Ok((data, av1c))
}

/// An ISOBMFF box.
fn boxed(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let mut out = ((content.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(content);
    out
}

/// A full box (version 0, no flags unless given).
fn full_box(kind: &[u8; 4], version: u8, content: &[u8]) -> Vec<u8> {
    boxed(kind, &[&[version, 0, 0, 0][..], content].concat())
}

/// An AVIF file of a color item (1) and, if given, an alpha item (2) auxiliary to it: the boxes
/// HEIF, MIAF and AVIF require and no more (no item names or optional brands), colr boxes for the
/// code points and the ICC profile if any, and the image data in one mdat.
fn container(color: &[u8], alpha: Option<&[u8]>, w: usize, h: usize, depth: u8, info: &Color) -> Result<Vec<u8>, Error> {
    let (color, color_av1c) = item(color, depth, false)?;
    let alpha = alpha.map(|a| item(a, depth, true)).transpose()?;
    let items = 1 + alpha.is_some() as u16;
    let ftyp = boxed(b"ftyp", b"avif\0\0\0\0avifmif1miaf");
    let hdlr = full_box(b"hdlr", 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
    let pitm = full_box(b"pitm", 0, &1u16.to_be_bytes());
    let mut iinf = items.to_be_bytes().to_vec();
    for id in 1..=items {
        iinf.extend(full_box(b"infe", 2, &[&id.to_be_bytes()[..], b"\0\0av01\0"].concat()));
    }
    let iinf = full_box(b"iinf", 0, &iinf);
    let iref = alpha.is_some().then(|| full_box(b"iref", 0, &boxed(b"auxl", &[0, 2, 0, 1, 0, 1])));

    // properties, numbered from 1: ispe, then the color's, then the alpha's
    let mut ipco = Vec::new();
    let ispe = [(w as u32).to_be_bytes(), (h as u32).to_be_bytes()].concat();
    ipco.extend(full_box(b"ispe", 0, &ispe));
    ipco.extend(full_box(b"pixi", 0, &[3, depth, depth, depth]));
    ipco.extend(boxed(b"av1C", &color_av1c));
    let c = &info.cicp;
    // the code points, and full range
    ipco.extend(boxed(b"colr", &[b"nclx", &[0, c.color_primaries, 0, c.transfer_characteristics, 0, c.matrix_coefficients, 0x80][..]].concat()));
    let mut color_props = vec![1, 2, 0x80 | 3, 4];
    if let Some(icc) = &info.icc {
        ipco.extend(boxed(b"colr", &[b"prof", icc.as_slice()].concat()));
        color_props.push(5);
    }
    let mut ipma = (items as u32).to_be_bytes().to_vec();
    ipma.extend_from_slice(&1u16.to_be_bytes());
    ipma.push(color_props.len() as u8);
    ipma.extend_from_slice(&color_props);
    if let Some((_, alpha_av1c)) = &alpha {
        let first = color_props.len() as u8 + 1;
        ipco.extend(full_box(b"pixi", 0, &[1, depth]));
        ipco.extend(boxed(b"av1C", alpha_av1c));
        ipco.extend(full_box(b"auxC", 0, b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0"));
        ipma.extend_from_slice(&[0, 2, 4, 1, first, 0x80 | (first + 1), first + 2]);
    }
    let iprp = boxed(b"iprp", &[boxed(b"ipco", &ipco), full_box(b"ipma", 0, &ipma)].concat());

    // iloc's offsets are from the file's start, so its size comes first: 4-byte offsets and
    // lengths, no base offset, one extent per item
    let iloc_size = 12 + 4 + items as usize * 14;
    let meta_size = 12 + hdlr.len() + pitm.len() + iloc_size + iinf.len() + iref.as_ref().map_or(0, Vec::len) + iprp.len();
    let mdat_start = ftyp.len() + meta_size + 8;
    let mut iloc = vec![0x44, 0x00];
    iloc.extend_from_slice(&items.to_be_bytes());
    let mut offset = mdat_start;
    let mut add = |id: u16, data: &[u8]| {
        iloc.extend_from_slice(&id.to_be_bytes());
        iloc.extend_from_slice(&[0, 0, 0, 1]); // data_reference_index 0, one extent
        iloc.extend_from_slice(&(offset as u32).to_be_bytes());
        iloc.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += data.len();
    };
    add(1, &color);
    if let Some((data, _)) = &alpha {
        add(2, data);
    }
    let iloc = full_box(b"iloc", 0, &iloc);
    debug_assert_eq!(iloc.len(), iloc_size);
    let meta = full_box(b"meta", 0, &[hdlr, pitm, iloc, iinf, iref.unwrap_or_default(), iprp].concat());
    let mut mdat = color;
    if let Some((data, _)) = alpha {
        mdat.extend_from_slice(&data);
    }
    if mdat_start + mdat.len() > u32::MAX as usize {
        return Err("too big for an AVIF file here".into());
    }
    Ok([ftyp, meta, boxed(b"mdat", &mdat)].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantizers_follow_libavif() {
        assert_eq!((IQ_QUANTIZER[0], IQ_QUANTIZER[60], IQ_QUANTIZER[99], IQ_QUANTIZER[100]), (63, 30, 1, 0));
        assert!(IQ_QUANTIZER.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!([0, 50, 60, 99, 100].map(linear_quantizer), [63, 32, 25, 1, 0]);
    }

    #[test]
    fn gray_and_extremes_convert_exactly() {
        let pixels = [RGBA::new(0, 0, 0, 255), RGBA::new(255, 255, 255, 255), RGBA::new(128, 128, 128, 255)];
        let yuv = to_yuv(&pixels, 10);
        assert_eq!(yuv[0], [0, 1023, 514]);
        assert_eq!((yuv[1].clone(), yuv[2].clone()), (vec![512; 3], vec![512; 3]));
        let back = shown(&yuv.concat(), None, 3, 10);
        assert_eq!(back.iter().map(|p| (p.r, p.g, p.b)).collect::<Vec<_>>(), [(0, 0, 0), (255, 255, 255), (128, 128, 128)]);
        assert_eq!([0, 1, 128, 254, 255].map(|a| alpha_at(a, 10) >> 2), [0, 1, 128, 254, 255]);
    }
}
