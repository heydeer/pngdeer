//! pngdeer on the command line, writing the same file as the browser build (web/):
//!
//!   pngdeer IN.png -o OUT.png [--target auto|T] [--preview-pixels N] [--variants SPEC]
//!
//! Writes the smallest palette PNG that keeps IN's quality as SSIMULACRA2 measures it: a score
//! chosen per image (auto, the default; see search.rs), or at least T. Prints a JSON report: the
//! target, palette size and variant, score, evaluations, time per stage, outcome and bytes.
//!
//! --preview-pixels N: images over N pixels are searched on a downscaled preview, then checked at
//!   full size (default pngdeer::PREVIEW_PIXELS, as in the browser). 0 searches at full size: about
//!   1% smaller files, several times slower on large images.
//! --variants SPEC: the remapping variants tried at each palette size, e.g. 50,0,0r2,w0 (the
//!   default; see search::parse_variants).
//!
//! Two helpers for bench/bench.py:
//!   pngdeer IN.png --score OTHER.png        SSIMULACRA2 of OTHER against IN, as the search scores
//!   pngdeer IN.png -o OUT.png --upscale K   nearest-neighbour K× enlargement, written as RGBA

use std::{env, fs, process, sync::OnceLock, time::Instant};

use pngdeer::{codec, search};

const USAGE: &str = "usage: pngdeer IN.png -o OUT.png [--target auto|T] [--preview-pixels N] [--variants SPEC]
       pngdeer IN.png --score OTHER.png
       pngdeer IN.png -o OUT.png --upscale K";

fn main() {
    if let Err(e) = run() {
        eprintln!("pngdeer: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut settings = pngdeer::Settings { clock: Some(now_ms), ..Default::default() };
    let (mut input, mut output, mut score_against, mut upscale) = (None, None, None, None::<usize>);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "-o" => output = Some(value()?),
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
        let (a, b) = (codec::decode(&input)?, codec::decode(&fs::read(other)?)?);
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
    let result = pngdeer::compress(&input, &settings)?;
    fs::write(&output, &result.png)?;
    println!("{}", result.json());
    Ok(())
}

fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
