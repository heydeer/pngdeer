use super::*;
use zenpixels::PixelDescriptor;

fn floats(values: &[f32], desc: PixelDescriptor, stride: usize) -> PixelSlice<'_> {
    PixelSlice::new(bytemuck::cast_slice(values), 1, 2, stride, desc).unwrap()
}

#[test]
fn linear_alpha_and_gray_respect_layout_and_stride() {
    let rgba = [
        0.2, 0.4, 0.6, 0.5, 99.0, 99.0, 99.0, 99.0, 0.8, 0.6, 0.4, 0.25,
    ];
    let p = funnel(&floats(&rgba, PixelDescriptor::RGBAF32_LINEAR, 32)).unwrap();
    let PreparedInput::Linear { planes, alpha, .. } = p else {
        panic!("linear expected")
    };
    assert_eq!(planes, [vec![0.2, 0.8], vec![0.4, 0.6], vec![0.6, 0.4]]);
    assert_eq!(alpha.unwrap(), [0.5, 0.25]);
    for premult in [false, true] {
        let gray = if premult {
            [0.125, 0.5, 99.0, 99.0, 0.1875, 0.25]
        } else {
            [0.25, 0.5, 99.0, 99.0, 0.75, 0.25]
        };
        let desc = PixelDescriptor::GRAYAF32_LINEAR.with_alpha(Some(if premult {
            AlphaMode::Premultiplied
        } else {
            AlphaMode::Straight
        }));
        let p = funnel(&floats(&gray, desc, 16)).unwrap();
        let PreparedInput::Linear { planes, alpha, .. } = p else {
            panic!("linear expected")
        };
        assert_eq!(
            planes,
            [vec![0.25, 0.75], vec![0.25, 0.75], vec![0.25, 0.75]]
        );
        assert_eq!(alpha.unwrap(), [0.5, 0.25]);
    }
    let gray = [0.25, 99.0, 0.75];
    let p = funnel(&floats(&gray, PixelDescriptor::GRAYF32_LINEAR, 8)).unwrap();
    let PreparedInput::Linear { planes, .. } = p else {
        panic!("linear expected")
    };
    assert_eq!(planes[0], [0.25, 0.75]);
}

#[cfg(feature = "hdr-pu")]
#[test]
fn hdr_layouts_decode_the_same_colors() {
    use crate::pipeline::pu21::{hlg_triple_to_nits, pq_channel_to_nits};
    for transfer in [
        TransferFunction::Pq,
        TransferFunction::Hlg,
        TransferFunction::Linear,
    ] {
        for bits in [8, 16, 32] {
            if transfer == TransferFunction::Linear && bits != 32 {
                continue;
            }
            for gray in [false, true] {
                for alpha in [false, true] {
                    if gray && alpha && bits == 8 {
                        continue;
                    }
                    let desc = match (bits, gray, alpha) {
                        (8, false, false) => PixelDescriptor::RGB8_SRGB,
                        (8, false, true) => PixelDescriptor::RGBA8_SRGB,
                        (8, true, false) => PixelDescriptor::GRAY8_SRGB,
                        (16, false, false) => PixelDescriptor::RGB16_SRGB,
                        (16, false, true) => PixelDescriptor::RGBA16_SRGB,
                        (16, true, false) => PixelDescriptor::GRAY16_SRGB,
                        (16, true, true) => PixelDescriptor::GRAYA16_SRGB,
                        (32, false, false) => PixelDescriptor::RGBF32_LINEAR,
                        (32, false, true) => PixelDescriptor::RGBAF32_LINEAR,
                        (32, true, false) => PixelDescriptor::GRAYF32_LINEAR,
                        (32, true, true) => PixelDescriptor::GRAYAF32_LINEAR,
                        _ => unreachable!(),
                    }
                    .with_transfer(transfer);
                    let values: &[u8] = if gray {
                        &[51, 128]
                    } else {
                        &[51, 102, 153, 128]
                    };
                    let count = (if gray { 1 } else { 3 }) + usize::from(alpha);
                    let mut pixel = Vec::new();
                    for &v in &values[..count] {
                        match bits {
                            8 => pixel.push(v),
                            16 => pixel.extend_from_slice(&(v as u16 * 257).to_ne_bytes()),
                            _ => pixel.extend_from_slice(&(v as f32 * (1.0 / 255.0)).to_ne_bytes()),
                        }
                    }
                    let stride = pixel.len() * 2;
                    let mut bytes = pixel.clone();
                    bytes.resize(stride, 0xff);
                    bytes.extend_from_slice(&pixel);
                    let slice = PixelSlice::new(&bytes, 1, 2, stride, desc).unwrap();
                    let PreparedInput::Linear {
                        planes, alpha: a, ..
                    } = funnel_nits(&slice).unwrap()
                    else {
                        panic!("linear expected")
                    };
                    let rgb = if gray { [0.2; 3] } else { [0.2, 0.4, 0.6] };
                    let expected = match transfer {
                        TransferFunction::Pq => rgb.map(pq_channel_to_nits),
                        TransferFunction::Hlg => hlg_triple_to_nits(rgb),
                        _ => rgb,
                    };
                    for c in 0..3 {
                        for &actual in &planes[c] {
                            assert!(
                                (actual - expected[c]).abs() <= 0.001 * expected[c].max(1.0),
                                "{desc:?}: {actual} != {}",
                                expected[c]
                            );
                        }
                    }
                    assert_eq!(a.is_some(), alpha);
                    if bits == 8 && !gray && alpha {
                        let mut bgr = bytes.clone();
                        for y in 0..2 {
                            bgr.swap(y * stride, y * stride + 2);
                        }
                        let desc = PixelDescriptor::BGRA8_SRGB.with_transfer(transfer);
                        let slice = PixelSlice::new(&bgr, 1, 2, stride, desc).unwrap();
                        let PreparedInput::Linear {
                            planes: bgr_planes,
                            alpha: bgr_alpha,
                            ..
                        } = funnel_nits(&slice).unwrap()
                        else {
                            panic!("linear expected")
                        };
                        assert_eq!(planes, bgr_planes);
                        assert_eq!(a, bgr_alpha);
                    }
                }
            }
        }
    }
}

#[test]
fn dimensions_validate_before_allocation() {
    assert_eq!(checked_dimensions(1, 1).unwrap(), 1);
    assert_eq!(
        checked_dimensions(16384, 16384).unwrap(),
        crate::MAX_IMAGE_PIXELS
    );
    for (w, h) in [(0, 8), (8, 0)] {
        assert_eq!(
            checked_dimensions(w, h),
            Err(Ssimulacra2Error::InvalidImageSize)
        );
    }
    // Original fits, but its padded shape does not.
    assert!(matches!(
        checked_dimensions(crate::MAX_IMAGE_PIXELS, 1),
        Err(Ssimulacra2Error::ImageTooLarge { .. })
    ));
    assert!(matches!(
        checked_dimensions(usize::MAX, 2),
        Err(Ssimulacra2Error::ImageTooLarge { .. })
    ));
    assert!(matches!(
        checked_dimensions(16385, 16384),
        Err(Ssimulacra2Error::ImageTooLarge { .. })
    ));
}
