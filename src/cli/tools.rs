//! --score and --upscale, the helpers bench/bench.py runs.

use std::fs;

use slimdeer::{codec, search};

use super::Error;

/// --score: SSIMULACRA2 of `other` against `file`, printed.
pub fn score(file: &str, other: &str) -> Result<(), Error> {
    let input = fs::read(file)?;
    if slimdeer::apng::is_animated(&input) {
        println!("{:.4}", frame_scores(&input, &fs::read(other)?)?.into_iter().fold(f64::INFINITY, f64::min));
        return Ok(());
    }
    let (a, b) = (decode_any(&input)?, decode_any(&fs::read(other)?)?);
    if (a.width, a.height) != (b.width, b.height) {
        return Err("images differ in size".into());
    }
    println!("{:.4}", search::score(&a.pixels, &b.pixels, a.width, a.height)?);
    Ok(())
}

/// --upscale: `file` enlarged k times, nearest neighbour, written to `output` as RGBA.
pub fn upscale(file: &str, output: &str, k: usize) -> Result<(), Error> {
    let image = codec::decode(&fs::read(file)?)?;
    let (w, h, pixels) = (image.width, image.height, &image.pixels);
    let big: Vec<_> = (0..h * k).flat_map(|y| (0..w * k).map(move |x| pixels[(y / k) * w + x / k])).collect();
    fs::write(output, codec::encode_rgba(&big, w * k, h * k))?;
    Ok(())
}

/// SSIMULACRA2 of each frame of animation `a` against the frame animation `b` shows at the same
/// time (b may have merged frames that repeat into longer ones), default images aside.
fn frame_scores(a: &[u8], b: &[u8]) -> Result<Vec<f64>, Error> {
    let (a, b) = (slimdeer::apng::decode(a)?, slimdeer::apng::decode(b)?);
    if (a.width, a.height) != (b.width, b.height) {
        return Err("animations differ in size".into());
    }
    let starts = |timing: &slimdeer::apng::Timing| {
        let seconds = |&(num, den): &(u16, u16)| num as f64 / if den == 0 { 100.0 } else { den as f64 };
        timing.delays.iter().scan(0.0, |t, d| Some(std::mem::replace(t, *t + seconds(d)))).collect::<Vec<f64>>()
    };
    let (sa, sb) = (starts(&a.timing), starts(&b.timing));
    fn frame(anim: &slimdeer::apng::Animation, i: usize) -> &[slimdeer::RGBA] {
        let (n, f) = (anim.width * anim.height, i + anim.timing.default_image as usize);
        &anim.pixels[f * n..(f + 1) * n]
    }
    sa.iter()
        .enumerate()
        .map(|(i, &t)| {
            let j = sb.iter().rposition(|&s| s <= t + 1e-9).unwrap_or(0);
            search::score(frame(&a, i), frame(&b, j), a.width, a.height)
        })
        .collect()
}

/// A PNG, a WebP file with the webp feature, or a JPEG with the jpeg feature (turned as its EXIF
/// orientation says, as viewers show it), as RGBA.
fn decode_any(bytes: &[u8]) -> Result<codec::Bitmap, Error> {
    #[cfg(feature = "jpeg")]
    if slimdeer::jpeg::is_jpeg(bytes) {
        let photo = slimdeer::jpeg::decode(bytes)?;
        let (pixels, width, height) = slimdeer::jpeg::oriented(photo.pixels, photo.width, photo.height, photo.orientation);
        return Ok(codec::Bitmap { pixels, width, height });
    }
    #[cfg(feature = "webp")]
    if bytes.starts_with(b"RIFF") {
        return slimdeer::webp::decode(bytes);
    }
    codec::decode(bytes)
}
