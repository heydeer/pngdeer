#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity
)]
//! Bit-exact port of libjxl's `FastGaussian` (Charalampidis IIR Gaussian,
//! sigma = 1.5) as used by the reference SSIMULACRA2 implementation.
//!
//! The reference code runs on the `HWY_CAPPED(float, 4)` trait for the
//! horizontal pass, so its accumulation structure is a fixed 4-lane unroll
//! regardless of the host SIMD width (except `HWY_SCALAR`, which no shipped
//! binary uses). The vertical pass is lane-independent. Reproducing the
//! exact FMA ordering of both passes is what makes the score bit-identical
//! to the official binaries.
//!
//! Reference: `ssimulacra2/src/lib/jxl/gauss_blur.cc` and
//! `libjxl v0.12.0 lib/jxl/gauss_blur.cc` (numerically identical).

/// Recursive Gaussian coefficients, mirroring `struct RecursiveGaussian`.
///
/// Lane j of each `mul_*` array is the coefficient for the unrolled
/// recurrence term j steps back (JXL_GAUSS_MAX_LANES = 4).
#[allow(unused_imports)]
use super::Fma; // pngdeer patch
pub struct RecursiveGaussian {
    pub radius: i32,
    /// `n2[i]` for i in {0,1,2} (oscillator terms 1,3,5).
    pub n2: [f32; 3],
    /// `d1[i]` for i in {0,1,2}.
    pub d1: [f32; 3],
    pub mul_in: [[f32; 4]; 3],
    pub mul_prev: [[f32; 4]; 3],
    pub mul_prev2: [[f32; 4]; 3],
}

/// Port of `CreateRecursiveGaussian` — all f64 math in identical order.
pub fn create_recursive_gaussian(sigma: f64) -> RecursiveGaussian {
    /// `M_PI` as transcribed in the C++ source (equals `f64::consts::PI`).
    #[allow(clippy::approx_constant, clippy::excessive_precision)]
    const K_PI: f64 = 3.141592653589793238;

    // `roundf` in C++ rounds half away from zero; f64::round matches.
    let radius = (3.2795 * sigma + 0.2546).round();

    let pi_div_2r = K_PI / (2.0 * radius);
    let omega = [pi_div_2r, 3.0 * pi_div_2r, 5.0 * pi_div_2r];

    let p_1 = 1.0 / (0.5 * omega[0]).tan();
    let p_3 = -1.0 / (0.5 * omega[1]).tan();
    let p_5 = 1.0 / (0.5 * omega[2]).tan();

    let r_1 = p_1 * p_1 / omega[0].sin();
    let r_3 = -p_3 * p_3 / omega[1].sin();
    let r_5 = p_5 * p_5 / omega[2].sin();

    let neg_half_sigma2 = -0.5 * sigma * sigma;
    let recip_radius = 1.0 / radius;
    let rho = [
        (neg_half_sigma2 * omega[0] * omega[0]).exp() * recip_radius,
        (neg_half_sigma2 * omega[1] * omega[1]).exp() * recip_radius,
        (neg_half_sigma2 * omega[2] * omega[2]).exp() * recip_radius,
    ];

    let d_13 = p_1 * r_3 - r_1 * p_3;
    let d_35 = p_3 * r_5 - r_3 * p_5;
    let d_51 = p_5 * r_1 - r_5 * p_1;

    let recip_d13 = 1.0 / d_13;
    let zeta_15 = d_35 * recip_d13;
    let zeta_35 = d_51 * recip_d13;

    // Invert the 3x3 matrix in place (Inv3x3Matrix), intermediate in f64.
    let mut a = [p_1, p_3, p_5, r_1, r_3, r_5, zeta_15, zeta_35, 1.0];
    let t = [
        a[4] * a[8] - a[5] * a[7],
        a[2] * a[7] - a[1] * a[8],
        a[1] * a[5] - a[2] * a[4],
        a[5] * a[6] - a[3] * a[8],
        a[0] * a[8] - a[2] * a[6],
        a[2] * a[3] - a[0] * a[5],
        a[3] * a[7] - a[4] * a[6],
        a[1] * a[6] - a[0] * a[7],
        a[0] * a[4] - a[1] * a[3],
    ];
    let det = a[0] * t[0] + a[1] * t[3] + a[2] * t[6];
    let idet = 1.0 / det;
    for i in 0..9 {
        a[i] = t[i] * idet;
    }

    // MatMul(A, gamma, 3, 3, 1, beta) — column of A-inv times gamma vector.
    let gamma = [
        1.0,
        radius * radius - sigma * sigma,
        zeta_15 * rho[0] + zeta_35 * rho[1] + rho[2],
    ];
    let mut beta = [0.0f64; 3];
    for (y, b) in beta.iter_mut().enumerate() {
        *b = a[y * 3] * gamma[0] + a[y * 3 + 1] * gamma[1] + a[y * 3 + 2] * gamma[2];
    }
    // C++ accumulates `e += a[...] * temp[z]` left-to-right; for 3 terms the
    // order is ((t0+t1)+t2) — same as the fma-free expression above since
    // Rust does not contract fmul+fadd into fma.

    let mut n2 = [0f32; 3];
    let mut d1 = [0f32; 3];
    let mut mul_in = [[0f32; 4]; 3];
    let mut mul_prev = [[0f32; 4]; 3];
    let mut mul_prev2 = [[0f32; 4]; 3];

    for i in 0..3 {
        let n2_i = -beta[i] * (omega[i] * (radius + 1.0)).cos(); // (33)
        let d1_i = -2.0 * omega[i].cos(); // (33)
        n2[i] = n2_i as f32;
        d1[i] = d1_i as f32;

        let d_2 = d1_i * d1_i;
        // C++ fills all four lanes of each entry with the same expression.
        mul_prev[i] = [
            (-d1_i) as f32,
            (d_2 - 1.0) as f32,
            (-d_2 * d1_i + 2.0 * d1_i) as f32,
            (d_2 * d_2 - 3.0 * d_2 + 1.0) as f32,
        ];
        mul_prev2[i] = [
            -1.0,
            d1_i as f32,
            (-d_2 + 1.0) as f32,
            (d_2 * d1_i - 2.0 * d1_i) as f32,
        ];
        mul_in[i] = [
            n2_i as f32,
            (-d1_i * n2_i) as f32,
            (d_2 * n2_i - n2_i) as f32,
            (-d_2 * d1_i * n2_i + 2.0 * d1_i * n2_i) as f32,
        ];
    }

    RecursiveGaussian {
        radius: radius as i32,
        n2,
        d1,
        mul_in,
        mul_prev,
        mul_prev2,
    }
}

impl RecursiveGaussian {
    /// Port of `FastGaussian1D` — scalar prologue, 4-wide unrolled middle
    /// (`HWY_CAPPED(float,4)`), scalar remainder. fma order matches the
    /// C++ `MulAdd`/`NegMulSub` sequence lane-for-lane.
    pub fn fast_gaussian_1d(&self, input: &[f32], output: &mut [f32]) {
        let width = input.len() as i64;
        let n_radius = self.radius as i64;

        let mut prev = [0f32; 3];
        let mut prev2 = [0f32; 3];

        let mut n = -n_radius + 1;

        // Scalar section used by both boundary loops.
        macro_rules! scalar_step {
            () => {{
                let left = n - n_radius - 1;
                let right = n + n_radius - 1;
                let left_val = if left >= 0 { input[left as usize] } else { 0.0 };
                let right_val = if right < width {
                    input[right as usize]
                } else {
                    0.0
                };
                let sum = left_val + right_val;
                let mut out = [0f32; 3];
                for i in 0..3 {
                    let mut o = sum * self.mul_in[i][0];
                    o = self.mul_prev2[i][0].fma(prev2[i], o);
                    prev2[i] = prev[i];
                    o = self.mul_prev[i][0].fma(prev[i], o);
                    prev[i] = o;
                    out[i] = o;
                }
                if n >= 0 {
                    output[n as usize] = out[0] + (out[1] + out[2]);
                }
            }};
        }

        // Left side with bounds checks; first_aligned = RoundUpTo(N+1, 4).
        let first_aligned = (n_radius + 1).div_euclid(4) * 4
            + if (n_radius + 1).rem_euclid(4) != 0 {
                4
            } else {
                0
            };
        while n < first_aligned.min(width) {
            scalar_step!();
            n += 1;
        }

        // Unrolled, no bounds checking — 4 outputs per iteration.
        while n < width - n_radius + 1 - 3 {
            let base = n as usize;
            let off = n_radius as usize;
            // sum[j] = in[n+j-N-1] + in[n+j+N-1]
            let sum = [
                input[base - off - 1] + input[base + off - 1],
                input[base - off] + input[base + off],
                input[base - off + 1] + input[base + off + 1],
                input[base - off + 2] + input[base + off + 2],
            ];
            let mut out = [[0f32; 3]; 4];
            for j in 0..4 {
                for i in 0..3 {
                    let mut acc = sum[0] * self.mul_in[i][j];
                    if j >= 1 {
                        acc = self.mul_in[i][j - 1].fma(sum[1], acc);
                    }
                    if j >= 2 {
                        acc = self.mul_in[i][j - 2].fma(sum[2], acc);
                    }
                    if j >= 3 {
                        acc = self.mul_in[i][j - 3].fma(sum[3], acc);
                    }
                    acc = self.mul_prev2[i][j].fma(prev2[i], acc);
                    acc = self.mul_prev[i][j].fma(prev[i], acc);
                    out[j][i] = acc;
                }
            }
            for i in 0..3 {
                prev2[i] = out[2][i];
                prev[i] = out[3][i];
            }
            for j in 0..4 {
                output[base + j] = out[j][0] + (out[j][1] + out[j][2]);
            }
            n += 4;
        }

        // Right-side remainder with bounds checks.
        while n < width {
            scalar_step!();
            n += 1;
        }
    }

    /// Port of `FastGaussianVertical` for a single column. Lane-independent:
    /// each column is an independent 1D recurrence, so a scalar loop is
    /// bit-identical to the C++ vector lanes.
    ///
    /// `get` reads the input row values; `put` writes outputs.
    pub fn fast_gaussian_vertical_1d(
        &self,
        width: usize,
        height: usize,
        get: impl Fn(usize, usize) -> f32,
        put: &mut impl FnMut(usize, usize, f32),
    ) {
        self.fast_gaussian_vertical_columns(0, width, height, get, put);
    }

    /// [`Self::fast_gaussian_vertical_1d`] for the columns `start..end` only: each column's
    /// recursion is its own, so column ranges can run on separate threads (pngdeer patch).
    pub fn fast_gaussian_vertical_columns(
        &self,
        start: usize,
        end: usize,
        height: usize,
        get: impl Fn(usize, usize) -> f32,
        put: &mut impl FnMut(usize, usize, f32),
    ) {
        let n_rad = self.radius as i64;
        // pngdeer patch: columns in blocks, walked row by row, instead of one whole column at a
        // time (a stride-`width` walk per column that thrashes the cache: ~40% of all scoring time
        // in wasm). The arithmetic per column is unchanged, so results are bit-identical.
        const BLOCK: usize = 64;
        let mut x0 = start;
        while x0 < end {
            let x1 = (x0 + BLOCK).min(end);
            // Ring buffer of last two outputs per term (kMod=4 in C++,
            // semantically a 3-slot history), per column of the block.
            let mut y1_hist = [[0f32; 4]; BLOCK];
            let mut y3_hist = [[0f32; 4]; BLOCK];
            let mut y5_hist = [[0f32; 4]; BLOCK];
            let mut ctr: usize = 0;

            let mut n: i64 = -n_rad + 1;
            while n < height as i64 {
                let top = n - n_rad - 1;
                let bottom = n + n_rad - 1;
                ctr = ctr.wrapping_add(1);
                let n_0 = ctr % 4;
                let n_1 = (ctr.wrapping_sub(1)) % 4;
                let n_2 = (ctr.wrapping_sub(2)) % 4;
                for x in x0..x1 {
                    // During warmup and the top-border phase only `bottom` is fed;
                    // top is out of bounds and contributes nothing — numerically
                    // identical to adding 0. Interior adds in[top] + in[bottom].
                    let sum = if top < 0 {
                        if bottom < height as i64 {
                            get(bottom as usize, x)
                        } else {
                            0.0
                        }
                    } else {
                        get(top as usize, x)
                            + if bottom < height as i64 {
                                get(bottom as usize, x)
                            } else {
                                0.0
                            }
                    };

                    let (h1, h3, h5) = (&mut y1_hist[x - x0], &mut y3_hist[x - x0], &mut y5_hist[x - x0]);
                    // NegMulSub(d1, y_n1, y_n2) = -(d1*y_n1) - y_n2, then
                    // MulAdd(n2, sum, that) = fma(n2, sum, fma(-d1, y_n1, -y_n2)).
                    let y1 = self.n2[0].fma(sum, (-self.d1[0]).fma(h1[n_1], -h1[n_2]));
                    let y3 = self.n2[1].fma(sum, (-self.d1[1]).fma(h3[n_1], -h3[n_2]));
                    let y5 = self.n2[2].fma(sum, (-self.d1[2]).fma(h5[n_1], -h5[n_2]));
                    h1[n_0] = y1;
                    h3[n_0] = y3;
                    h5[n_0] = y5;

                    if n >= 0 {
                        put(n as usize, x, y1 + (y3 + y5));
                    }
                }
                n += 1;
            }
            x0 = x1;
        }
    }
}

/// `Multiply` — element-wise plane product, f32.
pub fn multiply_planes(a: &[Vec<f32>; 3], b: &[Vec<f32>; 3], out: &mut [Vec<f32>; 3]) {
    for c in 0..3 {
        for i in 0..a[c].len() {
            out[c][i] = a[c][i] * b[c][i];
        }
    }
}
