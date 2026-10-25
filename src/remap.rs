//! Pixels onto a palette: the nearest entry, or serpentine Floyd–Steinberg damped where the image
//! already has edges or texture; and neighbour smoothing (--rdo) afterwards.

use std::collections::HashMap;

use crate::color::{distance, to_color, Color, Nearest, DIM};
use crate::{par, RGBA};

/// Largest error (per coordinate) carried into a pixel by dithering: more gives smooth 3D renders
/// streaks and costs bytes (48 scored 2.4 points under libimagequant's remap at 256 colors).
const MAX_ERROR: f32 = 6.0;
/// Errors under this distance aren't diffused, which keeps flat areas clean (fewer bytes).
const DEAD_ZONE: f32 = 1.5;
/// Local contrast (largest distance to a 4-neighbour) where dithering starts to fade, and where it
/// is down to DAMPED_DITHER.
const FADE_FROM: f32 = 12.0;
const FADE_TO: f32 = 48.0;
const DAMPED_DITHER: f32 = 0.35;
/// Dithering runs in bands of this many rows, each starting with no error, so bands can run in
/// parallel and the result doesn't depend on the number of threads.
const BAND: usize = 64;

/// How much dithering each pixel gets (1 = full): less where it differs strongly from a neighbour,
/// at edges (where diffused error makes halos) and in texture (which hides banding anyway).
fn dither_map(rgba: &[RGBA], pixels: &[Color], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![1.0; pixels.len()];
    par::fill(&mut out, par::CHUNK, |offset, chunk| {
        for (i, v) in chunk.iter_mut().enumerate() {
            let k = offset + i;
            let (x, y) = (k % w, k / w);
            let c = &pixels[k];
            let mut most = 0f32;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < w && ny < h && rgba[ny * w + nx] != rgba[k] {
                    most = most.max(distance(c, &pixels[ny * w + nx]));
                }
            }
            let contrast = most.sqrt();
            let t = ((contrast - FADE_FROM) / (FADE_TO - FADE_FROM)).clamp(0.0, 1.0);
            *v = 1.0 - t * (1.0 - DAMPED_DITHER);
        }
    });
    out
}

/// Palette indices for `pixels`: the nearest entry, or with `dither` > 0 serpentine
/// Floyd–Steinberg diffusing that share of each error (damped by dither_map, capped at MAX_ERROR).
/// Only opaque pixels are dithered: error spread through anti-aliased edges into what they border
/// cost up to 4 points on line art over transparency, so translucent pixels take the
/// nearest entry and neither take nor pass on error.
pub fn remap(pixels: &[RGBA], w: usize, h: usize, palette: &[RGBA], dither: f32) -> Vec<u8> {
    let pal: Vec<Color> = palette.iter().map(|&c| to_color(c)).collect();
    let nearest = Nearest::new(&pal);
    let mut out = vec![0u8; pixels.len()];
    if dither <= 0.0 {
        for (k, part) in par::chunked(pixels, |_, chunk| {
            let mut cache: HashMap<RGBA, u8> = HashMap::new();
            let mut last: Option<(RGBA, u8)> = None;
            chunk
                .iter()
                .map(|&p| {
                    let i = match last {
                        Some((q, i)) if q == p => i,
                        _ => *cache.entry(p).or_insert_with(|| nearest.find(&to_color(p), last.map(|l| l.1)).0),
                    };
                    last = Some((p, i));
                    i
                })
                .collect::<Vec<u8>>()
        })
        .into_iter()
        .enumerate()
        {
            out[k * par::CHUNK..k * par::CHUNK + part.len()].copy_from_slice(&part);
        }
        return out;
    }
    // Colors repeat (runs, flat areas): convert each run once, and look up a pixel that takes no
    // error by its color.
    let mut src: Vec<Color> = vec![[0.0; DIM]; pixels.len()];
    par::fill(&mut src, par::CHUNK, |offset, chunk| {
        for i in 0..chunk.len() {
            let k = offset + i;
            chunk[i] = if i > 0 && pixels[k - 1] == pixels[k] { chunk[i - 1] } else { to_color(pixels[k]) };
        }
    });
    let damping = dither_map(pixels, &src, w, h);
    let top = 255.0;
    par::fill(&mut out, BAND * w, |offset, band| {
        let y0 = offset / w;
        let mut exact: HashMap<RGBA, (u8, f32)> = HashMap::new();
        let mut err = vec![[0f32; DIM]; (w + 2) * 2];
        let mut hint = None;
        for y in y0..y0 + band.len() / w {
            let (cur, next) = ((y % 2) * (w + 2), ((y + 1) % 2) * (w + 2));
            err[next..next + w + 2].fill([0.0; DIM]);
            let ltr = y % 2 == 0;
            for i in 0..w {
                let x = if ltr { i } else { w - 1 - i };
                let k = y * w + x;
                let c = src[k];
                if pixels[k].a < 255 {
                    band[k - offset] = nearest.find(&c, hint).0;
                    continue;
                }
                let e = err[cur + x + 1];
                let (want, (j, d)) = if e == [0.0; DIM] {
                    (c, *exact.entry(pixels[k]).or_insert_with(|| nearest.find(&c, hint)))
                } else {
                    let mut want: Color = std::array::from_fn(|ch| c[ch] + e[ch].clamp(-MAX_ERROR, MAX_ERROR));
                    want[0] = want[0].clamp(0.0, top);
                    want[3] = want[3].clamp(0.0, top);
                    (want, nearest.find(&want, hint))
                };
                hint = Some(j);
                band[k - offset] = j;
                if d < DEAD_ZONE * DEAD_ZONE {
                    continue;
                }
                let strength = dither * damping[k];
                let q: Color = std::array::from_fn(|ch| (want[ch] - pal[j as usize][ch]) * strength);
                let (fwd, back): (isize, isize) = if ltr { (1, -1) } else { (-1, 1) };
                let at = |dx: isize| (x as isize + 1 + dx) as usize;
                for ch in 0..DIM {
                    err[cur + at(fwd)][ch] += q[ch] * 7.0 / 16.0;
                    err[next + at(back)][ch] += q[ch] * 3.0 / 16.0;
                    err[next + at(0)][ch] += q[ch] * 5.0 / 16.0;
                    err[next + at(fwd)][ch] += q[ch] * 1.0 / 16.0;
                }
            }
        }
    });
    out
}

/// Distance between two colors as seen over any background: alpha, and color premultiplied by alpha.
fn premultiplied_distance(p: RGBA, q: RGBA) -> u32 {
    let pre = |c: u8, a: u8| c as u32 * a as u32 / 255;
    [pre(p.r, p.a).abs_diff(pre(q.r, q.a)), pre(p.g, p.a).abs_diff(pre(q.g, q.a)), pre(p.b, p.a).abs_diff(pre(q.b, q.a)), (p.a as u32).abs_diff(q.a as u32)]
        .into_iter()
        .max()
        .unwrap()
}

/// See --rdo: switch pixels to a neighbour's palette entry when it costs at most `delta` in accuracy.
pub fn prefer_neighbours(pixels: &[RGBA], palette: &[RGBA], indices: &mut [u8], w: usize, delta: u32) {
    for k in 0..indices.len() {
        let current = indices[k];
        let neighbours = [(k % w != 0).then(|| indices[k - 1]), (k >= w).then(|| indices[k - w])];
        if neighbours.contains(&Some(current)) {
            continue;
        }
        let limit = premultiplied_distance(pixels[k], palette[current as usize]) + delta;
        let best = neighbours.into_iter().flatten().map(|i| (premultiplied_distance(pixels[k], palette[i as usize]), i)).filter(|&(d, _)| d <= limit).min();
        if let Some((_, i)) = best {
            indices[k] = i;
        }
    }
}
