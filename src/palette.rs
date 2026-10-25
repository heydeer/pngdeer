//! Palettes: a weighted median cut (each box split where it most reduces the squared error) refined
//! by weighted k-means, in the color space of color.rs; plus the palettes the search tries besides
//! its own (plain, refined, Wu's) and the exact palette of an image with few colors.

use std::collections::{HashMap, HashSet};

use crate::color::{distance, to_color, to_rgba, Color, Nearest, DIM};
use crate::{par, RGBA};

/// Palettes are built from at most this many histogram entries: past it, colors that differ only in
/// their lowest bits are merged (their weighted mean), which keeps k-means quick on photos.
const MAX_ENTRIES: usize = 1 << 18;
/// Histogram bins per axis for Wu's quantizer (Oklab L, a, b), over the range sRGB colors cover
/// (scaled as in color.rs).
const WU_BINS: [usize; 3] = [32, 16, 16];
const WU_RANGES: [(f32, f32); 3] = [(0.0, 255.0), (-0.2339 * 255.0, 0.2763 * 255.0), (-0.3116 * 255.0, 0.1986 * 255.0)];
/// Pixels sampled by online k-means: past this, more barely move the centers.
const ONLINE_SAMPLES: usize = 1 << 18;

/// How a palette is built: k-means rounds, and the error-feedback gain (None: a single pass). With
/// feedback the palette is built twice: the first one, remapped without dithering, measures how far
/// each pixel lands from its color, and the second pass weighs pixels by 32 + gain × that error, so
/// rare but distinct colors (accent icons, text) get entries instead of losing them to large areas
/// of near-identical shades.
pub struct Quantizer {
    pub rounds: usize,
    pub feedback: Option<f32>,
}

/// The search's own palettes.
pub const SEARCH: Quantizer = Quantizer { rounds: 8, feedback: Some(1.0) };
/// Plain palettes: auto mode's reference and the 256-color extras (search.rs).
pub const PLAIN: Quantizer = Quantizer { rounds: 5, feedback: None };

impl Quantizer {
    /// Palette of at most `colors` entries.
    pub fn palette(&self, pixels: &[RGBA], w: usize, h: usize, colors: u32) -> Vec<RGBA> {
        let mut palette = build(pixels, None, colors as usize, self.rounds);
        if let Some(gain) = self.feedback {
            let indices = crate::remap::remap(pixels, w, h, &palette, 0.0);
            let map: Vec<u8> = pixels
                .iter()
                .zip(&indices)
                .map(|(p, &i)| {
                    let q = palette[i as usize];
                    let err = [p.r.abs_diff(q.r), p.g.abs_diff(q.g), p.b.abs_diff(q.b), p.a.abs_diff(q.a)].into_iter().max().unwrap();
                    (32.0 + gain * err as f32).min(255.0) as u8
                })
                .collect();
            palette = build(pixels, Some(&map), colors as usize, self.rounds);
        }
        palette
    }
}

/// Distinct colors (fully transparent ones as one) with their total weight.
fn histogram(pixels: &[RGBA], weights: Option<&[u8]>) -> Vec<(RGBA, f32)> {
    let mut counts: HashMap<RGBA, f32> = HashMap::new();
    let mut run: Option<(RGBA, f32)> = None;
    for (k, &p) in pixels.iter().enumerate() {
        let p = if p.a == 0 { RGBA::new(0, 0, 0, 0) } else { p };
        let w = weights.map_or(1.0, |m| m[k] as f32);
        match &mut run {
            Some((c, n)) if *c == p => *n += w,
            _ => {
                if let Some((c, n)) = run.replace((p, w)) {
                    *counts.entry(c).or_default() += n;
                }
            }
        }
    }
    if let Some((c, n)) = run {
        *counts.entry(c).or_default() += n;
    }
    counts.into_iter().filter(|&(_, n)| n > 0.0).collect()
}

/// The color with `shift` low bits dropped from r, g and b (alpha kept: opaque and translucent
/// pixels are remapped differently).
fn packed(c: &RGBA, shift: u8) -> u32 {
    u32::from_be_bytes([c.r >> shift, c.g >> shift, c.b >> shift, c.a])
}

/// Histogram counts as palette-building entries, in a fixed order; past MAX_ENTRIES, merged by
/// dropping as few low bits as gets them under it.
fn entries(mut counts: Vec<(RGBA, f32)>) -> Vec<(Color, f32)> {
    counts.sort_unstable_by_key(|(c, _)| packed(c, 0));
    if counts.len() <= MAX_ENTRIES {
        return counts.into_iter().map(|(c, n)| (to_color(c), n)).collect();
    }
    let shift = (1..8)
        .find(|&s| {
            let mut groups: Vec<u32> = counts.iter().map(|(c, _)| packed(c, s)).collect();
            groups.sort_unstable();
            groups.dedup();
            groups.len() <= MAX_ENTRIES
        })
        .unwrap_or(8);
    let mut merged: Vec<(u32, [f64; DIM], f64)> = Vec::new();
    for (c, n) in counts.iter().map(|&(c, n)| (c, n as f64)) {
        let group = if shift < 8 { packed(&c, shift) } else { 0 };
        let color = to_color(c);
        match merged.binary_search_by_key(&group, |m| m.0) {
            Ok(i) => {
                for k in 0..DIM {
                    merged[i].1[k] += n * color[k] as f64;
                }
                merged[i].2 += n;
            }
            Err(i) => merged.insert(i, (group, color.map(|v| n * v as f64), n)),
        }
    }
    merged.into_iter().map(|(_, sum, n)| (sum.map(|v| (v / n) as f32), n as f32)).collect()
}

/// Weighted mean and squared error of a set of entries.
fn stats(entries: &[(Color, f32)]) -> (Color, f64) {
    let (mut sum, mut sq, mut total) = ([0f64; DIM], 0f64, 0f64);
    for (c, w) in entries {
        let w = *w as f64;
        total += w;
        for k in 0..DIM {
            sum[k] += w * c[k] as f64;
            sq += w * (c[k] as f64).powi(2);
        }
    }
    let mean: [f64; DIM] = sum.map(|s| s / total.max(1e-9));
    let sse = sq - total * mean.iter().map(|m| m * m).sum::<f64>();
    (mean.map(|m| m as f32), sse.max(0.0))
}

/// Up to `k` box means: the box with the largest squared error splits next, along its widest
/// channel (by weighted variance), where the two halves' squared error along it is smallest.
fn median_cut(entries: &mut [(Color, f32)], k: usize) -> Vec<Color> {
    let mut boxes: Vec<(usize, usize, Color, f64)> = Vec::new();
    let (mean, sse) = stats(entries);
    boxes.push((0, entries.len(), mean, sse));
    while boxes.len() < k {
        let Some(b) = (0..boxes.len()).filter(|&b| boxes[b].1 - boxes[b].0 > 1 && boxes[b].3 > 0.0).max_by(|&a, &b| boxes[a].3.total_cmp(&boxes[b].3)) else {
            break;
        };
        let (start, end, mean, _) = boxes[b];
        let part = &mut entries[start..end];
        let mut var = [0f64; DIM];
        for (c, w) in part.iter() {
            for ch in 0..DIM {
                var[ch] += *w as f64 * (c[ch] - mean[ch]) as f64 * (c[ch] - mean[ch]) as f64;
            }
        }
        let ch = (0..DIM).max_by(|&a, &b| var[a].total_cmp(&var[b])).unwrap();
        part.sort_unstable_by(|a, b| a.0[ch].total_cmp(&b.0[ch]));
        // prefix sums along the channel: the split minimizing the halves' summed squared error
        let (mut w_all, mut x_all, mut xx_all) = (0f64, 0f64, 0f64);
        for (c, w) in part.iter() {
            let (w, x) = (*w as f64, c[ch] as f64);
            w_all += w;
            x_all += w * x;
            xx_all += w * x * x;
        }
        let (mut wl, mut xl, mut xxl) = (0f64, 0f64, 0f64);
        let (mut split, mut best) = (part.len() / 2, f64::MAX);
        for i in 1..part.len() {
            let (c, w) = &part[i - 1];
            let (w, x) = (*w as f64, c[ch] as f64);
            wl += w;
            xl += w * x;
            xxl += w * x * x;
            if part[i].0[ch] == part[i - 1].0[ch] {
                continue;
            }
            let (wr, xr, xxr) = (w_all - wl, x_all - xl, xx_all - xxl);
            let cost = (xxl - xl * xl / wl.max(1e-9)) + (xxr - xr * xr / wr.max(1e-9));
            if cost < best {
                (split, best) = (i, cost);
            }
        }
        if best == f64::MAX {
            boxes[b].3 = 0.0; // all equal along the widest channel: can't split
            continue;
        }
        let (lm, ls) = stats(&part[..split]);
        let (rm, rs) = stats(&part[split..]);
        boxes[b] = (start, start + split, lm, ls);
        boxes.push((start + split, end, rm, rs));
    }
    boxes.into_iter().map(|b| b.2).collect()
}

/// Weighted Lloyd iterations; the first `fixed` centers stay put. A center left with no colors
/// moves to the worst-served color.
fn kmeans(entries: &[(Color, f32)], centers: &mut [Color], fixed: usize, iterations: usize) {
    let mut assigned: Vec<u8> = vec![0; entries.len()];
    for it in 0..iterations {
        let nearest = Nearest::new(centers);
        let parts = par::chunked(entries, |offset, chunk| {
            let mut sums = vec![([0f64; DIM], 0f64); centers.len()];
            let mut picks = Vec::with_capacity(chunk.len());
            for (e, (c, w)) in chunk.iter().enumerate() {
                let (i, _) = nearest.find(c, (it > 0).then_some(assigned[offset + e]));
                picks.push(i);
                let s = &mut sums[i as usize];
                for k in 0..DIM {
                    s.0[k] += *w as f64 * c[k] as f64;
                }
                s.1 += *w as f64;
            }
            (sums, picks)
        });
        let mut sums = vec![([0f64; DIM], 0f64); centers.len()];
        for (offset, (part, picks)) in parts.into_iter().enumerate().map(|(i, p)| (i * par::CHUNK, p)) {
            assigned[offset..offset + picks.len()].copy_from_slice(&picks);
            for (s, p) in sums.iter_mut().zip(part) {
                for k in 0..DIM {
                    s.0[k] += p.0[k];
                }
                s.1 += p.1;
            }
        }
        let empty: Vec<usize> = (fixed..centers.len()).filter(|&i| sums[i].1 == 0.0).collect();
        for i in fixed..centers.len() {
            let (s, total) = sums[i];
            if total > 0.0 {
                centers[i] = s.map(|v| (v / total) as f32);
            }
        }
        if !empty.is_empty() {
            let mut errors: Vec<(f32, usize)> = entries.iter().enumerate().map(|(e, (c, w))| (w * distance(c, nearest.color(assigned[e])), e)).collect();
            errors.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            for (i, (_, e)) in empty.into_iter().zip(errors) {
                centers[i] = entries[e].0;
            }
        }
    }
}

/// Wu's color quantizer (Xiaolin Wu, "Efficient Statistical Computations for Optimal Color
/// Quantization", Graphics Gems II, 1991) on the first three coordinates (opaque Oklab): the
/// histogram binned WU_BINS over the range sRGB covers, then the box whose colors vary most is
/// split, along the axis and at the bin that leaves the two halves the least squared error, until
/// there are `k` boxes. Returns their means.
fn wu(entries: &[(Color, f32)], k: usize) -> Vec<Color> {
    #[derive(Clone, Copy, Default)]
    struct Moments {
        w: f64,
        m: [f64; 3],
        q: f64,
    }
    impl Moments {
        fn add(&mut self, o: &Moments, sign: f64) {
            self.w += sign * o.w;
            for a in 0..3 {
                self.m[a] += sign * o.m[a];
            }
            self.q += sign * o.q;
        }
        /// |sum|² / weight: the larger the two halves' total, the less squared error they leave.
        fn spread(&self) -> f64 {
            self.m.iter().map(|m| m * m).sum::<f64>() / self.w
        }
    }
    let n = WU_BINS.map(|b| b + 1);
    let at = |i: [usize; 3]| (i[0] * n[1] + i[1]) * n[2] + i[2];
    let bin = |v: f32, a: usize| {
        let (lo, hi) = WU_RANGES[a];
        (((v - lo) / (hi - lo) * WU_BINS[a] as f32).max(0.0) as usize).min(WU_BINS[a] - 1) + 1
    };
    let mut cells = vec![Moments::default(); n[0] * n[1] * n[2]];
    for (c, w) in entries {
        let cell = &mut cells[at([bin(c[0], 0), bin(c[1], 1), bin(c[2], 2)])];
        let w = *w as f64;
        cell.w += w;
        for a in 0..3 {
            cell.m[a] += w * c[a] as f64;
        }
        cell.q += w * (0..3).map(|a| (c[a] as f64).powi(2)).sum::<f64>();
    }
    // cumulative sums along each axis: cells[i] then holds the moments of the box (0, i]
    for axis in 0..3 {
        for i in 0..n[0] {
            for j in 0..n[1] {
                for l in 0..n[2] {
                    let idx = [i, j, l];
                    if idx[axis] == 0 {
                        continue;
                    }
                    let mut prev = idx;
                    prev[axis] -= 1;
                    let p = cells[at(prev)];
                    cells[at(idx)].add(&p, 1.0);
                }
            }
        }
    }
    // a box is (lo, hi] on each axis, in cumulative-index space
    let volume = |b: &[[usize; 2]; 3]| {
        let mut v = Moments::default();
        for corner in 0..8 {
            let idx = [0, 1, 2].map(|a| b[a][(corner >> a) & 1]);
            // inclusion–exclusion: minus for each lower bound among the corner's coordinates
            let sign = if (corner as u32).count_ones() % 2 == 1 { 1.0 } else { -1.0 };
            v.add(&cells[at(idx)], sign);
        }
        v
    };
    let variance = |b: &[[usize; 2]; 3]| {
        let v = volume(b);
        if v.w > 0.0 {
            v.q - v.spread()
        } else {
            0.0
        }
    };
    let mut boxes = vec![[[0, WU_BINS[0]], [0, WU_BINS[1]], [0, WU_BINS[2]]]];
    let mut spreads = vec![variance(&boxes[0])];
    while boxes.len() < k {
        let Some(i) = (0..boxes.len()).filter(|&i| spreads[i] > 0.0).max_by(|&a, &b| spreads[a].total_cmp(&spreads[b])) else {
            break;
        };
        let whole = volume(&boxes[i]);
        let mut best: Option<(f64, usize, usize)> = None;
        for axis in 0..3 {
            let [from, to] = boxes[i][axis];
            for cut in from + 1..to {
                let mut half = boxes[i];
                half[axis][1] = cut;
                let first = volume(&half);
                let mut rest = whole;
                rest.add(&first, -1.0);
                if first.w <= 0.0 || rest.w <= 0.0 {
                    continue;
                }
                let gain = first.spread() + rest.spread();
                if best.is_none_or(|(g, _, _)| gain > g) {
                    best = Some((gain, axis, cut));
                }
            }
        }
        let Some((_, axis, cut)) = best else {
            spreads[i] = 0.0;
            continue;
        };
        let mut second = boxes[i];
        boxes[i][axis][1] = cut;
        second[axis][0] = cut;
        spreads[i] = variance(&boxes[i]);
        spreads.push(variance(&second));
        boxes.push(second);
    }
    boxes
        .iter()
        .map(volume)
        .filter(|v| v.w > 0.0)
        .map(|v| {
            let mean = v.m.map(|m| (m / v.w) as f32);
            [mean[0], mean[1], mean[2], mean[0], mean[1], mean[2]]
        })
        .collect()
}

/// Online k-means on opaque colors: `samples` pixels drawn uniformly (a fixed pseudo-random
/// sequence, SplitMix64, so every platform draws the same), each moving its nearest center toward
/// itself by count^-1/2 of the way, count being how many samples that center has taken. The large
/// early steps let centers leave poor starting points, which batch k-means rarely does: on photos
/// this gives files up to 15% smaller at the same score.
fn online_kmeans(pixels: &[RGBA], centers: &mut [Color], samples: usize) {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut counts = vec![0u32; centers.len()];
    for _ in 0..samples {
        let pixel = pixels[(((next() >> 32) * pixels.len() as u64) >> 32) as usize];
        let c = to_color(pixel);
        let (mut best, mut dist) = (0, f32::MAX);
        for (i, e) in centers.iter().enumerate() {
            let d = (c[0] - e[0]) * (c[0] - e[0]) + (c[1] - e[1]) * (c[1] - e[1]) + (c[2] - e[2]) * (c[2] - e[2]);
            if d < dist {
                (best, dist) = (i, d);
            }
        }
        counts[best] += 1;
        let rate = 1.0 / (counts[best] as f32).sqrt();
        let e = &mut centers[best];
        for k in 0..3 {
            e[k] += rate * (c[k] - e[k]);
            e[k + 3] = e[k];
        }
    }
}

/// A palette for an opaque image from Wu's quantizer refined by online k-means; None if any pixel
/// is translucent.
pub fn wu_palette(pixels: &[RGBA], colors: usize) -> Option<Vec<RGBA>> {
    if pixels.iter().any(|p| p.a < 255) || colors == 0 {
        return None;
    }
    let entries = entries(histogram(pixels, None));
    let mut centers: Vec<Color> = if entries.len() <= colors { entries.iter().map(|e| e.0).collect() } else { wu(&entries, colors) };
    if entries.len() > colors {
        online_kmeans(pixels, &mut centers, pixels.len().min(ONLINE_SAMPLES));
    }
    let mut out: Vec<RGBA> = Vec::with_capacity(centers.len());
    for c in centers {
        let c = to_rgba(c);
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Some(out)
}

/// A palette of at most `colors` entries. Pixels count by `weights` (an importance map, 0–255) when
/// given. A fully transparent entry is kept if any pixel is.
fn build(pixels: &[RGBA], weights: Option<&[u8]>, colors: usize, rounds: usize) -> Vec<RGBA> {
    let mut counts = histogram(pixels, weights);
    let mut pinned: Vec<Color> = Vec::new();
    if pixels.iter().any(|p| p.a == 0) {
        pinned.push(to_color(RGBA::new(0, 0, 0, 0)));
        counts.retain(|e| e.0.a > 0);
    }
    let mut entries = entries(counts);
    let free = colors.saturating_sub(pinned.len());
    let mut centers = pinned.clone();
    if entries.len() <= free {
        centers.extend(entries.iter().map(|e| e.0));
    } else if free > 0 {
        centers.extend(median_cut(&mut entries, free));
        kmeans(&entries, &mut centers, pinned.len(), rounds);
    }
    let mut out: Vec<RGBA> = Vec::with_capacity(centers.len());
    for c in centers {
        let c = to_rgba(c);
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Lloyd (k-means) iterations on a palette's opaque entries over the image's opaque colors, each
/// color weighted by its pixel count times its distance to its entry to the power `power - 2`: 2 is
/// plain k-means, 3 pulls entries toward the worst-served colors, which plain palettes leave a
/// little further off than TinyPNG's (the same mean error, a larger 99th percentile; FINDINGS.md
/// round 9), costing most where full dithering has to make up for it. Translucent entries are kept.
pub fn refine_palette(pixels: &[RGBA], palette: &[RGBA], iterations: usize, power: f64) -> Vec<RGBA> {
    let entries = entries(histogram(pixels, None).into_iter().filter(|(c, _)| c.a == 255).collect());
    let mut centers: Vec<Color> = palette.iter().filter(|c| c.a == 255).map(|&c| to_color(c)).collect();
    for _ in 0..iterations {
        let nearest = Nearest::new(&centers);
        let parts = par::chunked(&entries, |_, chunk| {
            let mut sums = vec![([0f64; DIM], 0f64); centers.len()];
            for (c, n) in chunk {
                let (i, d) = nearest.find(c, None);
                let weight = *n as f64 * libm::pow((d.sqrt() as f64).max(0.03), power - 2.0);
                let s = &mut sums[i as usize];
                for k in 0..DIM {
                    s.0[k] += weight * c[k] as f64;
                }
                s.1 += weight;
            }
            sums
        });
        for (i, center) in centers.iter_mut().enumerate() {
            let (mut sum, mut total) = ([0f64; DIM], 0f64);
            for part in &parts {
                for k in 0..DIM {
                    sum[k] += part[i].0[k];
                }
                total += part[i].1;
            }
            if total > 0.0 {
                *center = sum.map(|v| (v / total) as f32);
            }
        }
    }
    let mut out: Vec<RGBA> = centers.into_iter().map(to_rgba).collect();
    out.extend(palette.iter().copied().filter(|c| c.a < 255));
    out.sort_by_key(|c| (c.r, c.g, c.b, c.a));
    out.dedup();
    out
}

/// Whether the image has at most 256 colors (fully transparent pixels count as one), like
/// exact_palette but without building anything.
pub fn fits_palette(pixels: &[RGBA]) -> bool {
    let mut seen = HashSet::new();
    let mut last = None;
    for p in pixels {
        let p = if p.a == 0 { RGBA::new(0, 0, 0, 0) } else { *p };
        if last != Some(p) {
            last = Some(p);
            if seen.insert(p) && seen.len() > 256 {
                return false;
            }
        }
    }
    true
}

/// The image as an exact palette, if it has at most 256 colors (fully transparent pixels count as one).
pub fn exact_palette(pixels: &[RGBA]) -> Option<(Vec<RGBA>, Vec<u8>)> {
    let mut palette: Vec<RGBA> = Vec::new();
    let mut index: HashMap<RGBA, u8> = HashMap::new();
    let mut indices = Vec::with_capacity(pixels.len());
    for p in pixels {
        let p = if p.a == 0 { RGBA::new(0, 0, 0, 0) } else { *p };
        let k = match index.get(&p) {
            Some(&k) => k,
            None if palette.len() < 256 => {
                palette.push(p);
                index.insert(p, (palette.len() - 1) as u8);
                (palette.len() - 1) as u8
            }
            None => return None,
        };
        indices.push(k);
    }
    Some((palette, indices))
}
