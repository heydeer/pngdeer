//! The search: the smallest palette PNG of one image whose SSIMULACRA2 score reaches the target. The
//! source's reference pyramid is built once, each palette size is quantized once and only remapped
//! per variant, and candidates are scored in memory (fast-ssim2).
//!
//! For each variant (dithering level, optional neighbour smoothing), palette sizes 2–256 are bisected in
//! log space for the fewest colors that still reach the target; the smallest of those wins. If no
//! variant reaches it, the smallest output within MISS_SLACK of the best reachable score wins.
//!
//! Without Settings::target (auto mode) there is no fixed target. TinyPNG's quality per image tracks what a plain
//! 256-color palette gives it (REFERENCE: correlation 0.96 / 0.92 on the benchmark's two image
//! sets), so the target is that plain palette's score, less REFERENCE_OFFSET, at most AUTO_CAP:
//! palette-friendly images stay near-lossless and photos settle where palettes can, and the search
//! then reaches that quality with fewer bytes. See `reference` for the exceptions.
//!
//! With Settings::preview_pixels, a large image is searched on a downscaled preview instead (box
//! averaged, or point sampled if it has at most 256 colors, which keeps a palette image's grain): the
//! preview picks a palette size per variant, the variants are ranked by their (roughly encoded)
//! full-size files, and the first is checked at full size and stepped toward the target in at most
//! VERIFY_STEPS full-size scorings, a spare one going to the runner-up variant. If the
//! target is out of reach even at 256 colors, the miss rule runs on full-size scores. Scoring
//! dominates the cost, so this trades about 1.3% in file size for several times the speed where it
//! matters most: big images in a single-threaded browser.
//!
//! An animation (`run_animation`) is searched as all its frames at once, on one palette (a palette
//! APNG has one PLTE for every frame): palettes are built from every frame, each frame is remapped
//! on its own, and where a pixel's source is unchanged from the frame before it keeps that frame's
//! index, so dithering stays still where the picture does (and the frame's changed rectangle stays
//! small). A candidate's score is its worst frame's; its size is the APNG's (apng::encode). Its
//! preview is a few of its frames (`animation_preview`), downscaled only if they are large: the ones
//! a plain 256-color palette serves worst, where the target is hardest to reach (evenly spaced
//! frames could miss them, and the checks then overshoot the target as they step up).

use std::borrow::Cow;
use std::collections::HashMap;

use fast_ssim2::{PixelDescriptor, PixelSlice, Ssimulacra2Config, Ssimulacra2Reference};
use rgb::ComponentBytes;

use crate::apng::{self, Timing};
use crate::codec::{encode_palette, palette_size_hint};
use crate::palette::{self, fits_palette, refine_palette, wu_palette};
use crate::remap::{prefer_neighbours, remap};
use crate::{par, Settings, RGBA};

/// Also how close to the target a candidate counts when the target is only just in reach (at 256
/// colors, or out of full-size checks), so the smallest file wins: the extras are dithered, and
/// reaching the last fraction of a point with them can take 19% more bytes (tinypng@2x).
const MISS_SLACK: f64 = 0.5;
const LOW: u32 = 2;
const HIGH: u32 = 256;
/// Scores this high are exact copies of the source (an image of at most 256 colors, unchanged).
const EXACT: f64 = 99.99;
/// Auto mode's reference stays within this many points of the best the search reaches at 256 colors.
const TOP_SLACK: f64 = 3.0;
/// A full-size check that falls short steps up to at most this many times its palette size.
const MAX_STEP_UP: u32 = 3;
/// An animation's frames are scored several at once (on all threads), as many as fit in this many
/// scored pixels: each scoring's buffers take about 300 bytes a pixel at their peak.
const PARALLEL_SCORED_PIXELS: usize = 2_500_000;
/// An animation's preview search runs on at most this many of its frames, evenly spaced.
const PREVIEW_FRAMES: usize = 8;
/// Images whose longer side is below this are scored enlarged (nearest neighbour, a power of two, at
/// most 8×): SSIMULACRA2 looks at several downscaled levels, and on a 72 px icon there is little left
/// to compare. bench.py scores the same way.
pub const SMALL_SIDE: usize = 256;
/// Full-size scorings to spend after a preview search (the miss rule may add up to one per variant).
const VERIFY_STEPS: u32 = 3;
/// Added to the target for the preview search, since downscaling hides some damage.
const PREVIEW_MARGIN: f64 = 2.0;
/// Auto mode aims no higher than this: 90 is indistinguishable at 1:1.
pub(crate) const AUTO_CAP: f64 = 90.0;
/// Above this many scored pixels the reference keeps only the source between checks (fast-ssim2's
/// lean reference): about half the memory at the peak, slower checks, the same scores. It is what
/// lets wasm32's 4 GB hold images of 40 MP (web/worker.js).
const LEAN_PIXELS: usize = 8_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Variant {
    pub dither: u32, // percent
    pub rdo: Option<u32>,
    pub source: Source,
}

/// Where a candidate's palette comes from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// palette::SEARCH (error feedback).
    Search,
    /// palette::PLAIN, without the error-feedback second pass.
    Plain,
    /// The plain palette refined (palette::refine_palette).
    Refined,
    /// Wu's quantizer and online k-means (palette::wu_palette); opaque images only.
    Wu,
}

impl Variant {
    pub fn label(&self) -> String {
        let (prefix, suffix) = match self.source {
            Source::Search => ("d", ""),
            Source::Plain => ("p", ""),
            Source::Refined => ("p", "k"),
            Source::Wu => ("w", ""),
        };
        format!("{prefix}{}{}{suffix}", self.dither, self.rdo.map(|r| format!("r{r}")).unwrap_or_default())
    }
}

/// Extra candidates at 256 colors, tried when the target is out of the variants' reach there (the
/// plain one is already scored in auto mode): full dithering on the search's palette, on a plain
/// one, and on the plain one refined. On smooth 3D renders and illustrations they reach up to 3
/// points more than the variants (a plain palette doesn't spend entries on the rare colors error
/// feedback protects for UI; the refined one serves the worst-matched colors better).
const FULL_DITHER: Variant = Variant { dither: 100, rdo: None, source: Source::Search };
const PLAIN: Variant = Variant { dither: 100, rdo: None, source: Source::Plain };
const REFINED: Variant = Variant { dither: 100, rdo: None, source: Source::Refined };
const EXTRAS: [Variant; 3] = [FULL_DITHER, PLAIN, REFINED];
/// palette::refine_palette settings for REFINED, from palette_lab on the 14 hard images (FINDINGS.md
/// round 9).
const REFINE_ITERATIONS: usize = 5;
const REFINE_POWER: f64 = 3.0;
/// The most an extra may add to a file, per point it gains toward the target, to be picked over the
/// variants' best at 256 colors: renal-corpuscle gains 2.2 points for 6% (taken), tinypng@2x 1.1
/// for 30% (not).
const EXTRA_COST: f64 = 0.15;
/// Auto mode's reference: the plain palette half-dithered, less REFERENCE_OFFSET, which puts it
/// where libimagequant's plain palette (speed 4, full dithering) used to be against TinyPNG's score
/// (FINDINGS.md round 13): about half a point above it on the median image.
const REFERENCE: Variant = Variant { dither: 50, rdo: None, source: Source::Plain };
const REFERENCE_OFFSET: f64 = 1.4;

/// The variants tried at each palette size: half and no dithering, no dithering with neighbour
/// smoothing (remap::prefer_neighbours, 2 levels), and Wu's palette undithered (opaque images
/// only; it suits photos). "No dithering" still dithers smooth gradients (remap::SMOOTH_DITHER).
pub const DEFAULT_VARIANTS: [Variant; 4] = [
    Variant { dither: 50, rdo: None, source: Source::Search },
    Variant { dither: 0, rdo: None, source: Source::Search },
    Variant { dither: 0, rdo: Some(2), source: Source::Search },
    Variant { dither: 0, rdo: None, source: Source::Wu },
];

/// "50,0,0r2" -> dithering 50%, 0%, and 0% with neighbour smoothing 2; "p100" -> plain palette, full
/// dithering; "w0" -> Wu's palette, no dithering (skipped on images with transparency).
pub fn parse_variants(spec: &str) -> Result<Vec<Variant>, String> {
    spec.split(',')
        .map(|item| {
            let item = item.trim();
            let (source, item) = match item.as_bytes().first() {
                Some(b'p') => (Source::Plain, &item[1..]),
                Some(b'w') => (Source::Wu, &item[1..]),
                _ => (Source::Search, item),
            };
            let (d, r) = item.split_once('r').map_or((item, None), |(d, r)| (d, Some(r)));
            let dither: u32 = d.parse().map_err(|_| format!("bad variant: {item}"))?;
            let rdo = r.map(|r| r.parse::<u32>().map_err(|_| format!("bad variant: {item}"))).transpose()?;
            if dither > 100 {
                return Err(format!("bad variant: {item}"));
            }
            Ok(Variant { dither, rdo, source })
        })
        .collect()
}

pub struct Report {
    /// The target searched for (Settings::target, or what auto mode chose).
    pub target: f64,
    pub colors: u32,
    pub variant: Variant,
    pub score: f64,
    pub evaluations: usize,
    /// Full-size evaluations after a preview search (0 without a preview).
    pub checks: usize,
    pub miss: bool,
    /// Downscale factor of the preview the search ran on; 1 when it ran at full size.
    pub preview: usize,
    /// The chosen candidate's score on the preview (same as `score` without a preview).
    pub preview_score: f64,
    /// Animation frames (1 for a still image); an animation's scores are its worst frame's.
    pub frames: usize,
    /// How many of an animation's frames its preview search ran on (0 without a preview).
    pub preview_frames: usize,
    pub profile: Profile,
}

/// Where the time went, in milliseconds (zeros when Settings::clock is None).
#[derive(Default, Clone, Copy)]
pub struct Profile {
    pub reference: f64,
    pub quantize: f64,
    pub remap: f64,
    pub score: f64,
    pub encode: f64,
    /// oxipng on the final file (pngdeer::compress).
    pub optimize: f64,
}

impl Profile {
    fn plus(self, o: Profile) -> Profile {
        Profile {
            reference: self.reference + o.reference,
            quantize: self.quantize + o.quantize,
            remap: self.remap + o.remap,
            score: self.score + o.score,
            encode: self.encode + o.encode,
            optimize: self.optimize + o.optimize,
        }
    }
}

impl Report {
    pub fn json(&self) -> String {
        let p = &self.profile;
        let frames = if self.frames > 1 { format!(r#", "frames": {}, "preview_frames": {}"#, self.frames, self.preview_frames) } else { String::new() };
        format!(
            r#"{{"target": {:.2}, "colors": {}, "variant": "{}", "score": {:.3}, "evals": {}, "checks": {}, "miss": {}, "preview": {}, "preview_score": {:.3}{}, "ms": {{"reference": {:.0}, "quantize": {:.0}, "remap": {:.0}, "score": {:.0}, "encode": {:.0}, "optimize": {:.0}}}}}"#,
            self.target,
            self.colors,
            self.variant.label(),
            self.score,
            self.evaluations,
            self.checks,
            self.miss,
            self.preview,
            self.preview_score,
            frames,
            p.reference,
            p.quantize,
            p.remap,
            p.score,
            p.encode,
            p.optimize
        )
    }
}

struct Evaluation {
    score: Option<f64>, // None until scored: a preview search ranks some candidates by size alone
    palette: Vec<RGBA>,
    indices: Vec<u8>,
    png: Option<Vec<u8>>, // encoded only for the few candidates whose size decides the winner
}

type BoxError = Box<dyn std::error::Error>;

fn enlargement(w: usize, h: usize) -> usize {
    let mut factor = 1;
    while w.max(h) * factor < SMALL_SIDE && factor < 8 {
        factor *= 2;
    }
    factor
}

/// One-off score of `distorted` against `source` (pngdeer --score), with the same small-image rule.
pub fn score(source: &[RGBA], distorted: &[RGBA], w: usize, h: usize) -> Result<f64, BoxError> {
    Scorer::new(source, w, h)?.score(distorted)
}

/// Scores any number of versions of one image against it as the search does (small images
/// enlarged, a lean reference for large ones), with its reference built once: WebP's candidates.
pub struct Scorer {
    reference: Ssimulacra2Reference,
    w: usize,
    h: usize,
    factor: usize,
}

impl Scorer {
    pub fn new(source: &[RGBA], w: usize, h: usize) -> Result<Self, BoxError> {
        let factor = enlargement(w, h);
        let bytes = enlarged_bytes(source, w, h, factor);
        let reference = new_reference(&slice(&bytes, w * factor, h * factor, is_opaque(source))?)?;
        Ok(Scorer { reference, w, h, factor })
    }

    /// The pixels each scoring compares: the image's, enlarged if small.
    pub fn scored_pixels(&self) -> usize {
        self.w * self.h * self.factor * self.factor
    }

    pub fn score(&self, distorted: &[RGBA]) -> Result<f64, BoxError> {
        let (w, h) = (self.w * self.factor, self.h * self.factor);
        let bytes = enlarged_bytes(distorted, self.w, self.h, self.factor);
        Ok(self.reference.compare_with_config(&slice(&bytes, w, h, is_opaque(distorted))?, &ssim_config())?)
    }
}

fn is_opaque(pixels: &[RGBA]) -> bool {
    pixels.iter().all(|p| p.a == 255)
}

fn new_reference(source: &PixelSlice<'_>) -> Result<Ssimulacra2Reference, BoxError> {
    let pixels = source.width() as usize * source.rows() as usize;
    reference_of(source, pixels > LEAN_PIXELS)
}

fn reference_of(source: &PixelSlice<'_>, lean: bool) -> Result<Ssimulacra2Reference, BoxError> {
    Ok(if lean { Ssimulacra2Reference::new_lean_with_config(source, &ssim_config())? } else { Ssimulacra2Reference::new_with_config(source, &ssim_config())? })
}

pub fn run(pixels: &[RGBA], w: usize, h: usize, options: &Settings) -> Result<(Vec<u8>, Report), BoxError> {
    run_frames(pixels, w, h, None, options)
}

/// `run` for an animation: `frames` canvases of `w` × `h` in `pixels`, written as an APNG with
/// `timing` (see the module docs).
pub fn run_animation(pixels: &[RGBA], w: usize, h: usize, frames: usize, timing: &Timing, options: &Settings) -> Result<(Vec<u8>, Report), BoxError> {
    run_frames(pixels, w, h, Some((frames, timing)), options)
}

fn run_frames(pixels: &[RGBA], w: usize, h: usize, animation: Option<(usize, &Timing)>, options: &Settings) -> Result<(Vec<u8>, Report), BoxError> {
    // Wu's palettes (wN) model opaque colors only
    let opaque = pixels.iter().all(|p| p.a == 255);
    let variants: Vec<Variant> = options.variants.iter().copied().filter(|v| opaque || v.source != Source::Wu).collect();
    let variants = if variants.is_empty() { options.variants.clone() } else { variants };
    let (k, picked) = match animation {
        None => (preview_factor(w, h, options.preview_pixels), None),
        Some((frames, _)) => animation_preview(w, h, frames, options.preview_pixels),
    };
    if k == 1 && picked.is_none() {
        let mut search = Search::new(pixels, w, h, animation, options);
        let target = match options.target {
            Some(target) => target,
            None => {
                let plain = search.plain_score()?;
                reference(plain, search.top(&variants)?.0).min(AUTO_CAP)
            }
        };
        let ((colors, variant), miss) = search.best(target, &variants)?;
        let png = search.encoded((colors, variant)).to_vec();
        let score = search.evaluate(colors, variant)?;
        let report = Report {
            target,
            colors,
            variant,
            score,
            evaluations: search.scored().count(),
            checks: 0,
            miss,
            preview: 1,
            preview_score: score,
            frames: animation.map_or(1, |(frames, _)| frames),
            preview_frames: 0,
            profile: search.profile,
        };
        return Ok((png, report));
    }

    // An image of at most 256 colors is often a palette PNG already, dithered: averaging would blur
    // the dithering away and invent colors it doesn't have, and the preview would mislead the
    // search (it then never tries smoothing, the usual winner). Sampling keeps its colors and grain.
    let mut full = Search::new(pixels, w, h, animation, options);
    let picked = match picked {
        Some(even) => {
            full.evaluate(HIGH, REFERENCE)?; // auto mode's reference anyway
            let mut order: Vec<usize> = (0..full.frames).collect();
            order.sort_by(|&a, &b| full.frame_scores[a].total_cmp(&full.frame_scores[b]).then(a.cmp(&b)));
            let mut worst = order[..even.len()].to_vec();
            worst.sort_unstable();
            Some(worst)
        }
        None => None,
    };
    let sample = fits_palette(pixels);
    let (small, sw, sh) = match &picked {
        None => downscale(pixels, w, h, k, sample),
        Some(frames) => {
            let n = w * h;
            let parts: Vec<_> = frames.iter().map(|&f| downscale(&pixels[f * n..(f + 1) * n], w, h, k, sample)).collect();
            let (sw, sh) = (parts[0].1, parts[0].2);
            (parts.into_iter().flat_map(|p| p.0).collect(), sw, sh)
        }
    };
    // the preview's timing only sizes its candidates, to rank them
    let preview_timing = picked.as_ref().map(|frames| Timing { delays: vec![(1, 10); frames.len()], plays: 0, default_image: false });
    let mut preview = Search::new(&small, sw, sh, picked.as_ref().map(|f| f.len()).zip(preview_timing.as_ref()), options);
    let target = match options.target {
        Some(target) => target,
        // One full-size scoring, the plain palette's; the search's own best at 256 colors (which
        // matters only when far above it) is estimated by their distance on the preview. Unless the
        // plain palette copies the image: then that best is the reference, so it is scored instead.
        None => {
            let plain = full.plain_score()?;
            let (preview_top, variant) = preview.top(&variants)?;
            let top = if plain < EXACT { plain + preview_top - preview.plain_score()? } else { full.evaluate(HIGH, variant)? };
            reference(plain, top).min(AUTO_CAP)
        }
    };
    // frames at full size hide nothing
    let margin = if k > 1 { PREVIEW_MARGIN } else { 0.0 };
    let (mut ranked, mut preview_miss) = preview.ranked(target + margin, &variants)?;
    if preview_miss {
        // the margin alone may put it out of reach on the preview: let full size decide
        (ranked, preview_miss) = preview.ranked(target, &variants)?;
    }
    // Dithering noise costs far more bytes at full size than on the preview, so re-rank the
    // variants by their full-size files (roughly encoded: quantizing and deflating are cheap next
    // to scoring).
    let ranked = if preview_miss { ranked } else { full.sorted_roughly(ranked)? };
    let mut checks = VERIFY_STEPS;
    let mut found = Vec::new();
    let mut topped = preview_miss; // the target looks out of reach even at 256 colors
    if !preview_miss {
        for (i, &start) in ranked.iter().enumerate() {
            if i > 0 && start.1.source == Source::Wu {
                continue; // a full-size k-means just to fall short again
            }
            let (hit, optimism) = full.verify(&mut preview, start, target, &mut checks)?;
            if let Some(colors) = hit {
                found.push((colors, start.1));
                // That ranking is rough (sizes at the preview's palette sizes), so a spare check
                // goes to the runner-up, aimed with the gap the first variant measured.
                if let (0, Some(&(_, runner_up)), true) = (i, ranked.get(1), checks > 0) {
                    if let Some(colors) = preview.min_colors(target + optimism, runner_up, Some(colors))? {
                        if full.evaluate(colors, runner_up)? >= target {
                            found.push((colors, runner_up));
                        }
                    }
                }
                break;
            }
            topped = full.scored().any(|key| key == (HIGH, start.1));
            if !topped {
                break; // out of checks short of the target
            }
            // this variant can't reach it even at 256 colors: try the next, with a couple of checks
            checks = checks.max(2);
        }
    }
    let (colors, variant) = if found.is_empty() && topped {
        full.at_top(&variants, target)?.0[0]
    } else if found.is_empty() {
        // out of checks short of the target: the smallest file within MISS_SLACK of it, else the best
        // score seen (an extra only if worth it)
        let tried: Vec<(u32, Variant)> = full.scored().filter(|k| !EXTRAS.contains(&k.1)).collect();
        let near: Vec<(u32, Variant)> = tried.iter().copied().filter(|k| full.evaluations[k].score.is_some_and(|s| s >= target - MISS_SLACK)).collect();
        if near.is_empty() {
            let best = *tried.iter().max_by(|a, b| full.evaluations[a].score.partial_cmp(&full.evaluations[b].score).unwrap()).unwrap();
            full.upgrade(best, target, false)?
        } else {
            full.sorted(near)?[0]
        }
    } else {
        found.extend(full.extras_reaching(target));
        full.sorted(found)?[0]
    };
    let score = full.evaluate(colors, variant)?;
    let preview_score = preview.evaluate(colors, variant)?; // already scored on the way here
    let png = full.encoded((colors, variant)).to_vec();
    let report = Report {
        target,
        colors,
        variant,
        score,
        evaluations: preview.scored().count() + full.scored().count(),
        checks: full.scored().count(),
        miss: score < target,
        preview: k,
        preview_score,
        frames: animation.map_or(1, |(frames, _)| frames),
        preview_frames: picked.map_or(0, |f| f.len()),
        profile: preview.profile.plus(full.profile),
    };
    Ok((png, report))
}

/// Auto mode's reference score from a plain 256-color palette's and the best of the search's own at
/// 256 colors. Usually the plain one, but at most TOP_SLACK under the search's own: on some images
/// (line art with transparency, for one) a plain palette fell 10+ points behind both TinyPNG and
/// this search (with libimagequant's; 3 rather than 4 also lifted stripe@2x past TinyPNG for 3% more
/// bytes). An image the plain palette copies unchanged (at most 256 colors) gets the search's own
/// best that isn't a copy.
fn reference(plain: f64, top: f64) -> f64 {
    if plain >= EXACT {
        top
    } else {
        plain.max(top - TOP_SLACK)
    }
}

/// An animation's preview: its downscale factor and how many frames it runs on (up to
/// PREVIEW_FRAMES; `run_frames` picks which), so that together they fit `budget` pixels where they
/// can without a frame's longer side going under SMALL_SIDE (it would be scored enlarged again).
/// (1, None) for an animation small enough to search whole, or of too few frames for a few of them
/// to save much when they aren't downscaled.
fn animation_preview(w: usize, h: usize, frames: usize, budget: usize) -> (usize, Option<Vec<usize>>) {
    if budget == 0 || w * h * frames <= budget || frames <= 2 {
        return (1, None);
    }
    let count = frames.min(PREVIEW_FRAMES);
    let picked: Vec<usize> = (0..count).map(|j| j * (frames - 1) / (count - 1)).collect();
    let mut k = preview_factor(w, h, budget / count);
    while k > 1 && w.max(h) / k < SMALL_SIDE {
        k -= 1;
    }
    if k == 1 && frames < 2 * PREVIEW_FRAMES {
        return (1, None);
    }
    (k, Some(picked))
}

/// Smallest k >= 2 whose k×k box downscale fits in `budget` pixels; 1 if the image already does.
fn preview_factor(w: usize, h: usize, budget: usize) -> usize {
    if budget == 0 || w * h <= budget {
        return 1;
    }
    (2..).find(|k| (w / k) * (h / k) <= budget).unwrap()
}

/// k×k box average, weighted by alpha so transparent pixels don't darken edges, or with `sample`
/// the centre pixel of each block. Drops the remainder.
fn downscale(pixels: &[RGBA], w: usize, h: usize, k: usize, sample: bool) -> (Vec<RGBA>, usize, usize) {
    let (sw, sh) = (w / k, h / k);
    if sample {
        let out = (0..sh).flat_map(|y| (0..sw).map(move |x| pixels[(y * k + k / 2) * w + x * k + k / 2])).collect();
        return (out, sw, sh);
    }
    let mut out = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..k {
                for p in &pixels[(y * k + dy) * w + x * k..(y * k + dy) * w + x * k + k] {
                    let pa = p.a as u32;
                    r += p.r as u32 * pa;
                    g += p.g as u32 * pa;
                    b += p.b as u32 * pa;
                    a += pa;
                }
            }
            // fully transparent blocks come out as (0, 0, 0, 0)
            let mean = |sum: u32| sum.checked_div(a).unwrap_or(0) as u8;
            out.push(RGBA::new(mean(r), mean(g), mean(b), (a / (k * k) as u32) as u8));
        }
    }
    (out, sw, sh)
}

struct Search<'p> {
    /// The image, or an animation's frames one after another, each `w` × `h`.
    pixels: &'p [RGBA],
    w: usize,
    h: usize,
    frames: usize,
    /// An animation's timing, for its file (None for a still image).
    timing: Option<&'p Timing>,
    factor: usize,
    opaque: bool,
    reference: Option<Ssimulacra2Reference>,
    /// An animation's references, one per frame.
    frame_references: Vec<Ssimulacra2Reference>,
    /// Every frame's score in the latest animation scoring.
    frame_scores: Vec<f64>,
    /// Palettes by size and source.
    palettes: HashMap<(u32, Source), Vec<RGBA>>,
    evaluations: HashMap<(u32, Variant), Evaluation>,
    clock: Option<fn() -> f64>,
    profile: Profile,
}

impl<'p> Search<'p> {
    fn new(pixels: &'p [RGBA], w: usize, h: usize, animation: Option<(usize, &'p Timing)>, options: &Settings) -> Self {
        Search {
            pixels,
            w,
            h,
            frames: animation.map_or(1, |(frames, _)| frames),
            timing: animation.map(|(_, timing)| timing),
            factor: enlargement(w, h),
            opaque: is_opaque(pixels),
            reference: None,
            frame_references: Vec::new(),
            frame_scores: Vec::new(),
            palettes: HashMap::new(),
            evaluations: HashMap::new(),
            clock: options.clock,
            profile: Profile::default(),
        }
    }

    fn tick(&self) -> f64 {
        self.clock.map_or(0.0, |c| c())
    }

    /// The best (colors, variant) by the rules in the module docs, and whether the target was missed.
    fn best(&mut self, target: f64, variants: &[Variant]) -> Result<((u32, Variant), bool), BoxError> {
        let (mut ranked, miss) = self.ranked(target, variants)?;
        if !miss && !EXTRAS.contains(&ranked[0].1) {
            // a variant reaches the target: an extra that does too only wins by being smaller
            ranked.extend(self.extras_reaching(target));
            ranked = self.sorted(ranked)?;
        }
        Ok((ranked[0], miss))
    }

    /// Every variant's fewest colors reaching the target, smallest file first; if none reaches it,
    /// the MISS_SLACK candidates at 256 colors instead, and true.
    fn ranked(&mut self, target: f64, variants: &[Variant]) -> Result<(Vec<(u32, Variant)>, bool), BoxError> {
        let mut found = Vec::new();
        let mut guess = None;
        for &variant in variants {
            if let Some(colors) = self.min_colors(target, variant, guess)? {
                found.push((colors, variant));
                guess = Some(colors);
            }
        }
        if found.is_empty() {
            self.at_top(variants, target)
        } else {
            Ok((self.sorted(found)?, false))
        }
    }

    /// Auto mode's reference score: a plain 256-color palette's (REFERENCE), less REFERENCE_OFFSET
    /// unless it copies the image exactly.
    fn plain_score(&mut self) -> Result<f64, BoxError> {
        let score = self.evaluate(HIGH, REFERENCE)?;
        Ok(if score >= EXACT { score } else { score - REFERENCE_OFFSET })
    }

    /// Already-scored extra candidates (EXTRAS at 256 colors) reaching `target`.
    fn extras_reaching(&self, target: f64) -> Vec<(u32, Variant)> {
        EXTRAS.into_iter().map(|v| (HIGH, v)).filter(|k| self.evaluations.get(k).and_then(|e| e.score).is_some_and(|s| s >= target)).collect()
    }

    /// The best score at 256 colors over `variants`, and its variant, passing over exact copies of
    /// the source unless every variant gives one.
    fn top(&mut self, variants: &[Variant]) -> Result<(f64, Variant), BoxError> {
        let mut scores = Vec::new();
        for &variant in variants {
            scores.push((self.evaluate(HIGH, variant)?, variant));
        }
        let best = |it: &mut dyn Iterator<Item = &(f64, Variant)>| it.max_by(|a, b| a.0.total_cmp(&b.0)).copied();
        Ok(best(&mut scores.iter().filter(|s| s.0 < EXACT)).or(best(&mut scores.iter())).unwrap())
    }

    /// The pick at 256 colors when fewer colors won't do: the variants' (the smallest file within
    /// MISS_SLACK of `target`, else the ceiling), or an extra worth it (see `upgrade`). True if it
    /// falls short of the target.
    fn at_top(&mut self, variants: &[Variant], target: f64) -> Result<(Vec<(u32, Variant)>, bool), BoxError> {
        // Wu's variants are left out at 256 colors: a full-size k-means each, rarely better there
        // than the extras
        let others: Vec<Variant> = variants.iter().copied().filter(|v| v.source != Source::Wu).collect();
        let variants = if others.is_empty() { variants } else { &others[..] };
        let near: Vec<(u32, Variant)> = variants.iter().map(|&v| (HIGH, v)).collect();
        for &(c, v) in &near {
            self.evaluate(c, v)?;
        }
        let near: Vec<(u32, Variant)> = near.into_iter().filter(|k| self.evaluations[k].score.is_some_and(|s| s >= target - MISS_SLACK)).collect();
        if !near.is_empty() {
            return Ok((self.sorted(near)?, false));
        }
        let ceiling = self.ceiling(variants)?[0];
        let pick = self.upgrade(ceiling, target, true)?;
        Ok((vec![pick], self.evaluate(pick.0, pick.1)? < target - MISS_SLACK))
    }

    /// `pick`, or the highest-scoring extra at 256 colors whose extra bytes stay within EXTRA_COST
    /// per point it gains toward `target` (points past the target don't count toward the allowance,
    /// but among extras that qualify the best score wins). With `evaluate` false only extras already
    /// scored are considered.
    fn upgrade(&mut self, pick: (u32, Variant), target: f64, evaluate: bool) -> Result<(u32, Variant), BoxError> {
        let (score, bytes) = (self.evaluate(pick.0, pick.1)?, self.encoded(pick).len() as f64);
        let mut best = (pick, score);
        // The extras are all fully dithered and come out about the same size, so PLAIN (already
        // scored in auto mode) goes first: over the largest allowance, it rules out the others
        // without making them (each is a full-size quantization and scoring: 2 s on a 5 MP image
        // in the browser).
        let most = bytes * (1.0 + EXTRA_COST * (target - score).max(0.0));
        let mut hopeless = false;
        for extra in [PLAIN, FULL_DITHER, REFINED].map(|v| (HIGH, v)) {
            if (!evaluate && !self.is_scored(extra)) || hopeless {
                continue;
            }
            if extra.1 == PLAIN {
                self.evaluate(extra.0, extra.1)?;
                hopeless = self.encoded(extra).len() as f64 > most;
            }
            let extra_score = self.evaluate(extra.0, extra.1)?;
            let gain = extra_score.min(target) - score;
            if gain > 0.0 && extra_score > best.1 && self.encoded(extra).len() as f64 <= bytes * (1.0 + EXTRA_COST * gain) {
                best = (extra, extra_score);
            }
        }
        Ok(best.0)
    }

    /// Every variant at 256 colors within MISS_SLACK of the best score among them, smallest file first.
    fn ceiling(&mut self, variants: &[Variant]) -> Result<Vec<(u32, Variant)>, BoxError> {
        let mut tops = Vec::new();
        for &variant in variants {
            tops.push(((HIGH, variant), self.evaluate(HIGH, variant)?));
        }
        let best = tops.iter().map(|&(_, score)| score).fold(f64::MIN, f64::max);
        let near = tops.into_iter().filter(|&(_, score)| score >= best - MISS_SLACK).map(|(key, _)| key).collect();
        self.sorted(near)
    }

    /// `keys` by encoded size, smallest first.
    fn sorted(&mut self, mut keys: Vec<(u32, Variant)>) -> Result<Vec<(u32, Variant)>, BoxError> {
        for &key in &keys {
            self.candidate(key)?;
        }
        keys.sort_by_cached_key(|&key| self.encoded(key).len());
        Ok(keys)
    }

    /// `keys` by palette_size_hint, smallest first.
    fn sorted_roughly(&mut self, mut keys: Vec<(u32, Variant)>) -> Result<Vec<(u32, Variant)>, BoxError> {
        for &key in &keys {
            self.candidate(key)?;
        }
        let t = self.tick();
        if self.timing.is_some() {
            keys.sort_by_cached_key(|&key| self.encoded(key).len());
        } else {
            keys.sort_by_cached_key(|key| {
                let e = &self.evaluations[key];
                palette_size_hint(&e.palette, &e.indices, self.w)
            });
        }
        self.profile.encode += self.tick() - t;
        Ok(keys)
    }

    fn is_scored(&self, key: (u32, Variant)) -> bool {
        self.evaluations.get(&key).is_some_and(|e| e.score.is_some())
    }

    /// Candidates scored so far.
    fn scored(&self) -> impl Iterator<Item = (u32, Variant)> + '_ {
        self.evaluations.iter().filter(|(_, e)| e.score.is_some()).map(|(&key, _)| key)
    }

    /// Checks `start` (a preview search's pick) at full size and steps its palette size toward the
    /// fewest colors reaching `target`, using at most `checks` new evaluations; full-size scores the
    /// variant already has count too. With only one side of the target seen, the next size comes
    /// from the preview search re-aimed by the gap just measured (it is off by a few points, but by
    /// a similar amount for nearby sizes); with both, from interpolating the full-size scores.
    /// Returns the fewest colors seen to reach the target, if any, and the last gap (preview score
    /// minus full-size score).
    fn verify(&mut self, preview: &mut Search, start: (u32, Variant), target: f64, checks: &mut u32) -> Result<(Option<u32>, f64), BoxError> {
        let (mut colors, variant) = start;
        let (mut hit, mut short) = (None::<(u32, f64)>, None::<(u32, f64)>);
        // keeps the fewest colors seen to reach the target and the most seen to fall short
        let note = |hit: &mut Option<(u32, f64)>, short: &mut Option<(u32, f64)>, c: u32, score: f64| {
            if score >= target {
                *hit = Some(hit.filter(|&(h, _)| h < c).unwrap_or((c, score)));
            } else {
                *short = Some(short.filter(|&(s, _)| s > c).unwrap_or((c, score)));
            }
        };
        let known: Vec<u32> = self.scored().filter(|k| k.1 == variant).map(|k| k.0).collect();
        for c in known {
            note(&mut hit, &mut short, c, self.evaluate(c, variant)?);
        }
        let mut gap = 0.0;
        loop {
            if !self.is_scored((colors, variant)) {
                if *checks == 0 {
                    break;
                }
                *checks -= 1;
            }
            let score = self.evaluate(colors, variant)?;
            gap = preview.evaluate(colors, variant)? - score;
            note(&mut hit, &mut short, colors, score);
            let next = match (hit, short) {
                (Some((h, hs)), Some((s, ss))) => {
                    if h <= s || h - s <= (h / 8).max(2) {
                        break;
                    }
                    let along = ((target - ss) / (hs - ss)).clamp(0.0, 1.0);
                    let c = libm::exp(libm::log(s as f64) + along * (libm::log(h as f64) - libm::log(s as f64))).round() as u32;
                    c.clamp(s + 1, h - 1)
                }
                (Some((h, _)), None) => {
                    let c = preview.min_colors(target + gap, variant, Some(h))?.unwrap_or(HIGH);
                    if c + (h / 8).max(2) > h {
                        break;
                    }
                    c
                }
                (None, Some((s, _))) => {
                    if s >= HIGH {
                        break;
                    }
                    // A large shortfall's gap overstates the gap higher up, so the step is capped
                    let c = preview.min_colors(target + gap, variant, Some(s))?.unwrap_or(HIGH);
                    c.min(s * MAX_STEP_UP).max(s * 23 / 20).max(s + 2).min(HIGH)
                }
                (None, None) => unreachable!(),
            };
            if self.is_scored((next, variant)) {
                break; // nothing new to learn there
            }
            colors = next;
        }
        Ok((hit.map(|(h, _)| h), gap))
    }

    /// SSIMULACRA2 of one candidate (cached).
    fn evaluate(&mut self, colors: u32, variant: Variant) -> Result<f64, BoxError> {
        let key = (colors, variant);
        self.candidate(key)?;
        if let Some(score) = self.evaluations[&key].score {
            return Ok(score);
        }
        let mut e = self.evaluations.remove(&key).unwrap();
        let score = self.score_of(&e.palette, &e.indices);
        if self.frames > 1 && e.png.is_none() {
            // an animation's indices are frames × pixels: remapped again if it is ever encoded
            e.indices = Vec::new();
        }
        self.evaluations.insert(key, Evaluation { score: score.as_ref().ok().copied(), ..e });
        score
    }

    /// SSIMULACRA2 of a remapped image against the source (an animation's: its worst frame's).
    fn score_of(&mut self, palette: &[RGBA], indices: &[u8]) -> Result<f64, BoxError> {
        if self.frames > 1 {
            return self.frames_score_of(palette, indices);
        }
        let (w, h) = (self.w * self.factor, self.h * self.factor);
        if self.reference.is_none() {
            let t = self.tick();
            let source = enlarged_bytes(self.pixels, self.w, self.h, self.factor);
            self.reference = Some(new_reference(&slice(&source, w, h, self.opaque)?)?);
            self.profile.reference += self.tick() - t;
        }
        let t = self.tick();
        let remapped: Vec<RGBA> = indices.iter().map(|&i| palette[i as usize]).collect();
        let bytes = enlarged_bytes(&remapped, self.w, self.h, self.factor);
        // Whole-image scoring (multi-threaded through fast-ssim2's rayon feature); its strip mode
        // is no faster here and drifts from libjxl's scores by up to 2.4 points.
        let reference = self.reference.as_ref().unwrap();
        let score = reference.compare_with_config(&slice(&bytes, w, h, is_opaque(palette))?, &ssim_config())?;
        self.profile.score += self.tick() - t;
        Ok(score)
    }

    /// `score_of` for an animation: every frame against its source, the lowest score. A frame
    /// whose source and indices repeat the frame before's scores the same, so isn't scored again.
    fn frames_score_of(&mut self, palette: &[RGBA], indices: &[u8]) -> Result<f64, BoxError> {
        let (n, w, h) = (self.w * self.h, self.w * self.factor, self.h * self.factor);
        let pixels = self.pixels;
        let source = |f: usize| &pixels[f * n..(f + 1) * n];
        if self.frame_references.is_empty() {
            let t = self.tick();
            // full references for all frames would take several times the memory of the frames
            let lean = w * h * self.frames > LEAN_PIXELS;
            let (sw, sh, factor) = (self.w, self.h, self.factor);
            let opaque = self.opaque;
            let built = par::map(&(0..self.frames).collect::<Vec<_>>(), |&f| {
                let bytes = enlarged_bytes(source(f), sw, sh, factor);
                slice(&bytes, w, h, opaque).and_then(|s| reference_of(&s, lean)).map_err(|e| e.to_string())
            });
            self.frame_references = built.into_iter().collect::<Result<_, _>>()?;
            self.profile.reference += self.tick() - t;
        }
        let t = self.tick();
        let repeats = |f: usize| f > 0 && source(f) == source(f - 1) && indices[f * n..(f + 1) * n] == indices[(f - 1) * n..f * n];
        let distinct: Vec<usize> = (0..self.frames).filter(|&f| !repeats(f)).collect();
        let opaque = is_opaque(palette);
        let (sw, sh, factor) = (self.w, self.h, self.factor);
        let references = &self.frame_references;
        let score = |&f: &usize| {
            let remapped: Vec<RGBA> = indices[f * n..(f + 1) * n].iter().map(|&i| palette[i as usize]).collect();
            let bytes = enlarged_bytes(&remapped, sw, sh, factor);
            slice(&bytes, w, h, opaque).and_then(|s| Ok(references[f].compare_with_config(&s, &ssim_config())?)).map_err(|e| e.to_string())
        };
        let at_once = (PARALLEL_SCORED_PIXELS / (w * h)).max(1);
        let scores: Vec<Result<f64, String>> = distinct.chunks(at_once).flat_map(|frames| par::map(frames, score)).collect();
        self.profile.score += self.tick() - t;
        let scores = scores.into_iter().collect::<Result<Vec<f64>, String>>()?;
        // a repeat scores what the frame before did
        let mut each = Vec::with_capacity(self.frames);
        let mut scored = distinct.iter().zip(&scores).peekable();
        for f in 0..self.frames {
            match scored.peek() {
                Some(&(&g, &score)) if g == f => {
                    each.push(score);
                    scored.next();
                }
                _ => each.push(each[f - 1]),
            }
        }
        self.frame_scores = each;
        Ok(scores.into_iter().fold(f64::INFINITY, f64::min))
    }

    /// Quantizes (once per palette size) and remaps one candidate, without scoring it.
    fn candidate(&mut self, (colors, variant): (u32, Variant)) -> Result<(), BoxError> {
        if self.evaluations.contains_key(&(colors, variant)) {
            return Ok(());
        }
        let t0 = self.tick();
        let palette_key = (colors, variant.source);
        // an animation's frames stacked into one tall image
        let all_h = self.h * self.frames;
        if !self.palettes.contains_key(&palette_key) {
            let palette = match variant.source {
                Source::Search => palette::SEARCH.palette(self.pixels, self.w, all_h, colors),
                Source::Plain => palette::PLAIN.palette(self.pixels, self.w, all_h, colors),
                Source::Refined => {
                    let base = match self.palettes.get(&(colors, Source::Plain)) {
                        Some(palette) => palette.clone(),
                        None => palette::PLAIN.palette(self.pixels, self.w, all_h, colors),
                    };
                    refine_palette(self.pixels, &base, REFINE_ITERATIONS, REFINE_POWER)
                }
                Source::Wu => wu_palette(self.pixels, colors as usize).ok_or("Wu's palette needs an opaque image")?,
            };
            self.palettes.insert(palette_key, palette);
        }
        let t1 = self.tick();
        let palette = self.palettes[&palette_key].clone();
        let indices = self.remapped(&palette, variant);
        self.profile.quantize += t1 - t0;
        self.profile.remap += self.tick() - t1;
        self.evaluations.insert((colors, variant), Evaluation { score: None, palette, indices, png: None });
        Ok(())
    }

    /// The pixels on `palette` by `variant`: an animation's frame by frame, each pixel whose source
    /// is the frame before's keeping that frame's index.
    fn remapped(&self, palette: &[RGBA], variant: Variant) -> Vec<u8> {
        let (w, h, n) = (self.w, self.h, self.w * self.h);
        let frames: Vec<&[RGBA]> = self.pixels.chunks_exact(n).collect();
        let remapped = par::map(&frames, |frame| {
            let mut indices = remap(frame, w, h, palette, variant.dither as f32 / 100.0);
            if let Some(delta) = variant.rdo {
                prefer_neighbours(frame, palette, &mut indices, w, delta);
            }
            indices
        });
        let mut indices = remapped.concat();
        for f in 1..frames.len() {
            for p in 0..n {
                if frames[f][p] == frames[f - 1][p] {
                    indices[f * n + p] = indices[(f - 1) * n + p];
                }
            }
        }
        indices
    }

    fn encoded(&mut self, key: (u32, Variant)) -> &[u8] {
        let (w, h, t0) = (self.w, self.h, self.tick());
        if self.evaluations[&key].png.is_none() {
            if self.evaluations[&key].indices.is_empty() {
                let indices = self.remapped(&self.evaluations[&key].palette, key.1);
                self.evaluations.get_mut(&key).unwrap().indices = indices;
            }
            let e = self.evaluations.get_mut(&key).unwrap();
            e.png = Some(match self.timing {
                Some(timing) => apng::encode(&e.palette, &e.indices, w, h, timing),
                None => encode_palette(&e.palette, &e.indices, w, h),
            });
            if self.frames > 1 && e.score.is_some() {
                e.indices = Vec::new();
            }
        }
        self.profile.encode += self.tick() - t0;
        self.evaluations[&key].png.as_deref().unwrap()
    }

    /// Fewest colors (within the tolerance) reaching target with this variant; None if 256 falls short.
    fn min_colors(&mut self, target: f64, variant: Variant, guess: Option<u32>) -> Result<Option<u32>, BoxError> {
        if self.evaluate(HIGH, variant)? < target {
            return Ok(None);
        }
        let (mut lo, mut hi) = (LOW, HIGH); // hi reaches the target, lo is a lower bound
        if let Some(g) = guess {
            let spread = (g / 4).max(2);
            for colors in [(g + spread).min(HIGH), g.saturating_sub(spread).max(LOW)] {
                if lo < colors && colors < hi {
                    if self.evaluate(colors, variant)? >= target {
                        hi = colors;
                    } else {
                        lo = colors;
                    }
                }
            }
        }
        while hi - lo > (hi / 8).max(2) {
            let mid = ((lo.max(1) as f64 * hi as f64).sqrt().round() as u32).clamp(lo + 1, hi - 1);
            if self.evaluate(mid, variant)? >= target {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Ok(Some(hi))
    }
}

/// The pixels as bytes, enlarged `factor` times (borrowed at 1×: a copy of a big image is a lot of
/// memory next to its scoring).
fn enlarged_bytes(pixels: &[RGBA], w: usize, h: usize, factor: usize) -> Cow<'_, [u8]> {
    if factor == 1 {
        return Cow::Borrowed(pixels.as_bytes());
    }
    let mut out = Vec::with_capacity(pixels.len() * factor * factor * 4);
    for y in 0..h * factor {
        let row = &pixels[(y / factor) * w..(y / factor + 1) * w];
        for x in 0..w * factor {
            let p = row[x / factor];
            out.extend_from_slice(&[p.r, p.g, p.b, p.a]);
        }
    }
    Cow::Owned(out)
}

/// fast-ssim2's wasm SIMD kernels measured slower than its scalar ones (Chrome/V8), so wasm builds
/// pin the scalar path; elsewhere it picks the best SIMD at runtime.
fn ssim_config() -> Ssimulacra2Config<'static> {
    if cfg!(target_arch = "wasm32") {
        Ssimulacra2Config::scalar()
    } else {
        Ssimulacra2Config::default()
    }
}

/// RGBA bytes for fast-ssim2. Opaque ones are described as RGBX: with an alpha channel it scores
/// twice, over two backgrounds, which for opaque pixels is the same computation (same bits).
fn slice(bytes: &[u8], w: usize, h: usize, opaque: bool) -> Result<PixelSlice<'_>, BoxError> {
    let format = if opaque { PixelDescriptor::RGBX8_SRGB } else { PixelDescriptor::RGBA8_SRGB };
    Ok(PixelSlice::new(bytes, w as u32, h as u32, w * 4, format)?)
}
