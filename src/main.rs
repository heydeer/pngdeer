//! slimdeer on the command line, writing the same files as the browser build (web/):
//!
//!   slimdeer [OPTIONS] FILE...     (HELP below)
//!
//! Each FILE, a PNG or a JPEG, comes out smaller at a quality picked for it, next to it as
//! NAME.min.EXT, in a folder with -d, or where -o says. With -r a FILE that is a folder stands for
//! the images in it and below it, and -d's folder gets a copy of its layout. Like cp and pngquant, it says nothing unless
//! something goes wrong: -v says what each file came to (on stderr), --json prints each file's
//! report (on stdout, one a line). A file that fails doesn't stop the others; slimdeer then exits 1,
//! and 2 on a command it can't run. A name it makes up never replaces an input; -o can.
//!
//! A PNG comes out as the smallest palette PNG that keeps its quality as SSIMULACRA2 measures it: a
//! score chosen per image (auto, the default; see search.rs), or at least --target's. Its report
//! gives the target, palette size and variant, score, evaluations, time per stage, outcome and bytes.
//! An animated PNG stays animated, every frame at least the target on one palette (the report adds
//! "frames", and its score is the worst frame's).
//!
//! A JPEG comes out as a JPEG: re-encoded at the lowest quality that keeps the target, never bigger
//! than the input (slimdeer::jpeg). Automatic quality there is about TinyPNG's JPEG's, which goes by
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
//!   (slimdeer::WebpOptions).
//! --preview-pixels N: images over N pixels are searched on a downscaled preview, then checked at
//!   full size (default slimdeer::PREVIEW_PIXELS, as in the browser). 0 searches at full size: about
//!   1% smaller files, several times slower on large images.
//! --variants SPEC: the remapping variants tried at each palette size, e.g. 50,0,0r2,w0 (the
//!   default; see search::parse_variants).
//!
//! Two helpers for bench/bench.py:
//!   slimdeer IN.png --score OTHER           SSIMULACRA2 of OTHER (PNG, WebP or JPEG) against IN, as
//!                                           the search scores; for an animated IN, the worst frame's,
//!                                           each against the frame OTHER shows at that time (in the
//!                                           help too)
//!   slimdeer IN.png -o OUT.png --upscale K  nearest-neighbour K× enlargement, written as RGBA
//!
//! The parts are in cli/: the options (args.rs), which files and where their outputs go (files.rs),
//! running a command (run.rs), the encoders (encode.rs), what it says (report.rs) and the helpers
//! (tools.rs).

use std::{env, process};

mod cli;

fn main() {
    let code = match cli::args::parse(env::args().skip(1)) {
        Ok(Some(command)) => command.run(),
        Ok(None) => 0,
        Err(message) => cli::usage_error(&message),
    };
    process::exit(code);
}
