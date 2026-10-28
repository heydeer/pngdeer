//! Animated PNGs (APNG). `decode` draws each frame onto the canvas as a viewer shows it (blend and
//! dispose operations applied), so frames are searched and scored as the images people see.
//! `encode` writes frames on one palette (PLTE serves every frame of a palette APNG): the first in
//! full, each later one as the rectangle where it differs from the frame before, drawn over it.
//! Inside that rectangle, pixels that stay as they were become a fully transparent entry blended
//! over the canvas, where the palette has one and every changed pixel is opaque (blending a
//! translucent one would mix it with what was there): runs of one entry deflate better. A frame
//! that changes nothing adds its delay to the frame before.

use std::io::Cursor;

use crate::codec::{self, Error};
use crate::RGBA;

/// An animation as shown: every frame a full canvas.
pub struct Animation {
    pub width: usize,
    pub height: usize,
    /// The frames' canvases one after another, each `width` × `height`.
    pub pixels: Vec<RGBA>,
    pub frames: usize,
    pub timing: Timing,
}

/// What `encode` keeps from the input besides the pixels.
#[derive(Clone, Debug)]
pub struct Timing {
    /// Each animation frame's delay: numerator and denominator in seconds (a denominator of 0
    /// means 100, as in fcTL).
    pub delays: Vec<(u16, u16)>,
    /// How many times it plays, 0 for endlessly (acTL).
    pub plays: u32,
    /// The first image is a default image for viewers without APNG support, not a frame (no fcTL
    /// before IDAT): `delays` then has one entry fewer than the images.
    pub default_image: bool,
}

/// Whether a PNG file is animated: an acTL chunk ahead of the image data.
pub fn is_animated(png: &[u8]) -> bool {
    let mut pos = 8;
    while pos + 8 <= png.len() {
        let kind = &png[pos + 4..pos + 8];
        if kind == b"IDAT" {
            return false;
        }
        if kind == b"acTL" {
            return true;
        }
        pos += 12 + u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
    }
    false
}

pub fn decode(bytes: &[u8]) -> Result<Animation, Error> {
    use png::{BlendOp, DisposeOp};
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    let control = reader.info().animation_control.ok_or("not an animated PNG")?;
    let default_image = reader.info().frame_control.is_none();
    let images = control.num_frames as usize + default_image as usize;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let mut canvas = vec![RGBA::new(0, 0, 0, 0); w * h];
    let mut pixels = Vec::with_capacity(w * h * images);
    let mut delays = Vec::with_capacity(control.num_frames as usize);
    for i in 0..images {
        let info = reader.next_frame(&mut buf)?;
        let image = codec::rgba_of(&buf[..info.buffer_size()], info.color_type)?;
        if default_image && i == 0 {
            pixels.extend_from_slice(&image);
            continue;
        }
        let control = *reader.info().frame_control.as_ref().ok_or("an animation frame without fcTL")?;
        let (fw, fh, fx, fy) = (control.width as usize, control.height as usize, control.x_offset as usize, control.y_offset as usize);
        if fx + fw > w || fy + fh > h {
            return Err("an animation frame outside the canvas".into());
        }
        let rows = |canvas: &mut Vec<RGBA>, f: &mut dyn FnMut(&mut [RGBA], usize)| {
            for y in 0..fh {
                f(&mut canvas[(fy + y) * w + fx..(fy + y) * w + fx + fw], y);
            }
        };
        let first = delays.is_empty();
        // the first frame's "previous" is the empty canvas (the APNG spec)
        let saved: Vec<RGBA> = if control.dispose_op == DisposeOp::Previous && !first {
            (0..fh).flat_map(|y| canvas[(fy + y) * w + fx..(fy + y) * w + fx + fw].to_vec()).collect()
        } else {
            Vec::new()
        };
        rows(&mut canvas, &mut |row, y| {
            for (dst, &src) in row.iter_mut().zip(&image[y * fw..(y + 1) * fw]) {
                *dst = if control.blend_op == BlendOp::Source { src } else { over(src, *dst) };
            }
        });
        pixels.extend_from_slice(&canvas);
        delays.push((control.delay_num, control.delay_den));
        match control.dispose_op {
            DisposeOp::None => {}
            DisposeOp::Background => rows(&mut canvas, &mut |row, _| row.fill(RGBA::new(0, 0, 0, 0))),
            DisposeOp::Previous if first => rows(&mut canvas, &mut |row, _| row.fill(RGBA::new(0, 0, 0, 0))),
            DisposeOp::Previous => rows(&mut canvas, &mut |row, y| row.copy_from_slice(&saved[y * fw..(y + 1) * fw])),
        }
    }
    Ok(Animation { width: w, height: h, pixels, frames: images, timing: Timing { delays, plays: control.num_plays, default_image } })
}

/// `src` blended over `dst`, as the APNG spec's sample code does it in 8 bits.
fn over(src: RGBA, dst: RGBA) -> RGBA {
    match (src.a, dst.a) {
        (255, _) | (_, 0) => src,
        (0, _) => dst,
        (sa, da) => {
            let (u, v) = (sa as u32 * 255, (255 - sa as u32) * da as u32);
            let mix = |s: u8, d: u8| ((s as u32 * u + d as u32 * v) / (u + v)) as u8;
            RGBA::new(mix(src.r, dst.r), mix(src.g, dst.g), mix(src.b, dst.b), ((u + v) / 255) as u8)
        }
    }
}

/// One frame as written: its rectangle, how it blends, its delay and its compressed rows.
struct Written {
    rect: (usize, usize, usize, usize), // x, y, width, height
    over: bool,
    delay: (u16, u16),
    data: Vec<u8>,
}

/// A palette APNG of `images` canvases of `w` × `h` indices into `palette` (the default image
/// first, when `timing` says there is one), with `timing`'s delays and plays.
pub fn encode(palette: &[RGBA], indices: &[u8], w: usize, h: usize, timing: &Timing) -> Vec<u8> {
    let n = w * h;
    let images: Vec<&[u8]> = indices.chunks_exact(n).collect();
    // a fully transparent entry to leave pixels as they are with, unless it would cost a bit of depth
    let mut palette = palette.to_vec();
    let mut clear = palette.iter().position(|c| c.a == 0);
    if clear.is_none() && !matches!(palette.len(), 2 | 4 | 16 | 256) {
        clear = Some(palette.len());
        palette.push(RGBA::new(0, 0, 0, 0));
    }
    let (palette, renumber, depth) = codec::palette_order(&palette);
    let rows = |region: &[u8], width: usize| codec::best_idat(&codec::packed_rows(region, width, &renumber, depth), 1);

    let frames = &images[timing.default_image as usize..];
    let mut written: Vec<Written> = Vec::with_capacity(frames.len());
    for (f, &image) in frames.iter().enumerate() {
        let delay = timing.delays[f];
        if f == 0 {
            // each play starts on an empty canvas
            written.push(Written { rect: (0, 0, w, h), over: false, delay, data: rows(image, w) });
            continue;
        }
        let before = frames[f - 1];
        let Some((x0, y0, x1, y1)) = changed(before, image, w, h) else {
            let last = written.last_mut().unwrap();
            let den = |d: u16| if d == 0 { 100 } else { d };
            if den(last.delay.1) == den(delay.1) && last.delay.0 as u32 + delay.0 as u32 <= u16::MAX as u32 {
                last.delay.0 += delay.0;
            } else {
                written.push(Written { rect: (0, 0, 1, 1), over: false, delay, data: rows(&image[..1], 1) });
            }
            continue;
        };
        let (rw, rh) = (x1 - x0, y1 - y0);
        let region: Vec<u8> = (y0..y1).flat_map(|y| image[y * w + x0..y * w + x1].iter().copied()).collect();
        let mut frame = Written { rect: (x0, y0, rw, rh), over: false, delay, data: rows(&region, rw) };
        if let Some(clear) = clear {
            let below = |i: usize| before[(y0 + i / rw) * w + x0 + i % rw];
            let kept = |i: usize| below(i) == region[i];
            let alpha = |index: u8| palette[renumber[index as usize] as usize].a;
            // over an opaque pixel, or onto a clear one, a pixel replaces what was there
            if (0..region.len()).all(|i| kept(i) || alpha(region[i]) == 255 || alpha(below(i)) == 0) {
                let blended: Vec<u8> = (0..region.len()).map(|i| if kept(i) { clear as u8 } else { region[i] }).collect();
                let data = rows(&blended, rw);
                if data.len() < frame.data.len() {
                    frame = Written { over: true, data, ..frame };
                }
            }
        }
        written.push(frame);
    }

    let mut png = codec::start(w, h, depth, 3);
    codec::chunk(&mut png, b"acTL", &[(written.len() as u32).to_be_bytes(), timing.plays.to_be_bytes()].concat());
    codec::palette_chunks(&mut png, &palette);
    if timing.default_image {
        codec::chunk(&mut png, b"IDAT", &rows(images[0], w));
    }
    let mut sequence = 0u32;
    for (i, frame) in written.iter().enumerate() {
        let (x, y, fw, fh) = frame.rect;
        let control = [
            &sequence.to_be_bytes()[..],
            &(fw as u32).to_be_bytes(),
            &(fh as u32).to_be_bytes(),
            &(x as u32).to_be_bytes(),
            &(y as u32).to_be_bytes(),
            &frame.delay.0.to_be_bytes(),
            &frame.delay.1.to_be_bytes(),
            &[0, frame.over as u8], // dispose: none; blend: source or over
        ]
        .concat();
        codec::chunk(&mut png, b"fcTL", &control);
        sequence += 1;
        if i == 0 && !timing.default_image {
            codec::chunk(&mut png, b"IDAT", &frame.data);
        } else {
            codec::chunk(&mut png, b"fdAT", &[&sequence.to_be_bytes()[..], &frame.data].concat());
            sequence += 1;
        }
    }
    codec::chunk(&mut png, b"IEND", &[]);
    png
}

/// The bounding box (x0, y0, x1, y1), ends exclusive, of the pixels where two canvases differ.
fn changed(a: &[u8], b: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let differs = |y: usize| a[y * w..(y + 1) * w] != b[y * w..(y + 1) * w];
    let y0 = (0..h).find(|&y| differs(y))?;
    let y1 = (y0..h).rev().find(|&y| differs(y)).unwrap() + 1;
    let column = |x: usize| (y0..y1).any(|y| a[y * w + x] != b[y * w + x]);
    let x0 = (0..w).find(|&x| column(x)).unwrap();
    let x1 = (x0..w).rev().find(|&x| column(x)).unwrap() + 1;
    Some((x0, y0, x1, y1))
}
