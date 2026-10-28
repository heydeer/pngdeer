//! webplab LIST SETTINGS [--fine] > OUT.tsv: libwebp settings for pngdeer's WebP output, compared by
//! the bytes each needs to reach the same score (experiments/webp).
//!
//! LIST is a TSV of images and target scores (path, target). For each image and each setting in
//! SETTINGS (comma-separated names, see `setting`), the lowest lossy quality whose file scores at
//! least the target as pngdeer scores (whole steps, searched as pngdeer does; with --fine, then
//! quarter steps below it), and near-lossless files (`nearN`, N in NEAR) with their scores.
//! Prints: image, setting, quality (-1: none reaches the target; near-lossless: N), bytes, score,
//! files scored. Bytes are the file's as pngdeer ships it, with the alpha plane compressed: that
//! plane is the same at every lossy setting, so it is compressed once per image and added.

use std::{env, fs, process};

use libwebp::{Lossy, Tuning};
use pngdeer::{codec, search::Scorer, RGBA};
use rayon::prelude::*;
use rgb::{ComponentBytes, FromSlice};

type Error = Box<dyn std::error::Error + Send + Sync>;

const NEAR: [u8; 6] = [100, 80, 60, 40, 20, 0];

/// A named lossy setting, atoms joined by "+" (af+fs6): method, sharp YUV and libwebp's finer
/// settings (its presets as config_enc.c sets them).
fn setting(name: &str) -> Option<(u8, bool, Tuning)> {
    let (mut method, mut sharp_yuv, mut t) = (6, true, Tuning::default());
    for atom in name.split('+') {
        match atom {
            "base" => {}
            "m5" => method = 5,
            "nosharp" => sharp_yuv = false,
            "af" => t.autofilter = Some(true),
            "sns0" => t.sns_strength = Some(0),
            "sns25" => t.sns_strength = Some(25),
            "sns75" => t.sns_strength = Some(75),
            "sns100" => t.sns_strength = Some(100),
            "f0" => t.filter_strength = Some(0),
            "f20" => t.filter_strength = Some(20),
            "f40" => t.filter_strength = Some(40),
            "f80" => t.filter_strength = Some(80),
            "fs3" => t.filter_sharpness = Some(3),
            "fs6" => t.filter_sharpness = Some(6),
            "fs7" => t.filter_sharpness = Some(7),
            "simple" => t.filter_type = Some(0),
            "seg1" => t.segments = Some(1),
            "seg2" => t.segments = Some(2),
            "pre1" => t.preprocessing = Some(1),
            "pre2" => t.preprocessing = Some(2),
            "picture" => (t.sns_strength, t.filter_sharpness, t.filter_strength) = (Some(80), Some(4), Some(35)),
            "photo" => (t.sns_strength, t.filter_sharpness, t.filter_strength, t.preprocessing) = (Some(80), Some(3), Some(30), Some(2)),
            "drawing" => (t.sns_strength, t.filter_sharpness, t.filter_strength) = (Some(25), Some(6), Some(10)),
            "text" => (t.sns_strength, t.filter_strength, t.segments) = (Some(0), Some(0), Some(2)),
            _ => return None,
        }
    }
    Some((method, sharp_yuv, t))
}

/// The size of a WebP file's chunk of this kind, header and padding included (0 if none).
fn chunk_size(webp: &[u8], kind: &[u8; 4]) -> usize {
    let mut pos = 12;
    while pos + 8 <= webp.len() {
        let len = u32::from_le_bytes(webp[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &webp[pos..pos + 4] == kind {
            return 8 + len + len % 2;
        }
        pos += 8 + len + len % 2;
    }
    0
}

struct Image {
    pixels: Vec<RGBA>,
    w: usize,
    h: usize,
    scorer: Scorer,
    /// The compressed alpha chunk's size (0 for an opaque image).
    alpha: usize,
}

impl Image {
    /// The file's size as shipped and its score, at a lossy setting and quality.
    fn lossy(&self, (method, sharp_yuv, tuning): (u8, bool, Tuning), quality: f32) -> Result<(usize, f64), Error> {
        let settings = Lossy { quality, method, sharp_yuv, alpha_quality: 100, alpha_filtering: 1, alpha_compression: self.alpha == 0, tuning };
        let file = libwebp::encode_lossy(self.pixels.as_bytes(), self.w, self.h, &settings)?;
        let score = self.scorer.score(libwebp::decode(&file)?.rgba.as_rgba()).map_err(|e| e.to_string())?;
        Ok((file.len() - chunk_size(&file, b"ALPH") + self.alpha, score))
    }
}

/// The lowest whole quality reaching `target` (pngdeer's search: from 70, aimed by the scores so
/// far, halving when that stalls), then with `fine` quarter steps below it: quality, bytes, score
/// (quality -1: none reaches it; then quality 100's bytes and score), and files scored.
fn search(image: &Image, setting: (u8, bool, Tuning), target: f64, fine: bool) -> Result<(f32, usize, f64, usize), Error> {
    let (mut lo, mut hi): ((i32, Option<f64>), (i32, Option<f64>)) = ((-1, None), (101, None));
    let (mut quality, mut best, mut top, mut probes, mut halve) = (70, None, None, 0, false);
    loop {
        let (bytes, score) = image.lossy(setting, quality as f32)?;
        probes += 1;
        if quality == 100 {
            top = Some((bytes, score));
        }
        let width = hi.0 - lo.0;
        if score >= target {
            hi = (quality, Some(score));
            best = Some((quality as f32, bytes, score));
        } else {
            lo = (quality, Some(score));
        }
        if hi.0 - lo.0 <= 1 {
            break;
        }
        halve = !halve && 2 * (hi.0 - lo.0) > width;
        quality = if halve {
            (lo.0 + hi.0) / 2
        } else {
            let at = match (lo.1, hi.1) {
                (Some(a), Some(b)) if b > a => lo.0 as f64 + (target - a) / (b - a) * (hi.0 - lo.0) as f64,
                (Some(a), None) => lo.0 as f64 + (target - a) / 0.5,
                (None, Some(b)) => hi.0 as f64 - (b - target) / 0.5,
                _ => (lo.0 + hi.0) as f64 / 2.0,
            };
            (at.ceil() as i32).clamp(lo.0 + 1, hi.0 - 1)
        };
    }
    let Some(mut best) = best else {
        let (bytes, score) = match top {
            Some(t) => t,
            None => image.lossy(setting, 100.0)?,
        };
        return Ok((-1.0, bytes, score, probes));
    };
    if fine && best.0 > 0.0 {
        let mut step = 0.5;
        let mut q = best.0 - step;
        while step >= 0.25 {
            let (bytes, score) = image.lossy(setting, q)?;
            probes += 1;
            step /= 2.0;
            if score >= target && bytes <= best.1 {
                best = (q, bytes, score);
                q -= step;
            } else {
                q += step;
            }
        }
    }
    Ok((best.0, best.1, best.2, probes))
}

fn run(path: &str, target: f64, settings: &[&str], fine: bool) -> Result<Vec<String>, Error> {
    let bitmap = codec::decode(&fs::read(path)?).map_err(|e| e.to_string())?;
    let (w, h) = (bitmap.width, bitmap.height);
    let scorer = Scorer::new(&bitmap.pixels, w, h).map_err(|e| e.to_string())?;
    let mut image = Image { pixels: bitmap.pixels, w, h, scorer, alpha: 0 };
    if image.pixels.iter().any(|p| p.a < 255) {
        let settings =
            Lossy { quality: 75.0, method: 6, sharp_yuv: true, alpha_quality: 100, alpha_filtering: 1, alpha_compression: true, tuning: Tuning::default() };
        image.alpha = chunk_size(&libwebp::encode_lossy(image.pixels.as_bytes(), w, h, &settings)?, b"ALPH");
    }
    let mut lines = Vec::new();
    for name in settings {
        let (quality, bytes, score, probes) = search(&image, setting(name).unwrap(), target, fine)?;
        lines.push(format!("{path}\t{name}\t{quality}\t{bytes}\t{score:.3}\t{probes}"));
    }
    for near in NEAR {
        let file = libwebp::encode_near_lossless(image.pixels.as_bytes(), w, h, 6, near)?;
        let score = image.scorer.score(libwebp::decode(&file)?.rgba.as_rgba()).map_err(|e| e.to_string())?;
        lines.push(format!("{path}\tnear{near}\t{near}\t{}\t{score:.3}\t1", file.len()));
    }
    Ok(lines)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let fine = args.iter().any(|a| a == "--fine");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--fine").collect();
    if args.len() != 2 {
        eprintln!("usage: webplab LIST SETTINGS [--fine]");
        process::exit(2);
    }
    let settings: Vec<&str> = args[1].split(',').collect();
    if let Some(bad) = settings.iter().find(|s| setting(s).is_none()) {
        eprintln!("webplab: unknown setting {bad}");
        process::exit(2);
    }
    let list = fs::read_to_string(args[0]).unwrap();
    let jobs: Vec<(String, f64)> = list
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (path, target) = l.split_once('\t').unwrap();
            (path.to_string(), target.trim().parse().unwrap())
        })
        .collect();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Vec<String>> = jobs
        .par_iter()
        .map(|(path, target)| {
            let lines = run(path, *target, &settings, fine).unwrap_or_else(|e| vec![format!("{path}\terror\t{e}")]);
            eprintln!("[{}/{}] {path}", done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, jobs.len());
            lines
        })
        .collect();
    for line in results.iter().flatten() {
        println!("{line}");
    }
}
