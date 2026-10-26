//! PNG reading and writing. Decoding and the writer are pure Rust; `optimize` is oxipng, whose C
//! libdeflate builds without libc for WebAssembly (the `freestanding` feature).
//!
//! The writer is quick rather than small: transparent palette entries first (short tRNS), the
//! smallest bit depth the palette allows, and the best of a few row-filter strategies at a middling
//! deflate level. Its sizes rank candidates; the file that ships goes through `optimize` (oxipng).

use std::io::Cursor;

use crate::RGBA;
use miniz_oxide::deflate::compress_to_vec_zlib;

pub type Error = Box<dyn std::error::Error>;

pub struct Bitmap {
    pub pixels: Vec<RGBA>,
    pub width: usize,
    pub height: usize,
}

/// Any PNG as 8-bit RGBA. Color-space chunks (gAMA, iCCP, ...) are ignored, as the benchmark does.
pub fn decode(bytes: &[u8]) -> Result<Bitmap, Error> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf)?;
    let data = &buf[..info.buffer_size()];
    let pixels = match info.color_type {
        png::ColorType::Rgba => data.as_chunks::<4>().0.iter().map(|p| RGBA::new(p[0], p[1], p[2], p[3])).collect(),
        png::ColorType::Rgb => data.as_chunks::<3>().0.iter().map(|p| RGBA::new(p[0], p[1], p[2], 255)).collect(),
        png::ColorType::GrayscaleAlpha => data.as_chunks::<2>().0.iter().map(|p| RGBA::new(p[0], p[0], p[0], p[1])).collect(),
        png::ColorType::Grayscale => data.iter().map(|&g| RGBA::new(g, g, g, 255)).collect(),
        png::ColorType::Indexed => return Err("palette was not expanded".into()),
    };
    Ok(Bitmap { pixels, width: info.width as usize, height: info.height as usize })
}

pub fn encode_palette(palette: &[RGBA], indices: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (palette, depth, rows) = pack(palette, indices, w);
    let mut png = start(w, h, depth, 3);
    chunk(&mut png, b"PLTE", &palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect::<Vec<u8>>());
    if let Some(last) = palette.iter().rposition(|c| c.a < 255) {
        chunk(&mut png, b"tRNS", &palette[..=last].iter().map(|c| c.a).collect::<Vec<u8>>());
    }
    chunk(&mut png, b"IDAT", &best_idat(&rows, 1));
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Rough size of encode_palette's output (unfiltered, fast deflate), to rank candidates cheaply.
pub fn palette_size_hint(palette: &[RGBA], indices: &[u8], w: usize) -> usize {
    let (palette, _, rows) = pack(palette, indices, w);
    compress_to_vec_zlib(&filter_rows(&rows, 1, Strategy::Fixed(0)), 1).len() + palette.len() * 4
}

/// The palette reordered for tRNS (translucent entries first), its bit depth, and packed rows.
fn pack(palette: &[RGBA], indices: &[u8], w: usize) -> (Vec<RGBA>, u8, Vec<Vec<u8>>) {
    let mut order: Vec<usize> = (0..palette.len()).collect();
    order.sort_by_key(|&i| palette[i].a == 255); // stable: translucent entries first
    let mut renumber = vec![0u8; palette.len()];
    for (new, &old) in order.iter().enumerate() {
        renumber[old] = new as u8;
    }
    let palette: Vec<RGBA> = order.iter().map(|&i| palette[i]).collect();
    let depth = match palette.len() {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    };
    let per_byte = 8 / depth;
    let rows: Vec<Vec<u8>> = indices
        .chunks_exact(w)
        .map(|line| {
            let mut row = vec![0u8; w.div_ceil(per_byte)];
            for (x, &i) in line.iter().enumerate() {
                row[x / per_byte] |= renumber[i as usize] << (8 - depth * (1 + x % per_byte));
            }
            row
        })
        .collect();
    (palette, depth as u8, rows)
}

pub fn encode_rgba(pixels: &[RGBA], w: usize, h: usize) -> Vec<u8> {
    let rows: Vec<Vec<u8>> = pixels.chunks_exact(w).map(|line| line.iter().flat_map(|p| [p.r, p.g, p.b, p.a]).collect()).collect();
    let mut png = start(w, h, 8, 6);
    chunk(&mut png, b"IDAT", &best_idat(&rows, 4));
    chunk(&mut png, b"IEND", &[]);
    png
}

/// oxipng -o 2 --strip safe on a PNG file, as bench.py finishes its outputs (unchanged without the
/// `oxipng` feature).
#[cfg(feature = "oxipng")]
pub fn optimize(png: &[u8]) -> Result<Vec<u8>, Error> {
    let mut options = oxipng::Options::from_preset(2);
    options.strip = oxipng::StripChunks::Safe;
    Ok(oxipng::optimize_from_memory(png, &options)?)
}

#[cfg(not(feature = "oxipng"))]
pub fn optimize(png: &[u8]) -> Result<Vec<u8>, Error> {
    Ok(png.to_vec())
}

fn start(w: usize, h: usize, depth: u8, color_type: u8) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[depth, color_type, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32fast::hash(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}

#[derive(Clone, Copy)]
enum Strategy {
    Fixed(u8), // the same PNG filter type on every row
    Adaptive,  // per row, the filter with the smallest sum of absolute (signed) bytes
}

/// The filter strategy that deflates smallest at miniz level 4. No harder pass: the final file is
/// recompressed by `optimize` anyway.
fn best_idat(rows: &[Vec<u8>], bpp: usize) -> Vec<u8> {
    let strategies = [Strategy::Fixed(0), Strategy::Fixed(1), Strategy::Fixed(2), Strategy::Fixed(4), Strategy::Adaptive];
    strategies.into_iter().map(|s| compress_to_vec_zlib(&filter_rows(rows, bpp, s), 4)).min_by_key(|idat| idat.len()).unwrap()
}

fn filter_rows(rows: &[Vec<u8>], bpp: usize, strategy: Strategy) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.iter().map(|r| r.len() + 1).sum());
    let zero = vec![0u8; rows.first().map_or(0, |r| r.len())];
    let mut candidate = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        let prev = if y == 0 { &zero } else { &rows[y - 1] };
        let kind = match strategy {
            Strategy::Fixed(k) => k,
            Strategy::Adaptive => (0..5u8)
                .min_by_key(|&k| {
                    apply(k, row, prev, bpp, &mut candidate);
                    candidate.iter().map(|&b| (b as i8).unsigned_abs() as u64).sum::<u64>()
                })
                .unwrap(),
        };
        apply(kind, row, prev, bpp, &mut candidate);
        out.push(kind);
        out.extend_from_slice(&candidate);
    }
    out
}

fn apply(kind: u8, row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>) {
    out.clear();
    for i in 0..row.len() {
        let (a, b) = (if i >= bpp { row[i - bpp] } else { 0 }, prev[i]);
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let predicted = match kind {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            _ => paeth(a, b, c),
        };
        out.push(row[i].wrapping_sub(predicted));
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}
