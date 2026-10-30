//! End-to-end checks of pngdeer::compress_jpeg on small synthetic photos.
#![cfg(feature = "jpeg")]

use pngdeer::{compress_jpeg, jpeg, search, Settings};

const W: usize = 160;
const H: usize = 120;

/// RGB samples of a photo-like test image: smooth gradients, a soft disc and a little noise.
fn photo() -> Vec<u8> {
    let mut seed = 7u32;
    (0..W * H)
        .flat_map(|i| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let (x, y, noise) = (i % W, i / W, ((seed >> 16) % 9) as i32 - 4);
            let (dx, dy) = (x as i32 - 100, y as i32 - 55);
            let disc = (900 - (dx * dx + dy * dy)).clamp(0, 900) / 8;
            let c = |v: i32| (v + noise).clamp(0, 255) as u8;
            [c(40 + x as i32 + disc), c(60 + y as i32 * 3 / 2), c(170 - x as i32 / 2 + disc / 2)]
        })
        .collect()
}

/// A camera-like JPEG of the test photo: high quality, full-resolution chroma.
fn camera_jpeg(icc: Option<&[u8]>, exif: Option<&[u8]>) -> Vec<u8> {
    let options = mozjpeg::Options { quality: 95, full_chroma: true, ..Default::default() };
    mozjpeg::encode(&photo(), W, H, 3, &options, icc, exif).unwrap()
}

/// An EXIF APP1 payload with an orientation and a camera model, little-endian as cameras write it.
fn camera_exif(orientation: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x02\0".to_vec();
    exif.extend_from_slice(&[0x10, 0x01, 2, 0, 4, 0, 0, 0, b'C', b'a', b'm', 0]); // model "Cam", inline
    exif.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0]);
    exif.extend_from_slice(&orientation.to_le_bytes());
    exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    exif
}

/// A minimal ICC profile whose description is `desc` (all is_srgb looks at).
fn profile(desc: &str) -> Vec<u8> {
    let tag = [&b"desc\0\0\0\0"[..], &(desc.len() as u32 + 1).to_be_bytes(), desc.as_bytes(), b"\0"].concat();
    let mut icc = vec![0u8; 128];
    icc.extend_from_slice(&1u32.to_be_bytes());
    icc.extend_from_slice(b"desc");
    icc.extend_from_slice(&144u32.to_be_bytes());
    icc.extend_from_slice(&(tag.len() as u32).to_be_bytes());
    icc.extend_from_slice(&tag);
    icc
}

/// The JPEG markers of a file up to its first scan: (marker, payload).
fn markers(jpeg: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff && jpeg[at + 1] != 0xda {
        let size = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        out.push((jpeg[at + 1], &jpeg[at + 4..at + 2 + size]));
        at += 2 + size;
    }
    out
}

#[test]
fn reaches_the_target_in_fewer_bytes() {
    let input = camera_jpeg(None, None);
    let out = compress_jpeg(&input, &Settings::default()).unwrap();
    assert_eq!(out.outcome, "hit");
    assert!(out.jpeg.starts_with(&[0xff, 0xd8]) && out.jpeg.len() < input.len(), "{} vs {} bytes", out.jpeg.len(), input.len());
    // the report's score is the file's against the input as it decodes, and reaches the target
    let (source, compressed) = (jpeg::decode(&input).unwrap(), jpeg::decode(&out.jpeg).unwrap());
    assert_eq!((compressed.width, compressed.height), (W, H));
    let score = search::score(&source.pixels, &compressed.pixels, W, H).unwrap();
    assert!((score - out.score).abs() < 1e-9, "{score} vs {}", out.score);
    assert!(out.score >= out.target);
    // the quality just below falls short: a fixed target at this score lands on the same quality
    let fixed = compress_jpeg(&input, &Settings { target: Some(out.score), ..Default::default() }).unwrap();
    assert_eq!((fixed.quality, fixed.jpeg.len()), (out.quality, out.jpeg.len()));
}

#[test]
fn never_bigger_than_the_input() {
    // already heavily compressed: nothing that reaches a high target is smaller
    let small = mozjpeg::encode(&photo(), W, H, 3, &mozjpeg::Options { quality: 30, ..Default::default() }, None, None).unwrap();
    let out = compress_jpeg(&small, &Settings { target: Some(90.0), ..Default::default() }).unwrap();
    assert_eq!((out.outcome, out.quality, out.score), ("original", None, 100.0));
    assert_eq!(out.jpeg, small);
}

#[test]
fn misses_by_the_least() {
    // 4:2:0 can't reach 99 on this image: the best-scoring file smaller than the input
    let input = mozjpeg::encode(&photo(), W, H, 3, &mozjpeg::Options { quality: 100, full_chroma: true, ..Default::default() }, None, None).unwrap();
    let out = compress_jpeg(&input, &Settings { target: Some(99.0), ..Default::default() }).unwrap();
    assert_eq!(out.outcome, "miss");
    assert!(out.score < 99.0 && out.jpeg.len() < input.len());
}

#[test]
fn keeps_the_colors_and_the_orientation_only() {
    let display_p3 = profile("Display P3");
    let input = camera_jpeg(Some(&display_p3), Some(&camera_exif(6)));
    let out = compress_jpeg(&input, &Settings::default()).unwrap();
    let marks = markers(&out.jpeg);
    // the profile, whole, in one APP2 marker
    let icc: Vec<&[u8]> = marks.iter().filter(|(m, p)| *m == 0xe2 && p.starts_with(b"ICC_PROFILE\0")).map(|(_, p)| &p[14..]).collect();
    assert_eq!(icc, [&display_p3[..]]);
    // EXIF: the orientation, without the camera model
    let exif: Vec<&[u8]> = marks.iter().filter(|(m, p)| *m == 0xe1 && p.starts_with(b"Exif\0\0")).map(|(_, p)| *p).collect();
    assert_eq!(exif.len(), 1);
    assert!(!exif[0].windows(3).any(|w| w == b"Cam"));
    assert_eq!(jpeg::decode(&out.jpeg).unwrap().orientation, 6);
    // an sRGB profile is what viewers assume anyway, and no orientation needs none: neither is kept
    let plain = compress_jpeg(&camera_jpeg(Some(&profile("sRGB IEC61966-2.1")), Some(&camera_exif(1))), &Settings::default()).unwrap();
    assert!(markers(&plain.jpeg).iter().all(|(m, _)| !matches!(m, 0xe1 | 0xe2)));
}

#[test]
fn gray_stays_gray() {
    let gray: Vec<u8> = photo().chunks(3).map(|p| p[1]).collect();
    let input = mozjpeg::encode(&gray, W, H, 1, &mozjpeg::Options { quality: 95, ..Default::default() }, None, None).unwrap();
    let out = compress_jpeg(&input, &Settings::default()).unwrap();
    assert!(jpeg::decode(&out.jpeg).unwrap().gray);
    // one component in the frame header
    let sof = markers(&out.jpeg).into_iter().find(|(m, _)| matches!(m, 0xc0 | 0xc2)).unwrap().1;
    assert_eq!(sof[5], 1);
}

#[test]
fn refuses_what_it_cant_read() {
    let input = camera_jpeg(None, None);
    let scan = input.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
    for broken in [&input[..2], &input[..scan], b"\xff\xd8\xff\xe0 not a jpeg"] {
        let error = compress_jpeg(broken, &Settings::default()).err().unwrap().to_string();
        assert!(error.starts_with("not a JPEG pngdeer can read: ") && !error.ends_with('\n'), "{error:?}");
    }
    // a file cut short in its scans compresses as viewers show it: what arrived, gray below
    assert!(compress_jpeg(&input[..input.len() / 2], &Settings::default()).is_ok());
}

/// The bytes on every platform: mozjpeg is built as plain C without fused multiply-add, and the
/// decoder is zune-jpeg's plain Rust, so the command line and the browser build write the same
/// files. A change that alters pngdeer's output on purpose updates these.
#[test]
fn same_bytes_everywhere() {
    let digest = |input: &[u8], target| {
        let out = compress_jpeg(input, &Settings { target, ..Default::default() }).unwrap();
        (out.jpeg.len(), crc32fast::hash(&out.jpeg))
    };
    let (plain, tagged) = (camera_jpeg(None, None), camera_jpeg(Some(&profile("Display P3")), Some(&camera_exif(6))));
    let digests = [digest(&plain, None), digest(&tagged, None), digest(&plain, Some(80.0))];
    assert_eq!(digests, [(3767, 3560351722), (3988, 3204447720), (3621, 3529542978)]);
}

/// The chunks of a WebP file: type and contents.
#[cfg(feature = "webp")]
fn webp_chunks(webp: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 12;
    while at + 8 <= webp.len() {
        let len = u32::from_le_bytes(webp[at + 4..at + 8].try_into().unwrap()) as usize;
        out.push((webp[at..at + 4].try_into().unwrap(), &webp[at + 8..at + 8 + len]));
        at += 8 + len + len % 2;
    }
    out
}

#[cfg(feature = "webp")]
#[test]
fn becomes_lossy_webp_at_the_jpeg_target() {
    use pngdeer::{compress_webp, webp};
    let input = camera_jpeg(None, None);
    let out = compress_webp(&input, &Settings::default()).unwrap();
    assert_eq!((&out.webp[..4], &out.webp[8..12], out.mode), (&b"RIFF"[..], &b"WEBP"[..], "lossy"));
    assert!(out.png.is_none() && out.lossless_bytes.is_none());
    // the automatic target is JPEG output's, reached, and the score is the file's
    assert_eq!(out.target, compress_jpeg(&input, &Settings::default()).unwrap().target);
    let (source, decoded) = (jpeg::decode(&input).unwrap(), webp::decode(&out.webp).unwrap());
    let score = search::score(&source.pixels, &decoded.pixels, W, H).unwrap();
    assert!((score - out.score).abs() < 1e-9 && out.score >= out.target, "{score} vs {} for {}", out.score, out.target);
}

#[cfg(feature = "webp")]
#[test]
fn webp_is_turned_upright_and_keeps_the_colors() {
    use pngdeer::{compress_webp, webp};
    let display_p3 = profile("Display P3");
    let out = compress_webp(&camera_jpeg(Some(&display_p3), Some(&camera_exif(6))), &Settings::default()).unwrap();
    // a quarter turn: 120 × 160, its top row the photo's left column, bottom up
    let decoded = webp::decode(&out.webp).unwrap();
    assert_eq!((decoded.width, decoded.height), (H, W));
    let (upright, ..) = jpeg::oriented(jpeg::decode(&camera_jpeg(None, None)).unwrap().pixels, W, H, 6);
    assert!(search::score(&upright, &decoded.pixels, H, W).unwrap() > 60.0);
    // the profile in an ICCP chunk, no EXIF
    let chunks = webp_chunks(&out.webp);
    assert_eq!(chunks.iter().filter(|(k, _)| k == b"ICCP").map(|(_, d)| *d).collect::<Vec<_>>(), [&display_p3[..]]);
    assert!(chunks.iter().all(|(k, _)| k != b"EXIF"));
}

/// As `same_bytes_everywhere`, for WebP from a JPEG.
#[cfg(feature = "webp")]
#[test]
fn same_webp_bytes_everywhere() {
    let digest = |input: &[u8], target| {
        let out = pngdeer::compress_webp(input, &Settings { target, ..Default::default() }).unwrap();
        (out.webp.len(), crc32fast::hash(&out.webp))
    };
    let (plain, tagged) = (camera_jpeg(None, None), camera_jpeg(Some(&profile("Display P3")), Some(&camera_exif(6))));
    let digests = [digest(&plain, None), digest(&tagged, None), digest(&plain, Some(80.0))];
    assert_eq!(digests, [(5560, 3088742262), (5722, 2315651081), (3820, 1353286174)]);
}
