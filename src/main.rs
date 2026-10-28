//! pngdeer on the command line, writing the same file as the browser build (web/):
//!
//!   pngdeer IN.png -o OUT.png [--format png|webp] [--target auto|T] [--preview-pixels N] [--variants SPEC]
//!
//! Writes the smallest palette PNG that keeps IN's quality as SSIMULACRA2 measures it: a score
//! chosen per image (auto, the default; see search.rs), or at least T. Prints a JSON report: the
//! target, palette size and variant, score, evaluations, time per stage, outcome and bytes.
//!
//! --format webp (also chosen by an OUT ending in .webp): the smaller of a palette image as lossless
//!   WebP and the lossiest WebP that keeps the target. Automatic quality there is TinyPNG's WebP's:
//!   the PNG's where the palette wins, else lossy quality 70's score (see webp.rs). Its report names
//!   the winner, its quality and score, both candidates' sizes, and has the PNG's report.
//! --webp-method N, --webp-sharp-yuv 0|1, --webp-alpha-filter N, --webp-autofilter 0|1,
//!   --webp-large-alpha-effort N: the lossy candidates' settings;
//!   --webp-lossless-level N: also a slower lossless level where lossless may win;
//!   --webp-reference Q|none: automatic quality's reference lossy quality (see webp.rs)
//!   (pngdeer::WebpOptions, for experiments).
//!
//! --preview-pixels N: images over N pixels are searched on a downscaled preview, then checked at
//!   full size (default pngdeer::PREVIEW_PIXELS, as in the browser). 0 searches at full size: about
//!   1% smaller files, several times slower on large images.
//! --variants SPEC: the remapping variants tried at each palette size, e.g. 50,0,0r2,w0 (the
//!   default; see search::parse_variants).
//!
//! Two helpers for bench/bench.py:
//!   pngdeer IN.png --score OTHER            SSIMULACRA2 of OTHER (PNG or WebP) against IN, as the
//!                                           search scores
//!   pngdeer IN.png -o OUT.png --upscale K   nearest-neighbour K× enlargement, written as RGBA

use std::{env, fs, process, sync::OnceLock, time::Instant};

use pngdeer::{codec, search};

const USAGE: &str = "usage: pngdeer IN.png -o OUT.png [--format png|webp] [--target auto|T] [--preview-pixels N] [--variants SPEC]
       pngdeer IN.png --score OTHER
       pngdeer IN.png -o OUT.png --upscale K";

fn main() {
    if let Err(e) = run() {
        eprintln!("pngdeer: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut settings = pngdeer::Settings { clock: Some(now_ms), ..Default::default() };
    let (mut input, mut output, mut score_against, mut upscale, mut format) = (None, None, None, None::<usize>, None);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "-o" => output = Some(value()?),
            "--format" => format = Some(value()?),
            "--webp-method" => settings.webp.method = value()?.parse()?,
            "--webp-lossless-level" => settings.webp.slow_lossless_level = Some(value()?.parse()?),
            "--webp-sharp-yuv" => settings.webp.sharp_yuv = value()? != "0",
            "--webp-alpha-filter" => settings.webp.alpha_filtering = value()?.parse()?,
            "--webp-autofilter" => settings.webp.autofilter = value()? != "0",
            "--webp-large-alpha-effort" => settings.webp.large_alpha_effort = value()?.parse()?,
            "--webp-reference" => {
                let v = value()?;
                settings.webp.reference_quality = if v == "none" { None } else { Some(v.parse()?) };
            }
            "--target" => {
                let v = value()?;
                settings.target = if v == "auto" { None } else { Some(v.parse().map_err(|_| format!("bad --target: {v}"))?) };
            }
            "--preview-pixels" => settings.preview_pixels = value()?.parse()?,
            "--variants" => settings.variants = search::parse_variants(&value()?)?,
            "--score" => score_against = Some(value()?),
            "--upscale" => upscale = Some(value()?.parse()?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ if arg.starts_with('-') || input.is_some() => return Err(format!("unexpected argument: {arg}\n{USAGE}").into()),
            _ => input = Some(arg),
        }
    }
    let input = fs::read(input.ok_or(USAGE)?)?;

    if let Some(other) = score_against {
        let (a, b) = (codec::decode(&input)?, decode_any(&fs::read(other)?)?);
        if (a.width, a.height) != (b.width, b.height) {
            return Err("images differ in size".into());
        }
        println!("{:.4}", search::score(&a.pixels, &b.pixels, a.width, a.height)?);
        return Ok(());
    }
    let output = output.ok_or(USAGE)?;
    if let Some(k) = upscale {
        let image = codec::decode(&input)?;
        let (w, h, pixels) = (image.width, image.height, &image.pixels);
        let big: Vec<_> = (0..h * k).flat_map(|y| (0..w * k).map(move |x| pixels[(y / k) * w + x / k])).collect();
        fs::write(&output, codec::encode_rgba(&big, w * k, h * k))?;
        return Ok(());
    }
    let webp = match format.as_deref() {
        Some("webp") => true,
        Some("png") => false,
        Some(other) => return Err(format!("unknown --format: {other} (png or webp)").into()),
        None => output.to_lowercase().ends_with(".webp"),
    };
    if webp {
        return write_webp(&input, &output, &settings);
    }
    let result = pngdeer::compress(&input, &settings)?;
    fs::write(&output, &result.png)?;
    println!("{}", result.json());
    Ok(())
}

#[cfg(feature = "webp")]
fn write_webp(input: &[u8], output: &str, settings: &pngdeer::Settings) -> Result<(), Box<dyn std::error::Error>> {
    let result = pngdeer::compress_webp(input, settings)?;
    fs::write(output, &result.webp)?;
    println!("{}", result.json());
    Ok(())
}

#[cfg(not(feature = "webp"))]
fn write_webp(_: &[u8], _: &str, _: &pngdeer::Settings) -> Result<(), Box<dyn std::error::Error>> {
    Err("this pngdeer was built without the webp feature".into())
}

/// A PNG, or a WebP file with the webp feature, as RGBA.
fn decode_any(bytes: &[u8]) -> Result<codec::Bitmap, Box<dyn std::error::Error>> {
    #[cfg(feature = "webp")]
    if bytes.starts_with(b"RIFF") {
        return pngdeer::webp::decode(bytes);
    }
    codec::decode(bytes)
}

fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
