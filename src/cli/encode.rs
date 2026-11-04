//! The output formats, and the library's encoder for each.

use std::path::Path;

use super::Error;

pub const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Avif,
}

impl Format {
    pub fn named(name: &str) -> Option<Format> {
        match name.to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpeg" | "jpg" => Some(Format::Jpeg),
            "webp" => Some(Format::Webp),
            "avif" => Some(Format::Avif),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Avif => "avif",
        }
    }
}

/// The format an output's name says, by its extension.
pub fn named_by(output: Option<&str>) -> Option<Format> {
    Format::named(&Path::new(output?).extension()?.to_string_lossy())
}

/// The file in `format`, its JSON report, and whether it is the input unchanged (nothing smaller
/// kept the quality).
pub fn encode(input: &[u8], jpeg: bool, format: Format, settings: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    match (format, jpeg) {
        (Format::Webp, _) => encode_webp(input, settings),
        (Format::Avif, _) => encode_avif(input, settings),
        (Format::Jpeg, true) => encode_jpeg(input, settings),
        (Format::Png, false) => {
            let result = slimdeer::compress(input, settings)?;
            let json = result.json();
            Ok((result.png, json, result.outcome == "original"))
        }
        (Format::Png, true) => Err("a JPEG becomes a JPEG, WebP or AVIF, not a PNG".into()),
        (Format::Jpeg, false) => Err("a PNG becomes a PNG, WebP or AVIF, not a JPEG".into()),
    }
}

#[cfg(feature = "webp")]
fn encode_webp(input: &[u8], settings: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    let result = slimdeer::compress_webp(input, settings)?;
    let json = result.json();
    Ok((result.webp, json, false))
}

#[cfg(not(feature = "webp"))]
fn encode_webp(_: &[u8], _: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    Err("this slimdeer was built without the webp feature".into())
}

#[cfg(feature = "avif")]
fn encode_avif(input: &[u8], settings: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    let result = slimdeer::compress_avif(input, settings)?;
    let json = result.json();
    Ok((result.avif, json, false))
}

#[cfg(not(feature = "avif"))]
fn encode_avif(_: &[u8], _: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    Err("this slimdeer was built without the avif feature".into())
}

#[cfg(feature = "jpeg")]
pub fn is_jpeg(input: &[u8]) -> bool {
    slimdeer::jpeg::is_jpeg(input)
}

#[cfg(not(feature = "jpeg"))]
pub fn is_jpeg(_: &[u8]) -> bool {
    false
}

#[cfg(feature = "jpeg")]
fn encode_jpeg(input: &[u8], settings: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    let result = slimdeer::compress_jpeg(input, settings)?;
    let json = result.json();
    Ok((result.jpeg, json, result.outcome == "original"))
}

#[cfg(not(feature = "jpeg"))]
fn encode_jpeg(_: &[u8], _: &slimdeer::Settings) -> Result<(Vec<u8>, String, bool), Error> {
    Err("this slimdeer was built without the jpeg feature".into())
}
