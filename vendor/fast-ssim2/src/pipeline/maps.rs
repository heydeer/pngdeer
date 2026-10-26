#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]

//! Bit-exact ports of `SSIMMap` and `EdgeDiffMap` from `ssimulacra2.cc`.
//!
//! The reference computes per-pixel quotients in f32 but the final
//! `1 - q` subtraction, clamping, norms and accumulations in f64 —
//! reproducing that split is required for bit-identical scores.

#[allow(unused_imports)]
use super::Fma; // pngdeer patch
use enough::Stop;

pub(crate) const K_C2: f32 = 0.0009;

#[inline]
pub(crate) fn tothe4th(mut x: f64) -> f64 {
    x *= x;
    x *= x;
    x
}

/// `SSIMMap` → 6 values: per channel {L1, L4} averages.
/// m1/m2 = blurred images (mu), s11/s22 = blurred squares, s12 = blurred product.
pub fn ssim_map(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> [f64; 6] {
    ssim_map_opts(m1, m2, s11, s22, s12, width, height, false)
}

/// `SSIMMap` with a `sigma_f64` toggle: `true` computes the σ/μ products
/// and quotients in f64 (kills the reference's f32 cancellation noise on
/// flat inputs); `false` is the bit-exact official path.
pub fn ssim_map_opts(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    sigma_f64: bool,
) -> [f64; 6] {
    let one_per_pixels = 1.0 / (height * width) as f64;
    let mut out = [0f64; 6];
    for c in 0..3 {
        let mut sum0 = 0f64;
        let mut sum1 = 0f64;
        for i in 0..width * height {
            let d = if sigma_f64 {
                let mu1 = m1[c][i] as f64;
                let mu2 = m2[c][i] as f64;
                let mu11 = mu1 * mu1;
                let mu22 = mu2 * mu2;
                let mu12 = mu1 * mu2;
                let num_m = 1.0f64 - (mu1 - mu2) * (mu1 - mu2);
                let num_s = 2.0f64 * (s12[c][i] as f64 - mu12) + K_C2 as f64;
                let denom_s = (s11[c][i] as f64 - mu11) + (s22[c][i] as f64 - mu22) + K_C2 as f64;
                (1.0f64 - num_m * num_s / denom_s).max(0.0)
            } else {
                let mu1 = m1[c][i];
                let mu2 = m2[c][i];
                let mu11 = mu1 * mu1;
                let mu22 = mu2 * mu2;
                let mu12 = mu1 * mu2;
                // f32 throughout, then the f32 quotient is subtracted in f64.
                let dm = mu1 - mu2;
                let num_m = (-dm).fma(dm, 1.0f32);
                let num_s = 2.0f32 * (s12[c][i] - mu12) + K_C2;
                let denom_s = (s11[c][i] - mu11) + (s22[c][i] - mu22) + K_C2;
                let q = num_m * num_s / denom_s; // f32
                (1.0f64 - q as f64).max(0.0)
            };
            sum0 += d;
            sum1 += tothe4th(d);
        }
        out[c * 2] = one_per_pixels * sum0;
        out[c * 2 + 1] = (one_per_pixels * sum1).sqrt().sqrt();
    }
    out
}

/// One channel's edge-diff sums (raw, pre-normalization).
pub fn edge_diff_map_ch(img1: &[f32], mu1: &[f32], img2: &[f32], mu2: &[f32], sums: &mut [f64; 4]) {
    for i in 0..img1.len() {
        let num = 1.0 + (img2[i] - mu2[i]).abs() as f64;
        let den = 1.0 + (img1[i] - mu1[i]).abs() as f64;
        let d1 = num / den - 1.0;
        let artifact = d1.max(0.0);
        sums[0] += artifact;
        sums[1] += tothe4th(artifact);
        let detail_lost = (-d1).max(0.0);
        sums[2] += detail_lost;
        sums[3] += tothe4th(detail_lost);
    }
}

/// `EdgeDiffMap` → 12 values: per channel
/// {artifact L1, artifact L4, detail_lost L1, detail_lost L4}.
pub fn edge_diff_map(
    img1: &[Vec<f32>; 3],
    mu1: &[Vec<f32>; 3],
    img2: &[Vec<f32>; 3],
    mu2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> [f64; 12] {
    let one_per_pixels = 1.0 / (height * width) as f64;
    let mut out = [0f64; 12];
    for c in 0..3 {
        let mut sums = [0f64; 4];
        for i in 0..width * height {
            // f32 abs, f64 addition/division/subtraction.
            let num = 1.0 + (img2[c][i] - mu2[c][i]).abs() as f64;
            let den = 1.0 + (img1[c][i] - mu1[c][i]).abs() as f64;
            let d1 = num / den - 1.0;
            let artifact = d1.max(0.0);
            sums[0] += artifact;
            sums[1] += tothe4th(artifact);
            let detail_lost = (-d1).max(0.0);
            sums[2] += detail_lost;
            sums[3] += tothe4th(detail_lost);
        }
        out[c * 4] = one_per_pixels * sums[0];
        out[c * 4 + 1] = (one_per_pixels * sums[1]).sqrt().sqrt();
        out[c * 4 + 2] = one_per_pixels * sums[2];
        out[c * 4 + 3] = (one_per_pixels * sums[3]).sqrt().sqrt();
    }
    out
}

/// Pixels between `stop` polls inside `maps_fused_stop`'s per-pixel loop.
const MAPS_STOP_STRIDE: usize = 1 << 21;

/// Fused `SSIMMap` + `EdgeDiffMap` — one row sweep shares the
/// mu1/mu2/img loads. Each map's f64 accumulation order is identical to
/// the separate passes, so results are bit-exact vs calling the two fns.
pub fn maps_fused(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    img1: &[Vec<f32>; 3],
    img2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
) -> ([f64; 6], [f64; 12]) {
    // Unstoppable never fires — the only effect is strided checks.
    match maps_fused_stop(
        m1,
        m2,
        s11,
        s22,
        s12,
        img1,
        img2,
        width,
        height,
        &enough::Unstoppable,
    ) {
        Ok(o) => o,
        Err(_) => unreachable!("Unstoppable never stops"),
    }
}

/// [`maps_fused`] with cooperative cancellation — `stop` is checked per
/// channel and per `MAPS_STOP_STRIDE`-pixel chunk inside the fused loop
/// (accumulators stay scalar-ordered, so chunking is bit-exact).
#[allow(clippy::too_many_arguments)]
pub(crate) fn maps_fused_stop(
    m1: &[Vec<f32>; 3],
    m2: &[Vec<f32>; 3],
    s11: &[Vec<f32>; 3],
    s22: &[Vec<f32>; 3],
    s12: &[Vec<f32>; 3],
    img1: &[Vec<f32>; 3],
    img2: &[Vec<f32>; 3],
    width: usize,
    height: usize,
    stop: &dyn enough::Stop,
) -> Result<([f64; 6], [f64; 12]), enough::StopReason> {
    let stop = stop.may_stop().then_some(stop);
    let (mut ssim_out, mut edge_out) = ([0f64; 6], [0f64; 12]);
    for c in 0..3 {
        stop.check()?;
        let (ssim, edge) = maps_channel_stop(
            &m1[c], &m2[c], &s11[c], &s22[c], &s12[c], &img1[c], &img2[c], width, height, stop,
        )?;
        ssim_out[c * 2..c * 2 + 2].copy_from_slice(&ssim);
        edge_out[c * 4..c * 4 + 4].copy_from_slice(&edge);
    }
    Ok((ssim_out, edge_out))
}

/// One channel of [`maps_fused_stop`]: its `SSIMMap` pair and `EdgeDiffMap` quad. pngdeer patch:
/// split out so the low-memory comparison, which scores channel by channel, runs this same code.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maps_channel_stop(
    m1: &[f32],
    m2: &[f32],
    s11: &[f32],
    s22: &[f32],
    s12: &[f32],
    img1: &[f32],
    img2: &[f32],
    width: usize,
    height: usize,
    stop: Option<&dyn enough::Stop>,
) -> Result<([f64; 2], [f64; 4]), enough::StopReason> {
    let one_per_pixels = 1.0 / (height * width) as f64;
    let npix = width * height;
    let (mut sum0, mut sum1) = (0f64, 0f64);
    let mut esums = [0f64; 4];
    for chunk_start in (0..npix).step_by(MAPS_STOP_STRIDE) {
        for i in chunk_start..npix.min(chunk_start + MAPS_STOP_STRIDE) {
            let mu1 = m1[i];
            let mu2 = m2[i];
            let mu11 = mu1 * mu1;
            let mu22 = mu2 * mu2;
            let mu12 = mu1 * mu2;
            let dm = mu1 - mu2;
            let num_m = (-dm).fma(dm, 1.0f32);
            let num_s = 2.0f32 * (s12[i] - mu12) + K_C2;
            let denom_s = (s11[i] - mu11) + (s22[i] - mu22) + K_C2;
            let q = num_m * num_s / denom_s;
            let d = (1.0f64 - q as f64).max(0.0);
            sum0 += d;
            sum1 += tothe4th(d);

            let num = 1.0 + (img2[i] - mu2).abs() as f64;
            let den = 1.0 + (img1[i] - mu1).abs() as f64;
            let d1 = num / den - 1.0;
            let artifact = d1.max(0.0);
            esums[0] += artifact;
            esums[1] += tothe4th(artifact);
            let detail_lost = (-d1).max(0.0);
            esums[2] += detail_lost;
            esums[3] += tothe4th(detail_lost);
        }
        stop.check()?;
    }
    Ok((
        [one_per_pixels * sum0, (one_per_pixels * sum1).sqrt().sqrt()],
        [
            one_per_pixels * esums[0],
            (one_per_pixels * esums[1]).sqrt().sqrt(),
            one_per_pixels * esums[2],
            (one_per_pixels * esums[3]).sqrt().sqrt(),
        ],
    ))
}
