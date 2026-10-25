//! Internal sRGB-encoded-f32 → linear conversion, delegated to
//! `linear-srgb` (the family-standard implementation). Quantized inputs
//! never reach these fns — they take the captured LUT path.

/// sRGB-encoded f32 `[0,1]` → linear light.
#[inline]
pub(crate) fn srgb_to_linear(s: f32) -> f32 {
    linear_srgb::default::srgb_to_linear(s)
}
