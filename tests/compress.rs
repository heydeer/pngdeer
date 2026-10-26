//! End-to-end checks of pngdeer::compress on small synthetic images.

use pngdeer::{codec, compress, search, Settings, RGBA};

/// A 96×80 test image: flat colored blocks with fine noise (which costs a lossless PNG dearly and a
/// palette little) and, optionally, a translucent disc.
fn sample(translucent: bool) -> Vec<u8> {
    let (w, h) = (96, 80);
    let colors = [(200, 60, 40), (30, 120, 220), (240, 230, 210), (20, 20, 30), (90, 180, 90)];
    let mut pixels = Vec::with_capacity(w * h);
    let mut seed = 7u32;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let noise = (seed >> 16) % 9;
            let (r, g, b) = colors[(x / 16 + y / 20) % colors.len()];
            let mut p = RGBA::new(r + noise as u8, g + noise as u8, b - noise as u8, 255);
            let (dx, dy) = (x as i32 - 60, y as i32 - 40);
            if translucent && dx * dx + dy * dy < 400 {
                p.a = (dx.unsigned_abs() * 8).min(255) as u8;
            }
            pixels.push(p);
        }
    }
    codec::encode_rgba(&pixels, w, h)
}

#[test]
fn reaches_a_fixed_target_with_a_palette() {
    let input = sample(true);
    let settings = Settings { target: Some(80.0), ..Default::default() };
    let out = compress(&input, &settings).unwrap();
    assert!(out.png.len() < input.len());
    assert_eq!(out.outcome, "hit");
    assert!(out.report.score >= 80.0);
    assert!(out.report.colors <= 256);
    let (a, b) = (codec::decode(&input).unwrap(), codec::decode(&out.png).unwrap());
    assert_eq!((a.width, a.height), (b.width, b.height));
    // the report's score is the output's
    let score = search::score(&a.pixels, &b.pixels, a.width, a.height).unwrap();
    assert!((score - out.report.score).abs() < 1e-6, "{score} vs {}", out.report.score);
}

#[test]
fn same_input_same_bytes() {
    for translucent in [false, true] {
        let input = sample(translucent);
        for preview_pixels in [0, 2000] {
            let settings = Settings { preview_pixels, ..Default::default() };
            let first = compress(&input, &settings).unwrap();
            let second = compress(&input, &settings).unwrap();
            assert_eq!(first.png, second.png);
            assert_eq!(first.report.preview > 1, preview_pixels > 0);
        }
    }
}

#[test]
fn few_colors_stay_lossless() {
    let pixels: Vec<RGBA> = (0..64 * 64).map(|i| [RGBA::new(255, 0, 0, 255), RGBA::new(0, 0, 0, 0), RGBA::new(9, 9, 9, 255)][i % 7 % 3]).collect();
    let input = codec::encode_rgba(&pixels, 64, 64);
    let out = compress(&input, &Settings::default()).unwrap();
    assert!(matches!(out.outcome, "exact" | "hit"), "{}", out.outcome);
    assert_eq!(codec::decode(&out.png).unwrap().pixels, pixels);
}

/// One PNG chunk: length, type, data and CRC.
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    [&(data.len() as u32).to_be_bytes()[..], kind, data, &crc.finalize().to_be_bytes()].concat()
}

/// The chunk types of a PNG file, in order.
fn chunk_types(png: &[u8]) -> Vec<[u8; 4]> {
    let mut types = Vec::new();
    let mut pos = 8;
    while pos + 8 <= png.len() {
        types.push(png[pos + 4..pos + 8].try_into().unwrap());
        pos += 12 + u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
    }
    types
}

#[test]
fn keeps_the_color_space() {
    let settings = Settings { target: Some(80.0), ..Default::default() };
    // a display's gamma (1.96), as some screenshot tools tag it, and a comment, which goes
    let gamma = png_chunk(b"gAMA", &51_020u32.to_be_bytes());
    let input = codec::with_chunks(&sample(true), &[gamma.clone(), png_chunk(b"tEXt", b"Comment\0hello")].concat());
    let out = compress(&input, &settings).unwrap();
    assert_eq!(out.outcome, "hit");
    assert_eq!(codec::color_chunks(&out.png), gamma);
    assert_eq!(chunk_types(&out.png), [*b"IHDR", *b"gAMA", *b"PLTE", *b"tRNS", *b"IDAT", *b"IEND"]);
    // tagged as sRGB, it looks the same untagged
    let input = codec::with_chunks(&sample(true), &[png_chunk(b"sRGB", &[0]), gamma].concat());
    let out = compress(&input, &settings).unwrap();
    assert_eq!(chunk_types(&out.png), [*b"IHDR", *b"PLTE", *b"tRNS", *b"IDAT", *b"IEND"]);
}

#[test]
fn variant_specs() {
    let parsed = search::parse_variants("50,0,0r2,w0,p100").unwrap();
    let labels: Vec<String> = parsed.iter().map(|v| v.label()).collect();
    assert_eq!(labels, ["d50", "d0", "d0r2", "w0", "p100"]);
    assert_eq!(search::DEFAULT_VARIANTS.iter().map(|v| v.label()).collect::<Vec<_>>(), ["d50", "d0", "d0r2", "w0"]);
    for bad in ["", "101", "x5", "0r", "q0"] {
        assert!(search::parse_variants(bad).is_err(), "{bad}");
    }
}
