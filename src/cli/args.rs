//! The options, and what's wrong with a command line before any file is read.

use std::path::PathBuf;

use slimdeer::search;

use super::encode::{named_by, Format};

const HELP: &str = "slimdeer: smaller PNG and JPEG files, or WebP and AVIF, that look the same

Usage: slimdeer [OPTIONS] FILE...

Writes each FILE, a PNG or a JPEG, smaller, at a quality picked for it, next to it
as NAME.min.EXT. Says nothing unless something goes wrong.

Examples:
  slimdeer photo.jpg                 writes photo.min.jpg
  slimdeer *.png -d dist             writes dist/NAME.png for each
  slimdeer -r assets -d dist         writes every image under assets to dist, in the same folders
  slimdeer logo.png --format webp    writes logo.min.webp
  slimdeer in.png -o out.avif        writes out.avif, the format as its name says
  slimdeer shot.png --target 90      keeps a score of at least 90
  slimdeer - < in.png > out.png      from stdin to stdout

Options:
  -o, --output FILE    Write the one FILE's output here (- for stdout)
  -d, --dir DIR        Write the outputs into DIR, under the inputs' names
  -r, --recursive      Take a FILE that is a folder as the PNGs and JPEGs in it and
                       below it; -d's folder gets the same layout
      --format FORMAT  png, jpeg, webp or avif [default: the input's, or -o's]
                       (a PNG can't become a JPEG, nor a JPEG a PNG)
      --target T       The SSIMULACRA2 score to keep, 0 to 100: 90 looks lossless,
                       70 is high quality [default: auto, about TinyPNG's, per image]
  -v, --verbose        Say what each file came to, on stderr
      --json           Print each file's report, a JSON line, on stdout
  -h, --help           Print this help
  -V, --version        Print the version

Also:
  slimdeer ORIGINAL --score OTHER    Print OTHER's SSIMULACRA2 score against ORIGINAL";

pub struct Command {
    pub(super) files: Vec<String>,
    pub(super) output: Option<String>,
    pub(super) dir: Option<PathBuf>,
    pub(super) recursive: bool,
    pub(super) format: Option<Format>,
    pub(super) verbose: bool,
    pub(super) json: bool,
    pub(super) score_against: Option<String>,
    pub(super) upscale: Option<usize>,
    pub(super) settings: slimdeer::Settings,
}

/// The command line, or None where it asked for the help or the version (printed), or what's wrong
/// with it.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Option<Command>, String> {
    let mut command = Command {
        files: Vec::new(),
        output: None,
        dir: None,
        recursive: false,
        format: None,
        verbose: false,
        json: false,
        score_against: None,
        upscale: None,
        settings: slimdeer::Settings { clock: Some(super::now_ms), ..Default::default() },
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
            "-r" | "--recursive" => command.recursive = true,
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
                println!("slimdeer {}", env!("CARGO_PKG_VERSION"));
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
    if command.output.is_some() && (files.len() > 1 || command.recursive) {
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

fn number<T: std::str::FromStr>(option: &str, value: String) -> Result<T, String> {
    value.parse().map_err(|_| format!("{option}: not a number: {value}"))
}
