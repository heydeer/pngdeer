#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]
//! `Msssim::Score` — 108-weight walk and final nonlinear transform.
//!
//! The reference binary evaluates this in plain f64 mulsd/addsd (verified by
//! disassembly: no fma contraction), matching Rust's uncontracted ops.
//! `pow` resolves to the system libm in both implementations.

use crate::weights::WEIGHT;

/// Per-scale aggregates: 6 ssim + 12 edge-diff values.
pub struct ScaleAggregates {
    pub avg_ssim: [f64; 6],
    pub avg_edgediff: [f64; 12],
}

/// Final score, exactly as `Msssim::Score`.
///
/// The weight walk is channel-major: for each channel c ∈ {X, Y, B}, each
/// scale, each norm n ∈ {L1, L4}, it consumes the interleaved triple
/// `{ssim[c][n], artifact[c][n], detail_lost[c][n]}` — i.e. the WEIGHT
/// table's `((c * NUM_SCALES + scale) * 2 + n) * 3 + m` layout.
pub fn score(scales: &[ScaleAggregates]) -> f64 {
    let mut ssim = 0.0f64;
    let mut i = 0usize;
    for c in 0..3 {
        for s in scales {
            for n in 0..2 {
                ssim += WEIGHT[i] * s.avg_ssim[c * 2 + n].abs();
                i += 1;
                ssim += WEIGHT[i] * s.avg_edgediff[c * 4 + n].abs();
                i += 1;
                ssim += WEIGHT[i] * s.avg_edgediff[c * 4 + n + 2].abs();
                i += 1;
            }
        }
    }

    ssim = ssim * 0.956_238_261_683_484_4;
    let ssim = 2.326_765_642_916_932 * ssim - 0.020_884_521_182_843_837 * ssim * ssim
        + 6.248_496_625_763_138e-05 * ssim * ssim * ssim;
    if ssim > 0.0 {
        100.0 - 10.0 * ssim.powf(0.627_633_646_783_138_7)
    } else {
        100.0
    }
}
