//! pngdeer on the command line, writing the same files as the browser build (web/):
//!
//!   pngdeer [OPTIONS] FILE...     (HELP below)
//!
//! Each FILE, a PNG or a JPEG, comes out smaller at a quality picked for it, next to it as
//! NAME.min.EXT, in a folder with -d, or where -o says. Like cp and pngquant, it says nothing unless
//! something goes wrong: -v says what each file came to (on stderr), --json prints each file's
//! report (on stdout, one a line). A file that fails doesn't stop the others; pngdeer then exits 1,
//! and 2 on a command it can't run. A name it makes up never replaces an input; -o can.
//!
//! A PNG comes out as the smallest palette PNG that keeps its quality as SSIMULACRA2 measures it: a
//! score chosen per image (auto, the default; see search.rs), or at least --target's. Its report
//! gives the target, palette size and variant, score, evaluations, time per stage, outcome and bytes.
//! An animated PNG stays animated, every frame at least the target on one palette (the report adds
//! "frames", and its score is the worst frame's).
//!
//! A JPEG comes out as a JPEG: re-encoded at the lowest quality that keeps the target, never bigger
//! than the input (pngdeer::jpeg). Automatic quality there is about TinyPNG's JPEG's, which goes by
//! the photo's size (see jpeg.rs). Its report gives the quality, score and searches; the ICC profile
//! (unless sRGB's) and the EXIF orientation stay, the rest of EXIF goes. As WebP it comes out as
//! lossy WebP, turned as its orientation says (webp.rs).
//!
//! --format webp (or -o's name ending in .webp), for a PNG: the smaller of a palette image as
//!   lossless WebP and the lossiest WebP that keeps the target. Automatic quality there is TinyPNG's
//!   WebP's: the PNG's where the palette wins, else lossy quality 70's score (see webp.rs). Its report
//!   names the winner, its quality and score, both candidates' sizes, and has the PNG's report.
//! --format avif (or -o's name ending in .avif), for a PNG or a JPEG: the smallest AVIF, from libaom,
//!   that keeps the target as Chrome shows it. Automatic quality there is about TinyPNG's AVIF's: a
//!   JPEG photo's 62.3, a PNG's PNG target less 9 but at least 62.3 (see avif.rs). Its report gives
//!   the setting, quality, score and searches.
//!
//! For experiments, left out of the help:
//! --webp-method N, --webp-sharp-yuv 0|1, --webp-alpha-filter N, --webp-autofilter 0|1,
//!   --webp-large-alpha-effort N: the lossy candidates' settings;
//!   --webp-lossless-level N: also a slower lossless level where lossless may win;
//!   --webp-reference Q|none: automatic quality's reference lossy quality (see webp.rs)
//!   (pngdeer::WebpOptions).
//! --preview-pixels N: images over N pixels are searched on a downscaled preview, then checked at
//!   full size (default pngdeer::PREVIEW_PIXELS, as in the browser). 0 searches at full size: about
//!   1% smaller files, several times slower on large images.
//! --variants SPEC: the remapping variants tried at each palette size, e.g. 50,0,0r2,w0 (the
//!   default; see search::parse_variants).
//!
//! Two helpers for bench/bench.py:
//!   pngdeer IN.png --score OTHER            SSIMULACRA2 of OTHER (PNG, WebP or JPEG) against IN, as the
//!                                           search scores; for an animated IN, the worst frame's,
//!                                           each against the frame OTHER shows at that time (in the
//!                                           help too)
//!   pngdeer IN.png -o OUT.png --upscale K   nearest-neighbour K× enlargement, written as RGBA

use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process,
    sync::OnceLock,
    time::Instant,
};

use pngdeer::{codec, search};

const HELP: &str = "pngdeer: smaller PNG and JPEG files, or WebP and AVIF, that look the same

Usage: pngdeer [OPTIONS] FILE...

Writes each FILE, a PNG or a JPEG, smaller, at a quality picked for it, next to it
as NAME.min.EXT. Says nothing unless something goes wrong.

Examples:
  pngdeer photo.jpg                 writes photo.min.jpg
  pngdeer *.png -d dist             writes dist/NAME.png for each
  pngdeer logo.png --format webp    writes logo.min.webp
  pngdeer in.png -o out.avif        writes out.avif, the format as its name says
  pngdeer shot.png --target 90      keeps a score of at least 90
  pngdeer - < in.png > out.png      from stdin to stdout

Options:
  -o, --output FILE    Write the one FILE's output here (- for stdout)
  -d, --dir DIR        Write the outputs into DIR, under the inputs' names
      --format FORMAT  png, jpeg, webp or avif [default: the input's, or -o's]
                       (a PNG can't become a JPEG, nor a JPEG a PNG)
      --target T       The SSIMULACRA2 score to keep, 0 to 100: 90 looks lossless,
                       70 is high quality [default: auto, about TinyPNG's, per image]
  -v, --verbose        Say what each file came to, on stderr
      --json           Print each file's report, a JSON line, on stdout
  -h, --help           Print this help
  -V, --version        Print the version

Also:
  pngdeer ORIGINAL --score OTHER    Print OTHER's SSIMULACRA2 score against ORIGINAL";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

fn main() {
    let code = match parse(env::args().skip(1)) {
        Ok(Some(command)) => command.run(),
        Ok(None) => 0,
        Err(message) => {
            eprintln!("pngdeer: {message}\nTry 'pngdeer --help' for more.");
            2
        }
    };
    process::exit(code);
}

type Error = Box<dyn std::error::Error>;

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
    Webp,
    Avif,
}

impl Format {
    fn named(name: &str) -> Option<Format> {
        match name.to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpeg" | "jpg" => Some(Format::Jpeg),
            "webp" => Some(Format::Webp),
            "avif" => Some(Format::Avif),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Avif => "avif",
        }
    }
}

/// The format an output's name says, by its extension.
fn named_by(output: Option<&str>) -> Option<Format> {
    Format::named(&Path::new(output?).extension()?.to_string_lossy())
}

/// Where a file's output goes.
enum Destination {
    Stdout,
    /// A path, and whether pngdeer made it up (one that mustn't replace the input).
    File(PathBuf, bool),
}

impl Destination {
    fn name(&self) -> String {
        match self {
            Destination::Stdout => "-".into(),
            Destination::File(path, _) => path.display().to_string(),
        }
    }
}

struct Command {
    files: Vec<String>,
    output: Option<String>,
    dir: Option<PathBuf>,
    format: Option<Format>,
    verbose: bool,
    json: bool,
    score_against: Option<String>,
    upscale: Option<usize>,
    settings: pngdeer::Settings,
}

/// The command line, or None where it asked for the help or the version (printed), or what's wrong
/// with it.
fn parse(args: impl Iterator<Item = String>) -> Result<Option<Command>, String> {
    let mut command = Command {
        files: Vec::new(),
        output: None,
        dir: None,
        format: None,
        verbose: false,
        json: false,
        score_against: None,
        upscale: None,
        settings: pngdeer::Settings { clock: Some(now_ms), ..Default::default() },
    };
    let settings = &mut command.settings;
    let mut args = args.peekable();
    let mut options = true;
    while let Some(arg) = args.next() {
        if !options || arg == "-" || !arg.starts_with('-') {
            command.files.push(arg);
            continue;
        }
        // --name=value as well as --name value
        let (name, mut inline) = match arg.split_once('=') {
            Some((name, value)) if arg.starts_with("--") => (name.to_string(), Some(value.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = || inline.take().or_else(|| args.next()).ok_or_else(|| format!("{name} needs a value"));
        match name.as_str() {
            "--" => options = false,
            "-o" | "--output" => command.output = Some(value()?),
            "-d" | "--dir" => command.dir = Some(value()?.into()),
            "--format" => {
                let v = value()?;
                command.format = Some(Format::named(&v).ok_or_else(|| format!("unknown format: {v} (png, jpeg, webp or avif)"))?);
            }
            "--target" => {
                let v = value()?;
                settings.target = match v.as_str() {
                    "auto" => None,
                    _ => match v.parse::<f64>() {
                        Ok(t) if (0.0..=100.0).contains(&t) => Some(t),
                        _ => return Err(format!("--target: a score from 0 to 100, or auto, not {v}")),
                    },
                };
            }
            "-v" | "--verbose" => command.verbose = true,
            "--json" => command.json = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("pngdeer {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--score" => command.score_against = Some(value()?),
            "--upscale" => command.upscale = Some(number(&name, value()?)?),
            "--preview-pixels" => settings.preview_pixels = number(&name, value()?)?,
            "--variants" => settings.variants = search::parse_variants(&value()?).map_err(|e| format!("--variants: {e}"))?,
            "--webp-method" => settings.webp.method = number(&name, value()?)?,
            "--webp-lossless-level" => settings.webp.slow_lossless_level = Some(number(&name, value()?)?),
            "--webp-sharp-yuv" => settings.webp.sharp_yuv = value()? != "0",
            "--webp-alpha-filter" => settings.webp.alpha_filtering = number(&name, value()?)?,
            "--webp-autofilter" => settings.webp.autofilter = value()? != "0",
            "--webp-large-alpha-effort" => settings.webp.large_alpha_effort = number(&name, value()?)?,
            "--webp-reference" => {
                let v = value()?;
                settings.webp.reference_quality = if v == "none" { None } else { Some(number(&name, v)?) };
            }
            _ => return Err(format!("unknown option: {arg}")),
        }
        if inline.is_some() {
            return Err(format!("{name} takes no value"));
        }
    }
    let files = &command.files;
    if files.is_empty() {
        return Err("no input files".into());
    }
    if (command.score_against.is_some() || command.upscale.is_some()) && files.len() > 1 {
        return Err("--score and --upscale take one file".into());
    }
    if command.upscale.is_some() && command.output.is_none() {
        return Err("--upscale needs -o".into());
    }
    if command.output.is_some() && command.dir.is_some() {
        return Err("-o and -d both say where to write; give one".into());
    }
    if command.output.is_some() && files.len() > 1 {
        return Err("-o names one output; for several files, use -d DIR".into());
    }
    if files.iter().filter(|f| *f == "-").count() > 1 {
        return Err("stdin (-) can be read once".into());
    }
    if files.iter().any(|f| f == "-") && command.dir.is_some() {
        return Err("stdin (-) has no name to write under in -d's folder; use -o".into());
    }
    if let (Some(format), Some(named)) = (command.format, named_by(command.output.as_deref())) {
        if format != named {
            return Err(format!("--format {}, but -o's name says {}", format.extension(), named.extension()));
        }
    }
    let to_stdout = match &command.output {
        Some(output) => output == "-",
        None => command.dir.is_none() && files.iter().any(|f| f == "-"),
    };
    if command.json && to_stdout {
        return Err("--json and the image can't both go to stdout".into());
    }
    Ok(Some(command))
}

impl Command {
    /// Runs it; the exit code.
    fn run(&self) -> i32 {
        let helper = if let Some(other) = &self.score_against {
            Some(score(&self.files[0], other))
        } else {
            self.upscale.map(|k| upscale(&self.files[0], self.output.as_deref().unwrap_or_default(), k))
        };
        if let Some(result) = helper {
            return match result {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("pngdeer: {e}");
                    1
                }
            };
        }
        if let Some(dir) = &self.dir {
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!("pngdeer: {}: {}", dir.display(), plain(e));
                return 1;
            }
        }
        let mut failed = false;
        for file in &self.files {
            if let Err(e) = self.compress(file) {
                eprintln!("pngdeer: {}: {e}", if file == "-" { "stdin" } else { file });
                failed = true;
            }
        }
        failed as i32
    }

    /// One file: compressed, written, and reported as asked.
    fn compress(&self, file: &str) -> Result<(), Error> {
        let start = Instant::now();
        let input = if file == "-" {
            let mut bytes = Vec::new();
            io::stdin().read_to_end(&mut bytes).map_err(plain)?;
            bytes
        } else {
            fs::read(file).map_err(plain)?
        };
        let jpeg = is_jpeg(&input);
        if !jpeg && !input.starts_with(PNG_SIGNATURE) {
            return Err("not a PNG or a JPEG".into());
        }
        let own = if jpeg { Format::Jpeg } else { Format::Png };
        let format = self.format.or(named_by(self.output.as_deref())).unwrap_or(own);
        let destination = self.destination(file, format, format == own);
        if let Destination::Stdout = destination {
            if io::stdout().is_terminal() {
                return Err("an image can't go to the terminal: redirect stdout, or use -o".into());
            }
        }
        if let Destination::File(path, true) = &destination {
            if file != "-" && path.exists() && fs::canonicalize(path)? == fs::canonicalize(file)? {
                return Err(format!("{} would replace the input; name another with -o", path.display()).into());
            }
        }
        let (bytes, report, unchanged) = encode(&input, jpeg, format, &self.settings)?;
        match &destination {
            Destination::Stdout => io::stdout().lock().write_all(&bytes).map_err(plain)?,
            Destination::File(path, _) => fs::write(path, &bytes).map_err(|e| format!("{}: {}", path.display(), plain(e)))?,
        }
        if self.verbose {
            let from = if file == "-" { "stdin" } else { file };
            let to = match &destination {
                Destination::Stdout => "stdout".into(),
                Destination::File(path, _) => path.display().to_string(),
            };
            let what = if unchanged {
                format!("{}, unchanged: nothing smaller keeps its quality", size(input.len()))
            } else {
                let change = (bytes.len() as f64 / input.len() as f64 - 1.0) * 100.0;
                let sign = if change < 0.0 { "\u{2212}" } else { "+" };
                format!("{} \u{2192} {} ({sign}{:.0}%)", size(input.len()), size(bytes.len()), change.abs())
            };
            eprintln!("{from} \u{2192} {to}: {what}, {:.1} s", start.elapsed().as_secs_f64());
        }
        if self.json {
            println!(
                r#"{}, "input": {}, "output": {}, "input_bytes": {}}}"#,
                &report[..report.len() - 1],
                json_string(file),
                json_string(&destination.name()),
                input.len()
            );
        }
        Ok(())
    }

    /// Where `file`'s output goes: -o's, else stdout for stdin, else NAME.EXT in -d's folder or
    /// NAME.min.EXT beside it. EXT is the input's own (as spelt) where the format stays.
    fn destination(&self, file: &str, format: Format, same: bool) -> Destination {
        match &self.output {
            Some(output) if output == "-" => return Destination::Stdout,
            Some(output) => return Destination::File(output.into(), false),
            None if file == "-" => return Destination::Stdout,
            None => {}
        }
        let path = Path::new(file);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let extension = match path.extension() {
            Some(own) if same => own.to_string_lossy(),
            _ => format.extension().into(),
        };
        match &self.dir {
            Some(dir) => Destination::File(dir.join(format!("{stem}.{extension}")), true),
            None => Destination::File(path.with_file_name(format!("{stem}.min.{extension}")), true),
        }
    }
}

/// The file in `format`, its JSON report, and whether it is the input unchanged (nothing smaller
/// kept the quality).
fn encode(input: &[u8], jpeg: bool, format: Format, settings: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    match (format, jpeg) {
        (Format::Webp, _) => encode_webp(input, settings),
        (Format::Avif, _) => encode_avif(input, settings),
        (Format::Jpeg, true) => encode_jpeg(input, settings),
        (Format::Png, false) => {
            let result = pngdeer::compress(input, settings)?;
            let json = result.json();
            Ok((result.png, json, result.outcome == "original"))
        }
        (Format::Png, true) => Err("a JPEG becomes a JPEG, WebP or AVIF, not a PNG".into()),
        (Format::Jpeg, false) => Err("a PNG becomes a PNG, WebP or AVIF, not a JPEG".into()),
    }
}

#[cfg(feature = "webp")]
fn encode_webp(input: &[u8], settings: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    let result = pngdeer::compress_webp(input, settings)?;
    let json = result.json();
    Ok((result.webp, json, false))
}

#[cfg(not(feature = "webp"))]
fn encode_webp(_: &[u8], _: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    Err("this pngdeer was built without the webp feature".into())
}

#[cfg(feature = "avif")]
fn encode_avif(input: &[u8], settings: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    let result = pngdeer::compress_avif(input, settings)?;
    let json = result.json();
    Ok((result.avif, json, false))
}

#[cfg(not(feature = "avif"))]
fn encode_avif(_: &[u8], _: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    Err("this pngdeer was built without the avif feature".into())
}

#[cfg(feature = "jpeg")]
fn is_jpeg(input: &[u8]) -> bool {
    pngdeer::jpeg::is_jpeg(input)
}

#[cfg(not(feature = "jpeg"))]
fn is_jpeg(_: &[u8]) -> bool {
    false
}

#[cfg(feature = "jpeg")]
fn encode_jpeg(input: &[u8], settings: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    let result = pngdeer::compress_jpeg(input, settings)?;
    let json = result.json();
    Ok((result.jpeg, json, result.outcome == "original"))
}

#[cfg(not(feature = "jpeg"))]
fn encode_jpeg(_: &[u8], _: &pngdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    Err("this pngdeer was built without the jpeg feature".into())
}

/// An I/O error as people say it, without the OS's error number.
fn plain(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "no such file".into(),
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        io::ErrorKind::IsADirectory => "a folder, not a file".into(),
        _ => e.to_string(),
    }
}

fn number<T: std::str::FromStr>(option: &str, value: String) -> Result<T, String> {
    value.parse().map_err(|_| format!("{option}: not a number: {value}"))
}

/// A size in bytes for people: B, KB or MB, in thousands as file managers count.
fn size(bytes: usize) -> String {
    match bytes {
        0..1000 => format!("{bytes} B"),
        1000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// --score: SSIMULACRA2 of `other` against `file`, printed.
fn score(file: &str, other: &str) -> Result<(), Error> {
    let input = fs::read(file)?;
    if pngdeer::apng::is_animated(&input) {
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
fn upscale(file: &str, output: &str, k: usize) -> Result<(), Error> {
    let image = codec::decode(&fs::read(file)?)?;
    let (w, h, pixels) = (image.width, image.height, &image.pixels);
    let big: Vec<_> = (0..h * k).flat_map(|y| (0..w * k).map(move |x| pixels[(y / k) * w + x / k])).collect();
    fs::write(output, codec::encode_rgba(&big, w * k, h * k))?;
    Ok(())
}

/// SSIMULACRA2 of each frame of animation `a` against the frame animation `b` shows at the same
/// time (b may have merged frames that repeat into longer ones), default images aside.
fn frame_scores(a: &[u8], b: &[u8]) -> Result<Vec<f64>, Error> {
    let (a, b) = (pngdeer::apng::decode(a)?, pngdeer::apng::decode(b)?);
    if (a.width, a.height) != (b.width, b.height) {
        return Err("animations differ in size".into());
    }
    let starts = |timing: &pngdeer::apng::Timing| {
        let seconds = |&(num, den): &(u16, u16)| num as f64 / if den == 0 { 100.0 } else { den as f64 };
        timing.delays.iter().scan(0.0, |t, d| Some(std::mem::replace(t, *t + seconds(d)))).collect::<Vec<f64>>()
    };
    let (sa, sb) = (starts(&a.timing), starts(&b.timing));
    fn frame(anim: &pngdeer::apng::Animation, i: usize) -> &[pngdeer::RGBA] {
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
    if pngdeer::jpeg::is_jpeg(bytes) {
        let photo = pngdeer::jpeg::decode(bytes)?;
        let (pixels, width, height) = pngdeer::jpeg::oriented(photo.pixels, photo.width, photo.height, photo.orientation);
        return Ok(codec::Bitmap { pixels, width, height });
    }
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
