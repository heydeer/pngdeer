//! Animated PNGs: frames as viewers show them, and compressed animations that stay animations.

use miniz_oxide::deflate::compress_to_vec_zlib;
use pngdeer::{apng, compress, search, Settings, RGBA};

/// One frame of a test animation: its rectangle, delay, dispose and blend ops, and RGBA pixels.
struct Frame {
    rect: (u32, u32, u32, u32), // x, y, width, height
    delay: (u16, u16),
    dispose: u8,
    blend: u8,
    pixels: Vec<RGBA>,
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    png.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// An RGBA APNG of `frames` (the first covers the canvas and is the IDAT).
fn apng_file(w: u32, h: u32, plays: u32, frames: &[Frame]) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[&w.to_be_bytes()[..], &h.to_be_bytes(), &[8, 6, 0, 0, 0]].concat());
    chunk(&mut png, b"acTL", &[(frames.len() as u32).to_be_bytes(), plays.to_be_bytes()].concat());
    let mut sequence = 0u32;
    for (i, f) in frames.iter().enumerate() {
        let (x, y, fw, _) = f.rect;
        let control = [
            &sequence.to_be_bytes()[..],
            &f.rect.2.to_be_bytes(),
            &f.rect.3.to_be_bytes(),
            &x.to_be_bytes(),
            &y.to_be_bytes(),
            &f.delay.0.to_be_bytes(),
            &f.delay.1.to_be_bytes(),
            &[f.dispose, f.blend],
        ]
        .concat();
        chunk(&mut png, b"fcTL", &control);
        sequence += 1;
        let rows: Vec<u8> = f.pixels.chunks(fw as usize).flat_map(|row| std::iter::once(0).chain(row.iter().flat_map(|p| [p.r, p.g, p.b, p.a]))).collect();
        let data = compress_to_vec_zlib(&rows, 6);
        if i == 0 {
            chunk(&mut png, b"IDAT", &data);
        } else {
            chunk(&mut png, b"fdAT", &[&sequence.to_be_bytes()[..], &data].concat());
            sequence += 1;
        }
    }
    chunk(&mut png, b"IEND", &[]);
    png
}

const CLEAR: RGBA = RGBA::new(0, 0, 0, 0);

#[test]
fn frames_are_drawn_as_viewers_draw_them() {
    let (red, blue, green, black, yellow) =
        (RGBA::new(255, 0, 0, 255), RGBA::new(0, 0, 255, 128), RGBA::new(0, 255, 0, 255), RGBA::new(0, 0, 0, 255), RGBA::new(255, 255, 0, 255));
    let frames = [
        Frame { rect: (0, 0, 3, 1), delay: (1, 10), dispose: 0, blend: 0, pixels: vec![red, blue, CLEAR] },
        // half-transparent white over half-transparent blue; then back to the canvas before it
        Frame { rect: (1, 0, 2, 1), delay: (2, 10), dispose: 2, blend: 1, pixels: vec![RGBA::new(255, 255, 255, 128), green] },
        // then cleared
        Frame { rect: (0, 0, 1, 1), delay: (3, 10), dispose: 1, blend: 0, pixels: vec![black] },
        Frame { rect: (2, 0, 1, 1), delay: (4, 0), dispose: 0, blend: 0, pixels: vec![yellow] },
    ];
    let animation = apng::decode(&apng_file(3, 1, 5, &frames)).unwrap();
    assert_eq!((animation.width, animation.height, animation.frames), (3, 1, 4));
    assert_eq!((animation.timing.plays, animation.timing.default_image), (5, false));
    assert_eq!(animation.timing.delays, [(1, 10), (2, 10), (3, 10), (4, 0)]);
    let shown: Vec<&[RGBA]> = animation.pixels.chunks(3).collect();
    assert_eq!(shown[0], [red, blue, CLEAR]);
    // the APNG spec's blend: alpha 128 + 127 × 128 / 255, colors weighted by each alpha's share
    assert_eq!(shown[1], [red, RGBA::new(170, 170, 255, 191), green]);
    assert_eq!(shown[2], [black, blue, CLEAR]);
    assert_eq!(shown[3], [CLEAR, blue, yellow]);
}

/// A disc moving over two gradients, a frame held twice, a transparent corner.
fn moving_disc() -> Vec<u8> {
    let (w, h) = (64u32, 48u32);
    let canvas = |step: i32| -> Vec<RGBA> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as i32, (i / w) as i32);
                let (dx, dy) = (x - 12 - step * 8, y - 24);
                if dx * dx + dy * dy < 100 {
                    RGBA::new(250, 200, 40, 255)
                } else if x + y < 10 {
                    RGBA::new(0, 0, 0, 0)
                } else {
                    RGBA::new((x * 4) as u8, (60 + y / 8 * 12) as u8, 160, 255)
                }
            })
            .collect()
    };
    let full = |step, delay| Frame { rect: (0, 0, w, h), delay, dispose: 0, blend: 0, pixels: canvas(step) };
    apng_file(w, h, 2, &[full(0, (1, 10)), full(1, (1, 10)), full(1, (2, 10)), full(2, (1, 10)), full(3, (3, 20)), full(4, (1, 10))])
}

/// Each frame's start, in seconds.
fn starts(timing: &apng::Timing) -> Vec<f64> {
    let seconds = |&(num, den): &(u16, u16)| num as f64 / if den == 0 { 100.0 } else { den as f64 };
    timing.delays.iter().scan(0.0, |t, d| Some(std::mem::replace(t, *t + seconds(d)))).collect()
}

#[test]
fn a_compressed_animation_stays_one() {
    let input = moving_disc();
    let out = compress(&input, &Settings { target: Some(85.0), ..Default::default() }).unwrap();
    assert!(apng::is_animated(&out.png) && out.png.len() < input.len());
    let (before, after) = (apng::decode(&input).unwrap(), apng::decode(&out.png).unwrap());
    assert_eq!((after.width, after.height, after.timing.plays), (64, 48, 2));
    // the repeated frame is folded into the one before; the timeline is the same
    assert_eq!((after.frames, before.frames), (5, 6));
    let total = |t: &apng::Timing| starts(t).last().unwrap() + t.delays.last().map(|&(n, d)| n as f64 / d as f64).unwrap();
    assert!((total(&after.timing) - total(&before.timing)).abs() < 1e-9);
    // every frame, against the one shown at its time, reaches the target; the worst is the report's
    let (n, (sb, sa)) = (64 * 48, (starts(&before.timing), starts(&after.timing)));
    let scores: Vec<f64> = (0..before.frames)
        .map(|i| {
            let j = sa.iter().rposition(|&s| s <= sb[i] + 1e-9).unwrap();
            search::score(&before.pixels[i * n..(i + 1) * n], &after.pixels[j * n..(j + 1) * n], 64, 48).unwrap()
        })
        .collect();
    let worst = scores.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(out.outcome == "hit" && worst >= 85.0, "{scores:?}");
    assert!((worst - out.report.score).abs() < 1e-9, "{worst} vs {}", out.report.score);
    assert_eq!(out.report.frames, 6);
}

/// The same bytes on every platform and in the browser build (see tests/webp.rs).
#[test]
fn same_animation_bytes_everywhere() {
    let out = compress(&moving_disc(), &Settings::default()).unwrap();
    assert_eq!((out.png.len(), crc32fast::hash(&out.png)), (2025, 3733983883));
}
