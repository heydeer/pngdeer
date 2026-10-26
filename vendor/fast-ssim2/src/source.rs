//! Input funnel: a [`zenpixels::PixelSlice`] (borrowed, self-describing,
//! strided) becomes either an [`EncodedSrgb`] (quantized/encoded inputs —
//! u8 takes the reference-captured LUT for bit-exact parity; u16/f32
//! evaluate the sRGB polynomial) or deinterleaved linear `f32` planes
//! (already-linear inputs).
#![allow(clippy::chunks_exact_to_as_chunks)]

use std::borrow::Cow;

use zenpixels::{AlphaMode, PixelFormat, PixelSlice, SignalRange, TransferFunction};

use crate::Ssimulacra2Error;
use crate::pipeline::{EncodedData, EncodedSrgb};

/// Result of [`funnel`] — the encoded path preserves the original bytes
/// (LUT-exact); the linear path holds premultiplied-ready planes.
pub(crate) enum PreparedInput {
    /// Encoded sRGB quantized/f32-on-grid input → reference LUT path.
    Encoded(EncodedSrgb),
    /// Already-linear input — planes + optional straight-alpha plane.
    Linear {
        planes: [Vec<f32>; 3],
        alpha: Option<Vec<f32>>,
        width: usize,
        height: usize,
    },
}

impl PreparedInput {
    /// (width, height) of the source, pre-padding.
    pub(crate) fn dims(&self) -> (usize, usize) {
        match self {
            Self::Encoded(e) => (e.width, e.height),
            Self::Linear { width, height, .. } => (*width, *height),
        }
    }
}

/// Validate original and padded dimensions before allocating input planes.
pub(crate) fn checked_dimensions(w: usize, h: usize) -> Result<usize, Ssimulacra2Error> {
    if w == 0 || h == 0 {
        return Err(Ssimulacra2Error::InvalidImageSize);
    }
    let npix = w
        .checked_mul(h)
        .ok_or(Ssimulacra2Error::ImageTooLarge { actual: usize::MAX })?;
    if npix > crate::MAX_IMAGE_PIXELS {
        return Err(Ssimulacra2Error::ImageTooLarge { actual: npix });
    }
    let padded_pixels = w
        .max(8)
        .checked_mul(h.max(8))
        .ok_or(Ssimulacra2Error::ImageTooLarge { actual: usize::MAX })?;
    if padded_pixels > crate::MAX_IMAGE_PIXELS {
        return Err(Ssimulacra2Error::ImageTooLarge {
            actual: padded_pixels,
        });
    }
    Ok(npix)
}

/// Interpret `slice` per its [`PixelDescriptor`], materializing the
/// planes the pipeline needs. See the crate docs for which descriptors
/// are rejected (HDR transfers, narrow range, non-BT.709 primaries).
pub(crate) fn funnel(slice: &PixelSlice<'_>) -> Result<PreparedInput, Ssimulacra2Error> {
    let desc = slice.descriptor();
    let (w, h) = (slice.width() as usize, slice.rows() as usize);
    let npix = checked_dimensions(w, h)?;

    // Descriptor gates — reject what the SDR metric can't honestly score.
    match desc.transfer {
        TransferFunction::Srgb => {}
        TransferFunction::Linear => {
            if !matches!(
                desc.format,
                PixelFormat::RgbaF32
                    | PixelFormat::RgbF32
                    | PixelFormat::GrayF32
                    | PixelFormat::GrayAF32
            ) {
                return Err(Ssimulacra2Error::UnsupportedInput(
                    "linear transfer requires an f32 format",
                ));
            }
        }
        TransferFunction::Pq | TransferFunction::Hlg => {
            return Err(Ssimulacra2Error::UnsupportedInput(
                "HDR transfers (PQ, HLG) — convert via zenpixels-convert first",
            ));
        }
        TransferFunction::Bt709 | TransferFunction::Gamma22 => {
            return Err(Ssimulacra2Error::UnsupportedInput(
                "non-sRGB encoded transfer — the LUT is sRGB-shaped; convert upstream",
            ));
        }
        _ => {
            return Err(Ssimulacra2Error::UnsupportedInput(
                "unknown transfer — state it explicitly (e.g. RGB8_SRGB) so                  the bytes aren't silently misread",
            ));
        }
    }
    if desc.primaries != zenpixels::ColorPrimaries::Bt709 {
        return Err(Ssimulacra2Error::UnsupportedInput(
            "non-BT.709 primaries — gamut-convert via zenpixels-convert first",
        ));
    }
    if desc.signal_range != SignalRange::Full {
        return Err(Ssimulacra2Error::UnsupportedInput(
            "narrow/limited signal range — expand to full range upstream",
        ));
    }

    // Premultiplied alpha is un-multiplied into an owned buffer; every
    // other layout is consumed borrowed (zero-copy).
    let data: Cow<'_, [u8]> = if matches!(desc.alpha, Some(AlphaMode::Premultiplied)) {
        Cow::Owned(unpremultiply(slice))
    } else {
        Cow::Borrowed(slice.as_strided_bytes())
    };
    let stride = slice.stride();
    let premult_alpha = matches!(
        desc.alpha,
        Some(AlphaMode::Straight) | Some(AlphaMode::Premultiplied)
    );

    let f32_alpha_of = |data: &[u8], stride: usize, bpp: usize| -> Vec<f32> {
        let mut a = Vec::with_capacity(npix);
        for y in 0..h {
            let row = &data[y * stride..];
            for px in row[..w * bpp].chunks_exact(bpp) {
                a.push(f32::from_ne_bytes([px[12], px[13], px[14], px[15]]));
            }
        }
        a
    };

    Ok(match (desc.format, desc.transfer) {
        // ---------------------------------------------------- encoded u8
        (PixelFormat::Rgb8, _) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            for y in 0..h {
                let row = &data[y * stride..];
                rgb.extend_from_slice(&row[..w * 3]);
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U8(rgb),
                alpha: None,
            })
        }
        (PixelFormat::Rgba8, _) | (PixelFormat::Rgbx8, _) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            let mut alpha = premult_alpha.then(|| Vec::with_capacity(npix));
            for y in 0..h {
                let row = &data[y * stride..];
                for px in row[..w * 4].chunks_exact(4) {
                    rgb.extend_from_slice(&px[..3]);
                    if let Some(a) = alpha.as_mut() {
                        a.push(px[3] as f32 * (1.0 / 255.0));
                    }
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U8(rgb),
                alpha,
            })
        }
        (PixelFormat::Bgra8, _) | (PixelFormat::Bgrx8, _) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            let mut alpha = premult_alpha.then(|| Vec::with_capacity(npix));
            for y in 0..h {
                let row = &data[y * stride..];
                for px in row[..w * 4].chunks_exact(4) {
                    rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                    if let Some(a) = alpha.as_mut() {
                        a.push(px[3] as f32 * (1.0 / 255.0));
                    }
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U8(rgb),
                alpha,
            })
        }
        (PixelFormat::Gray8, _) => {
            let mut g = Vec::with_capacity(npix);
            for y in 0..h {
                let row = &data[y * stride..];
                g.extend_from_slice(&row[..w]);
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U8(g.into_iter().flat_map(|v| [v; 3]).collect()),
                alpha: None,
            })
        }
        // --------------------------------------------------- encoded u16
        (PixelFormat::Rgb16, _) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            for y in 0..h {
                let row = &data[y * stride..];
                for c in row[..w * 6].chunks_exact(2) {
                    rgb.push(u16::from_ne_bytes([c[0], c[1]]));
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U16(rgb),
                alpha: None,
            })
        }
        (PixelFormat::Rgba16, _) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            let mut alpha = premult_alpha.then(|| Vec::with_capacity(npix));
            for y in 0..h {
                let row = &data[y * stride..];
                for px in row[..w * 8].chunks_exact(8) {
                    for c in 0..3 {
                        rgb.push(u16::from_ne_bytes([px[2 * c], px[2 * c + 1]]));
                    }
                    if let Some(a) = alpha.as_mut() {
                        a.push(u16::from_ne_bytes([px[6], px[7]]) as f32 * (1.0 / 65535.0));
                    }
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U16(rgb),
                alpha,
            })
        }
        (PixelFormat::Gray16, _) => {
            let mut g = Vec::with_capacity(npix);
            for y in 0..h {
                let row = &data[y * stride..];
                for c in row[..w * 2].chunks_exact(2) {
                    g.push(u16::from_ne_bytes([c[0], c[1]]));
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::U16(g.into_iter().flat_map(|v| [v; 3]).collect()),
                alpha: None,
            })
        }
        // -------------------------------------- encoded f32
        // sRGB-encoded f32 is the *general* path — a rational polynomial
        // (`linear-srgb`), no grid tricks: encoded-f32 is a marginal input
        // class (decoders emit integers; linear pipelines emit Linear-f32),
        // and callers with u8-quantized data get bit-exact results by
        // handing us u8 directly.
        (PixelFormat::RgbF32, TransferFunction::Srgb) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            for y in 0..h {
                let row = &data[y * stride..];
                for c in row[..w * 12].chunks_exact(4) {
                    rgb.push(f32::from_ne_bytes([c[0], c[1], c[2], c[3]]));
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::F32(rgb),
                alpha: None,
            })
        }
        (PixelFormat::RgbaF32, TransferFunction::Srgb) => {
            let mut rgb = Vec::with_capacity(npix * 3);
            let mut alpha = premult_alpha.then(|| Vec::with_capacity(npix));
            for y in 0..h {
                let row = &data[y * stride..];
                for px in row[..w * 16].chunks_exact(16) {
                    for c in 0..3 {
                        rgb.push(f32::from_ne_bytes([
                            px[4 * c],
                            px[4 * c + 1],
                            px[4 * c + 2],
                            px[4 * c + 3],
                        ]));
                    }
                    if let Some(a) = alpha.as_mut() {
                        a.push(f32::from_ne_bytes([px[12], px[13], px[14], px[15]]));
                    }
                }
            }
            PreparedInput::Encoded(EncodedSrgb {
                width: w,
                height: h,
                data: EncodedData::F32(rgb),
                alpha,
            })
        }
        // ------------------------------------------------- linear f32
        (PixelFormat::RgbF32, TransferFunction::Linear) => {
            let mut planes = [
                Vec::with_capacity(npix),
                Vec::with_capacity(npix),
                Vec::with_capacity(npix),
            ];
            for y in 0..h {
                let row = &data[y * stride..];
                for px in row[..w * 12].chunks_exact(12) {
                    for (c, ch) in px.chunks_exact(4).enumerate() {
                        planes[c].push(f32::from_ne_bytes([ch[0], ch[1], ch[2], ch[3]]));
                    }
                }
            }
            PreparedInput::Linear {
                planes,
                alpha: None,
                width: w,
                height: h,
            }
        }
        (PixelFormat::RgbaF32, TransferFunction::Linear) => PreparedInput::Linear {
            planes: {
                let mut planes = [
                    Vec::with_capacity(npix),
                    Vec::with_capacity(npix),
                    Vec::with_capacity(npix),
                ];
                for y in 0..h {
                    let row = &data[y * stride..];
                    for px in row[..w * 16].chunks_exact(16) {
                        for c in 0..3 {
                            planes[c].push(f32::from_ne_bytes([
                                px[4 * c],
                                px[4 * c + 1],
                                px[4 * c + 2],
                                px[4 * c + 3],
                            ]));
                        }
                    }
                }
                planes
            },
            alpha: premult_alpha.then(|| f32_alpha_of(&data, stride, 16)),
            width: w,
            height: h,
        },
        (PixelFormat::GrayF32, TransferFunction::Linear) => {
            let mut g = Vec::with_capacity(npix);
            for y in 0..h {
                let row = &data[y * stride..];
                for c in row[..w * 4].chunks_exact(4) {
                    g.push(f32::from_ne_bytes([c[0], c[1], c[2], c[3]]));
                }
            }
            PreparedInput::Linear {
                planes: [g.clone(), g.clone(), g],
                alpha: None,
                width: w,
                height: h,
            }
        }
        (PixelFormat::GrayAF32, TransferFunction::Linear) => {
            let mut g = Vec::with_capacity(npix);
            let mut a = Vec::with_capacity(npix);
            for y in 0..h {
                let row = &data[y * stride..];
                for px in row[..w * 8].chunks_exact(8) {
                    g.push(f32::from_ne_bytes([px[0], px[1], px[2], px[3]]));
                    a.push(f32::from_ne_bytes([px[4], px[5], px[6], px[7]]));
                }
            }
            PreparedInput::Linear {
                planes: [g.clone(), g.clone(), g],
                alpha: premult_alpha.then_some(a),
                width: w,
                height: h,
            }
        }
        _ => {
            return Err(Ssimulacra2Error::UnsupportedInput(
                "unsupported pixel format for SSIMULACRA2 — RGB/RGBA/BGRA/Gray \
                 in u8/u16/f32-sRGB or f32-linear only",
            ));
        }
    })
}

/// Un-premultiply to straight alpha, native domain (ported from zensim's
/// zenpixels adapter — quantizes back for integer formats).
fn unpremultiply(slice: &PixelSlice<'_>) -> Vec<u8> {
    let (w, h) = (slice.width() as usize, slice.rows() as usize);
    let stride = slice.stride();
    let mut out = slice.as_strided_bytes().to_vec();
    match slice.descriptor().format {
        PixelFormat::Rgba8 | PixelFormat::Bgra8 | PixelFormat::Rgbx8 | PixelFormat::Bgrx8 => {
            for y in 0..h {
                let row_start = y * stride;
                for x in 0..w {
                    let off = row_start + x * 4;
                    let a = out[off + 3];
                    if a == 0 {
                        out[off] = 0;
                        out[off + 1] = 0;
                        out[off + 2] = 0;
                    } else if a < 255 {
                        let inv = 255.0 / a as f32;
                        for c in 0..3 {
                            out[off + c] = (out[off + c] as f32 * inv).round().min(255.0) as u8;
                        }
                    }
                }
            }
        }
        PixelFormat::Rgba16 | PixelFormat::GrayA16 => {
            let bpp = slice.descriptor().format.bytes_per_pixel();
            let alpha_offset = bpp - 2;
            for y in 0..h {
                let row_start = y * stride;
                for x in 0..w {
                    let off = row_start + x * bpp;
                    let a =
                        u16::from_ne_bytes([out[off + alpha_offset], out[off + alpha_offset + 1]]);
                    if a == 0 {
                        out[off..off + alpha_offset].fill(0);
                    } else if a < 65535 {
                        let inv = 65535.0 / a as f32;
                        for c in 0..alpha_offset / 2 {
                            let co = off + c * 2;
                            let v = u16::from_ne_bytes([out[co], out[co + 1]]);
                            let unpremul = (v as f32 * inv).round().min(65535.0) as u16;
                            out[co..co + 2].copy_from_slice(&unpremul.to_ne_bytes());
                        }
                    }
                }
            }
        }
        PixelFormat::RgbaF32 | PixelFormat::GrayAF32 => {
            let bpp = slice.descriptor().format.bytes_per_pixel();
            let alpha_offset = bpp - 4;
            for y in 0..h {
                let row_start = y * stride;
                for x in 0..w {
                    let off = row_start + x * bpp;
                    let a = f32::from_ne_bytes([
                        out[off + alpha_offset],
                        out[off + alpha_offset + 1],
                        out[off + alpha_offset + 2],
                        out[off + alpha_offset + 3],
                    ]);
                    if a <= 0.0 {
                        out[off..off + alpha_offset].fill(0);
                    } else if a < 1.0 {
                        let inv = 1.0 / a;
                        for c in 0..alpha_offset / 4 {
                            let co = off + c * 4;
                            let v = f32::from_ne_bytes([
                                out[co],
                                out[co + 1],
                                out[co + 2],
                                out[co + 3],
                            ]);
                            out[co..co + 4].copy_from_slice(&(v * inv).to_ne_bytes());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

// ============================================================================
// HDR/PU21 funnel (`hdr-pu` feature)
// ============================================================================

/// [`funnel`] variant for the PU21 path: produces linear planes of
/// **absolute luminance in cd/m²**.
///
/// Accepted descriptors:
/// - `Linear` f32 — treated as absolute nits directly.
/// - `Pq`/`Hlg` (u8/u16/f32) — EOTF-decoded to nits. BT.2020 primaries
///   are *not* gamut-converted (the opsin matrix consumes whatever
///   primaries the source declares — same convention as zensim's PU path).
#[cfg(feature = "hdr-pu")]
pub(crate) fn funnel_nits(slice: &PixelSlice<'_>) -> Result<PreparedInput, Ssimulacra2Error> {
    use crate::pipeline::pu21;

    let desc = slice.descriptor();
    let (w, h) = (slice.width() as usize, slice.rows() as usize);
    let npix = checked_dimensions(w, h)?;
    if desc.signal_range != SignalRange::Full {
        return Err(Ssimulacra2Error::UnsupportedInput(
            "narrow/limited signal range — expand to full range upstream",
        ));
    }

    let pq = matches!(desc.transfer, TransferFunction::Pq);
    let hlg = matches!(desc.transfer, TransferFunction::Hlg);
    let linear = matches!(desc.transfer, TransferFunction::Linear);
    if !(pq || hlg || linear) {
        return Err(Ssimulacra2Error::UnsupportedInput(
            "PU path needs Linear f32 (nits) or Pq/Hlg descriptors",
        ));
    }
    if linear
        && !matches!(
            desc.format,
            PixelFormat::RgbF32
                | PixelFormat::RgbaF32
                | PixelFormat::GrayF32
                | PixelFormat::GrayAF32
        )
    {
        return Err(Ssimulacra2Error::UnsupportedInput(
            "linear transfer requires an f32 format",
        ));
    }

    // Premultiplied alpha un-multiplied in code-value space first.
    let data: Cow<'_, [u8]> = if matches!(desc.alpha, Some(AlphaMode::Premultiplied)) {
        Cow::Owned(unpremultiply(slice))
    } else {
        Cow::Borrowed(slice.as_strided_bytes())
    };
    let stride = slice.stride();
    let alpha_present = matches!(
        desc.alpha,
        Some(AlphaMode::Straight) | Some(AlphaMode::Premultiplied)
    );

    // Decode one channel's code value → [0,1], then to nits. For HLG the
    // per-pixel OOTF needs the full triple — handled in the px loop below.
    let decode01 = |v01: f32| -> f32 {
        if pq {
            pu21::pq_channel_to_nits(v01)
        } else {
            v01 // Linear: already nits; HLG handled per-triple
        }
    };

    let rgb_planes = |data: &[u8],
                      bpp: usize,
                      channel_bytes: usize,
                      read: &dyn Fn(&[u8]) -> f32,
                      gray: bool,
                      has_alpha: bool,
                      alpha_off: usize,
                      bgr: bool| {
        let mut planes = [
            Vec::with_capacity(npix),
            Vec::with_capacity(npix),
            Vec::with_capacity(npix),
        ];
        let mut alpha = has_alpha.then(|| Vec::with_capacity(npix));
        for y in 0..h {
            let row = &data[y * stride..];
            for px in row[..w * bpp].chunks_exact(bpp) {
                if hlg {
                    let triple = if gray {
                        [read(px); 3]
                    } else {
                        core::array::from_fn(|c| {
                            let src = if bgr { 2 - c } else { c };
                            read(&px[src * channel_bytes..])
                        })
                    };
                    let n = pu21::hlg_triple_to_nits(triple);
                    for c in 0..3 {
                        planes[c].push(if gray { n[0] } else { n[c] });
                    }
                } else if gray {
                    let g = decode01(read(px));
                    planes[0].push(g);
                    planes[1].push(g);
                    planes[2].push(g);
                } else {
                    let cbytes = channel_bytes;
                    for (c, plane) in planes.iter_mut().enumerate() {
                        let src = if bgr { 2 - c } else { c };
                        plane.push(decode01(read(&px[src * cbytes..])));
                    }
                }
                if let Some(a) = alpha.as_mut() {
                    a.push(read(&px[alpha_off..]));
                }
            }
        }
        (planes, alpha)
    };

    let (planes, alpha) = match desc.format {
        PixelFormat::Rgb8
        | PixelFormat::Rgba8
        | PixelFormat::Rgbx8
        | PixelFormat::Bgra8
        | PixelFormat::Bgrx8
        | PixelFormat::Gray8 => {
            let bpp = desc.format.bytes_per_pixel();
            let (a_off, gray) = match desc.format {
                PixelFormat::Gray8 => (0, true),
                _ => (3, false),
            };
            let rd = |p: &[u8]| p[0] as f32 * (1.0 / 255.0);
            let bgr = matches!(desc.format, PixelFormat::Bgra8 | PixelFormat::Bgrx8);
            rgb_planes(&data, bpp, 1, &rd, gray, alpha_present, a_off, bgr)
        }
        PixelFormat::Rgb16 | PixelFormat::Rgba16 | PixelFormat::Gray16 | PixelFormat::GrayA16 => {
            let bpp = desc.format.bytes_per_pixel();
            let gray = matches!(desc.format, PixelFormat::Gray16 | PixelFormat::GrayA16);
            let rd = |p: &[u8]| u16::from_ne_bytes([p[0], p[1]]) as f32 * (1.0 / 65535.0);
            rgb_planes(&data, bpp, 2, &rd, gray, alpha_present, bpp - 2, false)
        }
        PixelFormat::RgbF32
        | PixelFormat::RgbaF32
        | PixelFormat::GrayF32
        | PixelFormat::GrayAF32 => {
            let bpp = desc.format.bytes_per_pixel();
            let gray = matches!(desc.format, PixelFormat::GrayF32 | PixelFormat::GrayAF32);
            let rd = |p: &[u8]| f32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
            rgb_planes(&data, bpp, 4, &rd, gray, alpha_present, bpp - 4, false)
        }
        _ => {
            return Err(Ssimulacra2Error::UnsupportedInput(
                "PU path supports RGB/RGBA/Gray/GrayA layouts",
            ));
        }
    };

    Ok(PreparedInput::Linear {
        planes,
        alpha,
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests;
