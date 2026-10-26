//! Colors as points: sRGB to Oklab and back, the distance palettes and remapping minimize, and the
//! nearest palette entry.
//!
//! A color is how it looks composited over a dark and a light background: the two results in
//! Oklab, six coordinates, each scaled to about 0–255. Opaque colors have the same three twice; a
//! translucent one differs as much as its alpha lets the background through, so translucent pixels
//! compare as they are seen (the benchmark scores them over the same two backgrounds). Distances are
//! half the squared difference, so opaque ones are plain Oklab distances.
//!
//! Transcendental functions come from the `libm` crate rather than the platform's math library, so
//! the command line and the browser build compute the same bits.

// The Oklab matrices are written as published.
#![allow(clippy::excessive_precision)]

use crate::RGBA;

pub const DIM: usize = 6;
pub type Color = [f32; DIM];

/// The two backgrounds colors are composited over: the gray levels the benchmark scores transparent
/// images on. Pure black and white compare dark translucent pixels poorly.
const BACKGROUNDS: [u8; 2] = [24, 232];
/// Oklab a and b span about ±0.4; this brings them to L's 0–255 range.
const CHROMA: f32 = 255.0;

/// sRGB channel value to linear light, 0–1.
fn linear(v: u8) -> f32 {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let s = i as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                libm::powf((s + 0.055) / 1.055, 2.4)
            }
        })
    })[v as usize]
}

/// Linear light, 0–1, to an sRGB channel value.
fn encoded(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.0031308 { 12.92 * v } else { 1.055 * libm::powf(v, 1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// x³ as two multiplications (`powi` is up to the platform).
fn cube(x: f32) -> f32 {
    x * x * x
}

/// Linear-light RGB to Oklab (L 0–1, a and b about ±0.4).
fn oklab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let l = libm::cbrtf(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
    let m = libm::cbrtf(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
    let s = libm::cbrtf(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// Inverse of `oklab`.
fn linear_rgb(lab: [f32; 3]) -> [f32; 3] {
    let l = cube(lab[0] + 0.3963377774 * lab[1] + 0.2158037573 * lab[2]);
    let m = cube(lab[0] - 0.1055613458 * lab[1] - 0.0638541728 * lab[2]);
    let s = cube(lab[0] - 0.0894841775 * lab[1] - 1.2914855480 * lab[2]);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// Scaled Oklab: L × 255, a and b × CHROMA.
fn scaled(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = oklab(rgb);
    [l * 255.0, a * CHROMA, b * CHROMA]
}

fn unscaled(c: &[f32]) -> [f32; 3] {
    [c[0] / 255.0, c[1] / CHROMA, c[2] / CHROMA]
}

pub fn to_color(p: RGBA) -> Color {
    let a = p.a as f32 / 255.0;
    let rgb = [linear(p.r), linear(p.g), linear(p.b)];
    let over = |bg: u8| scaled(rgb.map(|c| c * a + linear(bg) * (1.0 - a)));
    let [dark, light] = BACKGROUNDS;
    let d = over(dark);
    let l = if p.a == 255 { d } else { over(light) };
    [d[0], d[1], d[2], l[0], l[1], l[2]]
}

/// The color whose two composites come closest to `c`'s: alpha from how much the background shows
/// through, then the color from both composites.
pub fn to_rgba(c: Color) -> RGBA {
    let [dark, light] = BACKGROUNDS.map(linear);
    let (d, l) = (linear_rgb(unscaled(&c[..3])), linear_rgb(unscaled(&c[3..])));
    let through = (0..3).map(|k| (l[k] - d[k]) / (light - dark)).sum::<f32>() / 3.0;
    let a = (1.0 - through).clamp(0.0, 1.0);
    let alpha = (a * 255.0).round();
    if alpha == 0.0 {
        return RGBA::new(0, 0, 0, 0);
    }
    let a = alpha / 255.0;
    let channel = |k: usize| encoded((d[k] - dark * (1.0 - a) + l[k] - light * (1.0 - a)) / (2.0 * a));
    RGBA::new(channel(0), channel(1), channel(2), alpha as u8)
}

pub fn distance(p: &Color, q: &Color) -> f32 {
    let mut d = 0.0;
    for k in 0..DIM {
        d += (p[k] - q[k]) * (p[k] - q[k]);
    }
    d * 0.5
}

/// Half the coordinate sum: |key difference|² / 3 is a lower bound on the distance.
fn key(c: &Color) -> f32 {
    c.iter().sum::<f32>() * 0.5
}

/// Exact nearest palette entry, searched outward from the closest key.
pub struct Nearest {
    colors: Vec<Color>,
    keys: Vec<f32>,
    order: Vec<u8>,
    /// position in `colors` of each palette index
    position: Vec<usize>,
}

impl Nearest {
    pub fn new(palette: &[Color]) -> Self {
        let mut order: Vec<u8> = (0..palette.len() as u16).map(|i| i as u8).collect();
        order.sort_by(|&a, &b| key(&palette[a as usize]).total_cmp(&key(&palette[b as usize])));
        let colors: Vec<Color> = order.iter().map(|&i| palette[i as usize]).collect();
        let mut position = vec![0; palette.len()];
        for (p, &i) in order.iter().enumerate() {
            position[i as usize] = p;
        }
        Nearest { keys: colors.iter().map(key).collect(), colors, order, position }
    }

    /// Palette index and distance; `hint` (a likely answer, e.g. the neighbour's) only speeds it up.
    pub fn find(&self, c: &Color, hint: Option<u8>) -> (u8, f32) {
        let k = key(c);
        let (mut best, mut dist) = match hint {
            Some(i) => {
                let p = self.position[i as usize];
                (p, distance(c, &self.colors[p]))
            }
            None => (0, f32::MAX),
        };
        let start = self.keys.partition_point(|&e| e < k);
        let (mut lo, mut hi) = (start, start);
        loop {
            let below = lo > 0 && (k - self.keys[lo - 1]).powi(2) < 3.0 * dist;
            let above = hi < self.keys.len() && (self.keys[hi] - k).powi(2) < 3.0 * dist;
            if !below && !above {
                break;
            }
            for p in [below.then(|| lo - 1), above.then_some(hi)].into_iter().flatten() {
                let d = distance(c, &self.colors[p]);
                if d < dist {
                    (best, dist) = (p, d);
                }
            }
            lo -= below as usize;
            hi += above as usize;
        }
        (self.order[best], dist)
    }

    /// The palette entry `index` stands for.
    pub fn color(&self, index: u8) -> &Color {
        &self.colors[self.position[index as usize]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for a in [1u8, 2, 8, 40, 128, 200, 254, 255] {
            for r in (0..=255).step_by(15) {
                for g in (0..=255).step_by(17) {
                    for b in (0..=255).step_by(51) {
                        let p = RGBA::new(r as u8, g as u8, b as u8, a);
                        let q = to_rgba(to_color(p));
                        // a color channel can only be as exact as alpha lets it show
                        let tol = (255 / a as u32).max(1) + 1;
                        let err = [p.r.abs_diff(q.r), p.g.abs_diff(q.g), p.b.abs_diff(q.b)].into_iter().max().unwrap() as u32;
                        assert!(p.a.abs_diff(q.a) <= 1, "{p:?} -> {q:?}");
                        assert!(err <= tol, "{p:?} -> {q:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn nearest_is_exact() {
        let palette: Vec<Color> = (0..40u8).map(|i| to_color(RGBA::new(i * 6, 255 - i * 5, i * 3, 255 - i))).collect();
        let nearest = Nearest::new(&palette);
        for v in (0..=255).step_by(7) {
            let c = to_color(RGBA::new(v as u8, (v * 3 % 256) as u8, (255 - v) as u8, 255));
            let brute = (0..palette.len()).min_by(|&a, &b| distance(&c, &palette[a]).total_cmp(&distance(&c, &palette[b]))).unwrap();
            assert_eq!(distance(&c, &palette[nearest.find(&c, None).0 as usize]), distance(&c, &palette[brute]));
        }
    }
}
