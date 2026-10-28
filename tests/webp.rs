//! End-to-end checks of pngdeer::compress_webp on small synthetic images.
#![cfg(feature = "webp")]

use pngdeer::{codec, compress_webp, search, webp, Settings, RGBA};

/// A 96×80 test image: smooth gradients with a little noise (where lossy WebP wins), and optionally
/// a translucent disc.
fn photo(translucent: bool) -> Vec<RGBA> {
    let (w, h) = (96, 80);
    let mut seed = 11u32;
    (0..w * h)
        .map(|i| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let (x, y, noise) = (i % w, i / w, (seed >> 16) % 4);
            let (dx, dy) = (x as i32 - 60, y as i32 - 40);
            let a = if dx * dx + dy * dy < 400 { (dx.unsigned_abs() * 8).min(255) as u8 } else { 255 };
            RGBA::new((x * 2 + noise as usize) as u8, (y * 3) as u8, (120 + (x + y) / 2 - noise as usize) as u8, if translucent { a } else { 255 })
        })
        .collect()
}

/// An icon-like image of flat colors and hard edges (where lossless WebP wins).
fn flat() -> Vec<RGBA> {
    let colors = [RGBA::new(250, 250, 250, 255), RGBA::new(30, 30, 40, 255), RGBA::new(220, 60, 50, 255), RGBA::new(0, 0, 0, 0)];
    (0..96 * 80).map(|i| colors[(i % 96 / 12 + i / 96 / 10) % 4]).collect()
}

fn png(pixels: &[RGBA]) -> Vec<u8> {
    codec::encode_rgba(pixels, 96, 80)
}

#[test]
fn reaches_the_target_in_the_smaller_file() {
    for translucent in [false, true] {
        let input = png(&photo(translucent));
        let out = compress_webp(&input, &Settings::default()).unwrap();
        assert_eq!((&out.webp[..4], &out.webp[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
        let decoded = webp::decode(&out.webp).unwrap();
        assert_eq!((decoded.width, decoded.height), (96, 80));
        // the report's score is the file's, and reaches the PNG search's target
        let score = search::score(&photo(translucent), &decoded.pixels, 96, 80).unwrap();
        assert!((score - out.score).abs() < 1e-6, "{score} vs {}", out.score);
        assert!(out.score >= out.target);
        assert_eq!(out.webp.len(), out.lossless_bytes.min(out.lossy_bytes.unwrap_or(usize::MAX)));
        assert_eq!(out.mode, "lossy");
        // a translucent pixel stays as it was: lossy WebP keeps the alpha plane lossless
        assert!(decoded.pixels.iter().zip(photo(translucent)).all(|(a, b)| a.a == b.a));
    }
}

#[test]
fn misses_by_the_least() {
    // out of reach for a palette (about 28 on this image, scored 4× enlarged) and for lossy WebP
    let out = compress_webp(&png(&photo(false)), &Settings { target: Some(95.0), ..Default::default() }).unwrap();
    assert!(out.png.report.miss);
    assert!(out.score < 95.0 && out.score > out.png.report.score + 30.0, "{}", out.score);
    assert_eq!((out.mode, out.quality.is_some()), ("lossy", true));
}

#[test]
fn flat_images_stay_lossless() {
    let out = compress_webp(&png(&flat()), &Settings::default()).unwrap();
    assert_eq!(out.mode, "lossless");
    assert_eq!(webp::decode(&out.webp).unwrap().pixels, flat());
}

#[test]
fn automatic_quality_follows_tinypngs_webp() {
    // lossy at quality 70 beats the palette on this image: its score becomes the target
    let out = compress_webp(&png(&photo(false)), &Settings::default()).unwrap();
    let (quality, score, bytes, _) = out.reference.unwrap();
    assert_eq!(quality, 70);
    assert_eq!(out.target, score.min(90.0));
    assert!(out.score >= out.target && out.webp.len() <= bytes, "{} B at {} vs {bytes} B", out.webp.len(), out.score);
    // the palette beats it on a flat image, as TinyPNG would find: the PNG's target stays
    let flat = compress_webp(&png(&flat()), &Settings::default()).unwrap();
    assert!(flat.reference.unwrap().2 >= flat.lossless_bytes);
    assert_eq!(flat.mode, "lossless");
    // a fixed target is kept as it is
    assert!(compress_webp(&png(&photo(false)), &Settings { target: Some(70.0), ..Default::default() }).unwrap().reference.is_none());
}

/// The bytes on every platform: libwebp is built as plain C without fused multiply-add, and its math
/// functions and qsort are pngdeer's own (vendor/libwebp), so the command line and the browser build
/// write the same files. A change that alters pngdeer's output on purpose updates these.
#[test]
fn same_bytes_everywhere() {
    let digest = |input: &[u8], target| {
        let out = compress_webp(input, &Settings { target, ..Default::default() }).unwrap();
        (out.webp.len(), crc32fast::hash(&out.webp))
    };
    let (photo, translucent, flat) = (png(&photo(false)), png(&photo(true)), png(&flat()));
    let digests = [digest(&photo, None), digest(&translucent, None), digest(&flat, None), digest(&photo, Some(70.0))];
    assert_eq!(digests, [(262, 1347600136), (412, 2323082491), (100, 3506530094), (402, 2713252077)]);
}

/// One PNG chunk: length, type, data and CRC.
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    [&(data.len() as u32).to_be_bytes()[..], kind, data, &crc.finalize().to_be_bytes()].concat()
}

/// The chunks of a WebP file: type and contents.
fn riff_chunks(webp: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut chunks = Vec::new();
    let mut pos = 12;
    while pos + 8 <= webp.len() {
        let len = u32::from_le_bytes(webp[pos + 4..pos + 8].try_into().unwrap()) as usize;
        chunks.push((webp[pos..pos + 4].try_into().unwrap(), webp[pos + 8..pos + 8 + len].to_vec()));
        pos += 8 + len + len % 2;
    }
    assert_eq!(u32::from_le_bytes(webp[4..8].try_into().unwrap()) as usize + 8, webp.len());
    chunks
}

#[test]
fn carries_the_icc_profile() {
    let settings = Settings { target: Some(80.0), ..Default::default() };
    let profile: Vec<u8> = (0..301u32).map(|i| (i * 7) as u8).collect(); // libwebp doesn't read it
    let iccp = [&b"Display P3\0\0"[..], &miniz_oxide::deflate::compress_to_vec_zlib(&profile, 6)].concat();
    for pixels in [photo(true), photo(false), flat()] {
        let input = codec::with_chunks(&png(&pixels), &png_chunk(b"iCCP", &iccp));
        let out = compress_webp(&input, &settings).unwrap();
        let chunks = riff_chunks(&out.webp);
        let kinds: Vec<[u8; 4]> = chunks.iter().map(|(k, _)| *k).collect();
        assert_eq!(&kinds[..2], [*b"VP8X", *b"ICCP"]);
        let flags = chunks[0].1[0];
        assert_eq!(flags & 0x20, 0x20, "ICC flag");
        assert_eq!(flags & 0x10 != 0, pixels.iter().any(|p| p.a < 255), "alpha flag");
        assert_eq!(chunks[0].1[4..10], [95, 0, 0, 79, 0, 0], "canvas size");
        assert_eq!(chunks[1].1, profile);
        assert_eq!(webp::decode(&out.webp).unwrap().width, 96);
    }
    // an sRGB image needs no profile
    let out = compress_webp(&codec::with_chunks(&png(&photo(false)), &png_chunk(b"sRGB", &[0])), &settings).unwrap();
    assert!(riff_chunks(&out.webp).iter().all(|(k, _)| k != b"ICCP"));
}

#[test]
fn refuses_what_webp_cant_show() {
    let settings = Settings::default();
    let gamma = png_chunk(b"gAMA", &51_020u32.to_be_bytes());
    let pq = png_chunk(b"cICP", &[9, 16, 0, 1]); // BT.2020 primaries, PQ: HDR
    let animated = png_chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
    for (chunks, says) in [(gamma, "gAMA"), (pq, "cICP"), (animated, "animated")] {
        let error = compress_webp(&codec::with_chunks(&png(&flat()), &chunks), &settings).err().expect(says).to_string();
        assert!(error.contains(says), "{error}");
    }
}

#[test]
fn command_line_picks_webp_from_the_name() {
    let dir = std::env::temp_dir().join(format!("pngdeer-webp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (input, output) = (dir.join("in.png"), dir.join("out.webp"));
    std::fs::write(&input, png(&photo(false))).unwrap();
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_pngdeer")).arg(&input).arg("-o").arg(&output).args(["--target", "70"]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let report = String::from_utf8(run.stdout).unwrap();
    assert!(report.starts_with(r#"{"format": "webp", "target": 70.00"#), "{report}");
    let file = std::fs::read(&output).unwrap();
    assert_eq!(&file[8..12], b"WEBP");
    // --score reads it back, and gets the report's score
    let score = std::process::Command::new(env!("CARGO_BIN_EXE_pngdeer")).arg(&input).arg("--score").arg(&output).output().unwrap();
    let score: f64 = String::from_utf8(score.stdout).unwrap().trim().parse().unwrap();
    let reported: f64 = report.split(r#""score": "#).nth(1).unwrap().split(',').next().unwrap().parse().unwrap();
    assert!(score >= 70.0 && (score - reported).abs() < 1e-3, "{score} vs {reported}");
    std::fs::remove_dir_all(&dir).unwrap();
}
