//! End-to-end checks of pngdeer::compress_avif on synthetic images. pngdeer has no AV1 decoder, so
//! the files are checked as containers here; that they decode to what their reports scored, in dav1d
//! and in Chrome, is experiments/avif/FINDINGS.md round 9.
#![cfg(feature = "avif")]

use pngdeer::{codec, compress, compress_avif, Settings, RGBA};

/// A photo-like test image: smooth gradients, a soft disc and a little noise, optionally with a
/// translucent disc of its own.
fn photo(w: usize, h: usize, translucent: bool) -> Vec<RGBA> {
    let mut seed = 7u32;
    (0..w * h)
        .map(|i| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let (x, y, noise) = (i % w, i / w, ((seed >> 16) % 9) as i32 - 4);
            let (dx, dy) = (x as i32 - w as i32 * 5 / 8, y as i32 - h as i32 / 2);
            let disc = (900 - (dx * dx + dy * dy)).clamp(0, 900) / 8;
            let c = |v: i32| (v + noise).clamp(0, 255) as u8;
            let (sx, sy) = ((x * 160 / w) as i32, (y * 120 / h) as i32);
            let (ax, ay) = (x as i32 - w as i32 / 4, y as i32 - h as i32 / 3);
            let a = if translucent && ax * ax + ay * ay < 300 { (ax.unsigned_abs() * 12).min(255) as u8 } else { 255 };
            RGBA::new(c(40 + sx + disc), c(60 + sy * 3 / 2), c(170 - sx / 2 + disc / 2), a)
        })
        .collect()
}

/// An icon-like image of flat colors and hard edges, a palette's best case, on a transparent
/// background or a white one.
fn icon(translucent: bool) -> Vec<RGBA> {
    let background = if translucent { RGBA::new(0, 0, 0, 0) } else { RGBA::new(255, 255, 255, 255) };
    let colors = [background, RGBA::new(30, 30, 40, 255), RGBA::new(220, 60, 50, 255), RGBA::new(250, 200, 40, 255)];
    (0..96 * 80)
        .map(|i| {
            let (dx, dy) = (i % 96 - 48, i / 96 - 40);
            let r = dx * dx + dy * dy;
            colors[if r < 300 {
                3
            } else if r < 900 {
                2
            } else if r < 1300 {
                1
            } else {
                0
            }]
        })
        .collect()
}

fn png(pixels: &[RGBA], w: usize, h: usize) -> Vec<u8> {
    codec::encode_rgba(pixels, w, h)
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    [&(data.len() as u32).to_be_bytes()[..], kind, data, &crc.finalize().to_be_bytes()].concat()
}

/// An item property: type, contents, and whether it is marked essential.
type Property = ([u8; 4], Vec<u8>, bool);

/// An AVIF file taken apart: its brands, each item's data and properties, and its item references.
struct Avif {
    brands: Vec<[u8; 4]>,
    /// (item, its data)
    items: Vec<(u16, Vec<u8>)>,
    /// (item, its properties)
    properties: Vec<(u16, Vec<Property>)>,
    /// (type, from, to)
    references: Vec<([u8; 4], u16, u16)>,
}

/// The boxes in `bytes`: type and contents.
fn boxes(mut bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert!(size >= 8 && size <= bytes.len(), "box size {size} of {}", bytes.len());
        out.push((bytes[4..8].try_into().unwrap(), &bytes[8..size]));
        bytes = &bytes[size..];
    }
    out
}

fn child<'a>(boxes: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> &'a [u8] {
    let found: Vec<&[u8]> = boxes.iter().filter(|(k, _)| k == kind).map(|(_, c)| *c).collect();
    assert_eq!(found.len(), 1, "{} boxes", String::from_utf8_lossy(kind));
    found[0]
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(b[at..at + 2].try_into().unwrap())
}

fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

fn parse(file: &[u8]) -> Avif {
    let top = boxes(file);
    assert_eq!(top.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [*b"ftyp", *b"meta", *b"mdat"]);
    let ftyp = child(&top, b"ftyp");
    assert_eq!(&ftyp[..8], b"avif\0\0\0\0");
    let brands = ftyp[8..].chunks(4).map(|b| b.try_into().unwrap()).collect();
    let meta = boxes(&child(&top, b"meta")[4..]);
    assert_eq!(meta[0].0, *b"hdlr", "hdlr comes first");
    assert_eq!(&child(&meta, b"hdlr")[8..12], b"pict");
    assert_eq!(u16_at(child(&meta, b"pitm"), 4), 1);
    // iloc version 0: 4-byte offsets and lengths, no base offset, one extent an item
    let iloc = child(&meta, b"iloc");
    assert_eq!(&iloc[..6], &[0, 0, 0, 0, 0x44, 0]);
    let items = (0..u16_at(iloc, 6) as usize)
        .map(|i| {
            let e = &iloc[8 + i * 14..];
            assert_eq!(&e[2..6], &[0, 0, 0, 1]);
            let (offset, length) = (u32_at(e, 6), u32_at(e, 10));
            (u16_at(e, 0), file[offset..offset + length].to_vec())
        })
        .collect::<Vec<_>>();
    let iinf = child(&meta, b"iinf");
    let infe = boxes(&iinf[6..]);
    assert_eq!(infe.len(), items.len());
    for (_, e) in &infe {
        assert_eq!(&e[..1], &[2]);
        assert_eq!(&e[8..12], b"av01");
    }
    let iprp = boxes(child(&meta, b"iprp"));
    let ipco = boxes(child(&iprp, b"ipco"));
    let ipma = child(&iprp, b"ipma");
    let mut at = 8;
    let properties = (0..u32_at(ipma, 4))
        .map(|_| {
            let item = u16_at(ipma, at);
            let n = ipma[at + 2] as usize;
            let props =
                ipma[at + 3..at + 3 + n].iter().map(|&p| (ipco[(p & 0x7f) as usize - 1].0, ipco[(p & 0x7f) as usize - 1].1.to_vec(), p & 0x80 != 0)).collect();
            at += 3 + n;
            (item, props)
        })
        .collect();
    let references = meta
        .iter()
        .filter(|(k, _)| k == b"iref")
        .flat_map(|(_, c)| boxes(&c[4..]).into_iter().map(|(k, r)| (k, u16_at(r, 0), u16_at(r, 4))).collect::<Vec<_>>())
        .collect();
    Avif { brands, items, properties, references }
}

impl Avif {
    fn property(&self, item: u16, kind: &[u8; 4]) -> Vec<Vec<u8>> {
        self.properties.iter().find(|(i, _)| *i == item).unwrap().1.iter().filter(|(k, _, _)| k == kind).map(|(_, c, _)| c.clone()).collect()
    }

    /// The colr box of type `kind` on the color item.
    fn colr(&self, kind: &[u8; 4]) -> Option<Vec<u8>> {
        self.property(1, b"colr").into_iter().find(|c| &c[..4] == kind).map(|c| c[4..].to_vec())
    }
}

/// What every file must hold: the brands, an av1C matching its item's sequence header, ispe and
/// pixi, the item's AV1 data without a temporal delimiter; and alpha as an auxiliary item.
fn check(file: &[u8], w: usize, h: usize, alpha: bool) -> Avif {
    let avif = parse(file);
    assert_eq!(avif.brands, [*b"avif", *b"mif1", *b"miaf"]);
    assert_eq!(avif.items.len(), 1 + alpha as usize);
    for (item, data) in &avif.items {
        // a sequence header OBU first (type 1, with its size), then the frame
        assert_eq!(data[0], 0x0a, "item {item} starts with its sequence header");
        let header = &data[2..];
        let av1c = &avif.property(*item, b"av1C")[0];
        assert_eq!(av1c[0], 0x81);
        assert_eq!(av1c[1] >> 5, header[0] >> 5, "seq_profile");
        assert_eq!(av1c[1] & 31, (u16::from_be_bytes([header[0], header[1]]) >> 6 & 31) as u8, "seq_level_idx_0");
        assert_eq!(av1c[2] >> 4 & 1, (*item == 2) as u8, "monochrome");
        assert!(avif.properties.iter().find(|(i, _)| i == item).unwrap().1.iter().any(|(k, _, essential)| k == b"av1C" && *essential));
        assert_eq!(avif.property(*item, b"ispe")[0][4..], [(w as u32).to_be_bytes(), (h as u32).to_be_bytes()].concat());
        let depth = if av1c[2] & 0x40 != 0 { 10 } else { 8 };
        let channels = if *item == 1 { 3 } else { 1 };
        assert_eq!(avif.property(*item, b"pixi")[0][4..], [&[channels][..], &vec![depth; channels as usize]].concat());
    }
    if alpha {
        assert_eq!(avif.references, [(*b"auxl", 2, 1)]);
        assert_eq!(avif.property(2, b"auxC")[0][4..], *b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0");
    } else {
        assert!(avif.references.is_empty());
    }
    avif
}

#[test]
fn reaches_the_target_below_png_outputs() {
    for (pixels, translucent) in [(icon(false), false), (icon(true), true), (photo(96, 80, true), true)] {
        let input = png(&pixels, 96, 80);
        let out = compress_avif(&input, &Settings::default()).unwrap();
        // about TinyPNG's AVIF's quality: PNG output's automatic target less 9 (a photo's at least,
        // which this palette-hostile photo gets: a_smooth_gradient_aims_no_lower_than_a_photo)
        let png_target = compress(&input, &Settings::default()).unwrap().report.target;
        assert!((out.target - (png_target - 9.0).max(62.3)).abs() < 1e-9, "{} vs {png_target}", out.target);
        assert_eq!(out.outcome, "hit");
        assert!(out.score >= out.target);
        // a small image: the smallest of the settings searched, all at once
        assert!(["10bit-s4", "10bit-screen-s4", "8bit-s4", "10bit-s2"].contains(&out.setting.as_str()), "{}", out.setting);
        assert!(out.search.is_none());
        let avif = check(&out.avif, 96, 80, translucent);
        assert_eq!(avif.colr(b"nclx").unwrap(), [0, 1, 0, 13, 0, 6, 0x80], "sRGB, BT.601, full range");
        assert!(avif.colr(b"prof").is_none());
    }
}

#[test]
fn a_smooth_gradient_aims_no_lower_than_a_photo() {
    // thousands of colors in smooth ramps: a palette's worst case, so PNG output's target is low
    let pixels: Vec<RGBA> = (0..64 * 48).map(|i| RGBA::new((i % 64 * 1000 / 256) as u8, (i / 64 * 1300 / 256) as u8, 117, 255)).collect();
    let input = png(&pixels, 64, 48);
    assert!(compress(&input, &Settings::default()).unwrap().report.target - 9.0 < 62.3);
    let out = compress_avif(&input, &Settings::default()).unwrap();
    assert_eq!(out.target, 62.3);
    assert!(out.score >= 62.3);
}

#[test]
fn a_fixed_target_costs_more_the_higher_it_is() {
    let input = png(&photo(96, 80, false), 96, 80);
    let sizes: Vec<usize> = [50.0, 80.0]
        .iter()
        .map(|&target| {
            let out = compress_avif(&input, &Settings { target: Some(target), ..Default::default() }).unwrap();
            assert_eq!(out.target, target);
            assert!(out.score >= target, "{} for {target}", out.score);
            out.avif.len()
        })
        .collect();
    assert!(sizes[0] < sizes[1], "{sizes:?}");
}

#[test]
fn larger_images_search_fast_then_encode_slower() {
    let (w, h) = (900, 600);
    let out = compress_avif(&png(&photo(w, h, false), w, h), &Settings { target: Some(70.0), ..Default::default() }).unwrap();
    let (quality, bytes, score) = out.search.unwrap();
    assert!(score >= 70.0 && out.score >= 70.0);
    // speed 4's file, about a step below speed 8's quality, smaller than speed 8's
    assert_eq!(out.setting, "10bit-s4");
    assert!(out.quality <= quality + 2 && out.avif.len() < bytes, "{} at {} vs {bytes} at {quality}", out.avif.len(), out.quality);
    check(&out.avif, w, h, false);
}

#[test]
fn carries_the_colors() {
    let settings = Settings { target: Some(70.0), ..Default::default() };
    let input = png(&photo(96, 80, false), 96, 80);
    let profile: Vec<u8> = (0..301u32).map(|i| (i * 7) as u8).collect(); // libaom doesn't read it
    let iccp = [&b"Display P3\0\0"[..], &miniz_oxide::deflate::compress_to_vec_zlib(&profile, 6)].concat();
    let out = compress_avif(&codec::with_chunks(&input, &png_chunk(b"iCCP", &iccp)), &settings).unwrap();
    let avif = check(&out.avif, 96, 80, false);
    assert_eq!(avif.colr(b"prof").unwrap(), profile);
    assert_eq!(avif.colr(b"nclx").unwrap(), [0, 2, 0, 2, 0, 6, 0x80], "the profile's primaries and transfer");
    // cICP goes into the code points, with the YUV conversion's matrix
    let out = compress_avif(&codec::with_chunks(&input, &png_chunk(b"cICP", &[9, 16, 0, 1])), &settings).unwrap();
    let avif = check(&out.avif, 96, 80, false);
    assert_eq!(avif.colr(b"nclx").unwrap(), [0, 9, 0, 16, 0, 6, 0x80]);
    assert!(avif.colr(b"prof").is_none());
    // an sRGB image needs nothing more
    let out = compress_avif(&codec::with_chunks(&input, &png_chunk(b"sRGB", &[0])), &settings).unwrap();
    assert_eq!(check(&out.avif, 96, 80, false).colr(b"nclx").unwrap(), [0, 1, 0, 13, 0, 6, 0x80]);
}

#[test]
fn refuses_what_avif_cant_show() {
    let input = png(&photo(96, 80, false), 96, 80);
    let gamma = png_chunk(b"gAMA", &51_020u32.to_be_bytes());
    let limited = png_chunk(b"cICP", &[9, 16, 0, 0]); // BT.2020 primaries, PQ, limited-range RGB
    let animated = png_chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
    for (chunks, says) in [(gamma, "gAMA"), (limited, "cICP"), (animated, "animated")] {
        let error = compress_avif(&codec::with_chunks(&input, &chunks), &Settings::default()).err().expect(says).to_string();
        assert!(error.contains(says), "{error}");
    }
}

#[cfg(feature = "jpeg")]
mod photos {
    use super::*;

    /// A camera-like JPEG of the test photo, with an EXIF orientation if given.
    fn camera_jpeg(orientation: Option<u16>) -> Vec<u8> {
        let rgb: Vec<u8> = photo(160, 120, false).iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        let exif = orientation.map(|o| [&b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01"[..], &o.to_be_bytes(), &[0; 6]].concat());
        mozjpeg::encode(&rgb, 160, 120, 3, &mozjpeg::Options { quality: 95, full_chroma: true, ..Default::default() }, None, exif.as_deref()).unwrap()
    }

    #[test]
    fn a_photo_aims_at_tinypngs_median() {
        let out = compress_avif(&camera_jpeg(None), &Settings::default()).unwrap();
        assert_eq!((out.target, out.outcome), (62.3, "hit"));
        assert!(out.score >= out.target);
        check(&out.avif, 160, 120, false);
        // turned upright, as AVIF viewers wouldn't read EXIF's orientation
        let out = compress_avif(&camera_jpeg(Some(6)), &Settings::default()).unwrap();
        check(&out.avif, 120, 160, false);
    }
}

#[test]
fn command_line_picks_avif_from_the_name() {
    let dir = std::env::temp_dir().join(format!("pngdeer-avif-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (input, output) = (dir.join("in.png"), dir.join("out.avif"));
    std::fs::write(&input, png(&photo(96, 80, false), 96, 80)).unwrap();
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_pngdeer")).arg(&input).arg("-o").arg(&output).args(["--target", "70"]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let report = String::from_utf8(run.stdout).unwrap();
    assert!(report.starts_with(r#"{"format": "avif", "target": 70.00"#), "{report}");
    check(&std::fs::read(&output).unwrap(), 96, 80, false);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The bytes on every platform: libaom is built without fused multiply-add and with its math
/// functions routed to one implementation, its SIMD (NEON on 64-bit ARM) left out wherever it
/// differs from its C, its multithreaded encoder run with any number of threads (the same bytes
/// from 2 up, and one after another), and the YUV conversions are pngdeer's own, so the command line
/// and both browser builds write the same files. A change that alters pngdeer's output on purpose
/// updates these.
#[test]
fn same_bytes_everywhere() {
    let digest = |input: &[u8], target| {
        let out = compress_avif(input, &Settings { target, ..Default::default() }).unwrap();
        (out.avif.len(), crc32fast::hash(&out.avif))
    };
    let digests = [
        digest(&png(&icon(true), 96, 80), None),
        digest(&png(&photo(96, 80, true), 96, 80), None),
        digest(&png(&photo(900, 600, false), 900, 600), Some(70.0)),
    ];
    assert_eq!(digests, [(896, 1914741381), (2354, 1179717447), (1463, 2267430545)]);
}

/// libaom's multithreaded encoder writes the same file with any number of threads to run its
/// workers, one included (when they run one after another, as in the browser's single-threaded
/// build): same_bytes_everywhere's photo, whose encodes have several workers.
#[cfg(feature = "parallel")]
#[test]
fn any_number_of_threads_writes_the_same_file() {
    let input = png(&photo(900, 600, false), 900, 600);
    let digests: Vec<(usize, u32)> = [1, 2, 5]
        .iter()
        .map(|&threads| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let out = pool.install(|| compress_avif(&input, &Settings { target: Some(70.0), ..Default::default() }).map_err(|e| e.to_string()));
            let avif = out.unwrap().avif;
            (avif.len(), crc32fast::hash(&avif))
        })
        .collect();
    assert_eq!(digests, [(1463, 2267430545); 3]);
}
