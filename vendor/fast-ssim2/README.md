# fast-ssim2 [![CI](https://img.shields.io/github/actions/workflow/status/imazen/fast-ssim2/ci.yml?style=flat-square&label=CI)](https://github.com/imazen/fast-ssim2/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/fast-ssim2?style=flat-square)](https://crates.io/crates/fast-ssim2) [![lib.rs](https://img.shields.io/crates/v/fast-ssim2?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/fast-ssim2) [![docs.rs](https://img.shields.io/docsrs/fast-ssim2?style=flat-square)](https://docs.rs/fast-ssim2) [![MSRV](https://img.shields.io/badge/MSRV-1.89-blue?style=flat-square)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field) [![license](https://img.shields.io/crates/l/fast-ssim2?style=flat-square)](#license)

Fast SIMD-accelerated [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2)
image quality scoring in safe Rust. Compare two images, or cache a reference
for repeated comparisons. Runtime dispatch selects SIMD where available;
`SimdImpl::Scalar` selects the scalar kernels.

## Compare images

Inputs are borrowed `PixelSlice` views. The descriptor declares the pixel
layout and color semantics; stride is **bytes per row**, including padding.
Dimensions are `u32`; stride is `usize`. Both images must have the same original
width and height, even when they are smaller than 8×8.

```rust
use fast_ssim2::{PixelDescriptor, PixelSlice, compute_ssimulacra2};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (width, height) = (16u32, 16u32);
    let source_bytes = vec![128u8; width as usize * height as usize * 3];
    let distorted_bytes = vec![120u8; source_bytes.len()];
    let stride = width as usize * 3;
    let source = PixelSlice::new(
        &source_bytes, width, height, stride, PixelDescriptor::RGB8_SRGB,
    )?;
    let distorted = PixelSlice::new(
        &distorted_bytes, width, height, stride, PixelDescriptor::RGB8_SRGB,
    )?;
    let score = compute_ssimulacra2(&source, &distorted)?;
    println!("SSIMULACRA2: {score}");
    Ok(())
}
```

Higher scores indicate closer agreement. Identical images score 100; severe
distortions can produce negative scores. Scores are not percentages, and a
single threshold does not guarantee that a difference is invisible.

`PixelBuffer` owners pass `&buffer.as_slice()`. With the `imgref` feature,
`zenpixels` provides conversions from supported `imgref`/`rgb` pixel types.
Raw interleaved bytes need no intermediate pixel vector.

## Configure a comparison

`Ssimulacra2Config` combines backend selection, strip options, and cancellation.
Builders retain the other options. Configuration and error types are
non-exhaustive so callers can adopt future additions without exhaustive matches
or struct literals.

```rust
use fast_ssim2::{
    PixelDescriptor, PixelSlice, SimdImpl, Ssimulacra2Config, StripConfig,
    compute_ssimulacra2_with_config,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pixels = vec![128u8; 16 * 16 * 3];
    let image = PixelSlice::new(&pixels, 16, 16, 48, PixelDescriptor::RGB8_SRGB)?;
    let config = Ssimulacra2Config::default()
        .with_impl(SimdImpl::Scalar)
        .with_strip(StripConfig::new(128).with_halo_rows(96));
    let score = compute_ssimulacra2_with_config(&image, &image, &config)?;
    assert_eq!(score, 100.0);
    Ok(())
}
```

`Ssimulacra2Config::strips(rows)` is shorthand for SIMD strip evaluation.
`StripConfig::default()` requests 256 interior rows and 96 halo rows.
Interior boundaries are rounded to the pipeline's 32-row alignment.
A requested height below eight returns `InvalidConfiguration`.

Strip processing reduces the size of intermediate blur and metric buffers.
**It is not a streaming input API:** input conversion still materializes
whole-image buffers, and cached comparisons retain the full reference pyramid.
Total memory therefore still includes terms proportional to image size.
Strip boundaries and reduction order can change the score slightly; use
whole-image mode when exact agreement with that mode is required. Increasing
the halo trades more work for smaller boundary effects.

Parallel strips require `rayon` and explicit
`StripConfig::with_parallel_strips(true)`. Without the feature this option
returns `InvalidConfiguration`. Parallel processing runs up to eight strips at
once and increases intermediate memory use. Results merge in fixed strip order.

## Reuse a reference

```rust
use fast_ssim2::{PixelDescriptor, PixelSlice, Ssimulacra2Reference};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source_bytes = vec![128u8; 16 * 16 * 3];
    let source = PixelSlice::new(&source_bytes, 16, 16, 48, PixelDescriptor::RGB8_SRGB)?;
    let reference = Ssimulacra2Reference::new(&source)?;
    for value in [120u8, 124, 128] {
        let bytes = vec![value; source_bytes.len()];
        let distorted = PixelSlice::new(&bytes, 16, 16, 48, PixelDescriptor::RGB8_SRGB)?;
        println!("{value}: {}", reference.compare(&distorted)?);
    }
    Ok(())
}
```

`new_with_config` accepts backend selection and cancellation. It rejects strip
options because construction stores a full reference pyramid. Use
`compare_with_config` to select strips for subsequent comparisons. The reference
owns its data and can be shared across threads; cloning it copies its buffers.
Comparisons allocate temporary storage; there is no zero-allocation scratch API.

## Cancellation

Pass an `enough::Stop` token through `with_stop`. Cancellation is checked before
input conversion, between row or pixel chunks inside expensive passes, and at
scale or strip boundaries, including reference construction. It does not
interrupt a pixel operation already in progress.
An already-cancelled token returns `Ssimulacra2Error::Cancelled`.

```rust
use fast_ssim2::{PixelDescriptor, PixelSlice, Ssimulacra2Config, compute_ssimulacra2_with_config};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = vec![128u8; 16 * 16 * 3];
    let image = PixelSlice::new(&bytes, 16, 16, 48, PixelDescriptor::RGB8_SRGB)?;
    // Replace Unstoppable with your application's enough::Stop implementation.
    let stop = enough::Unstoppable;
    let config = Ssimulacra2Config::default().with_stop(&stop);
    assert_eq!(compute_ssimulacra2_with_config(&image, &image, &config)?, 100.0);
    Ok(())
}
```

## Input semantics

The descriptor controls interpretation; sample type alone does not determine
transfer function. SDR entry points require full-range BT.709 primaries and
support these layouts:

| Layout | Transfer | Conversion |
|---|---|---|
| RGB8, RGBA8, BGRA8, RGBX8, BGRX8, Gray8 | sRGB | Captured u8 LUT for opaque pixels |
| RGB16, RGBA16, Gray16 | sRGB | sRGB polynomial |
| RGBF32, RGBAF32 | sRGB | sRGB polynomial |
| RGBF32, RGBAF32, GrayF32, GrayAF32 | Linear | Used directly |

Integer samples span their full type range. SDR float inputs use normalized
color values; alpha is normalized to 0–1. Grayscale expands to equal RGB
channels. Alpha mode comes from the descriptor: straight alpha is composited,
premultiplied alpha is unmultiplied first, and absent/opaque alpha is ignored.
Integer unpremultiplication rounds back to integer samples. X channels are
padding under their standard descriptors.

If either image has alpha, scoring evaluates two backgrounds and takes the
lower score. Encoded inputs composite at sRGB values 0.1 and 0.9 before
linearization; linear inputs composite at the corresponding linear values.
Only opaque u8 sRGB uses the captured LUT throughout input conversion;
u16, float, and fractional-alpha conversions do not promise binary-reference
bit equality.

Unsupported descriptors return `UnsupportedInput`. Convert other color spaces
upstream, for example with [zenpixels-convert](https://lib.rs/crates/zenpixels-convert).

## HDR

The `hdr-pu` feature adds `compute_ssimulacra2_pu` and
`compute_ssimulacra2_pu_with_config`. These replace the cube-root encoding with
PU21 and produce **scores that are not interchangeable with SDR scores**.

Linear f32 inputs are absolute luminance in cd/m² (nits), not normalized SDR.
PQ inputs decode to nits; HLG uses a 1000-nit reference display with system
gamma 1.2. Supported layouts include RGB/RGBA, BGRX/BGRA u8, and grayscale
(u8/u16/f32; grayscale alpha in u16/f32). Signal range must be full.
Primaries are not converted: callers must supply both images in the same
primaries. Alpha composites over 20- and 200-nit backgrounds. HDR strip options
return `InvalidConfiguration`; there is no cached HDR reference API.

## Features and limits

| Feature | Purpose |
|---|---|
| `imgref` | Forward `zenpixels/imgref` conversions |
| `rayon` | Parallel kernels and opt-in parallel strips |
| `hdr-pu` | HDR scoring |
| `unstable-internals` | Development tools only; exposes kernels without an API stability guarantee |

No features are enabled by default. Runtime SIMD dispatch is available without
a feature flag. The MSRV is Rust 1.89.

Empty images are rejected. Images below 8×8 are reflect-padded, including strip
and cached paths. Original and padded sizes must fit `MAX_IMAGE_PIXELS`
(268,435,456 pixels). This is a pixel-count limit, not a memory budget; impose
smaller application limits where needed. Scoring allocations are infallible.

See [CHANGELOG.md](https://github.com/imazen/fast-ssim2/blob/main/CHANGELOG.md)
for migration from the released trait-based API, and
[benchmarks/README.md](https://github.com/imazen/fast-ssim2/blob/main/benchmarks/README.md)
for recorded benchmark methodology and results.

## Credits

This crate is a fork of [rust-av/ssimulacra2](https://github.com/rust-av/ssimulacra2)
(BSD-2-Clause) — thank you to the rust-av team for the original Rust
implementation. The SSIMULACRA2 metric itself was created by
[Cloudinary](https://github.com/cloudinary/ssimulacra2) (Jon Sneyers and
colleagues) and is maintained in [libjxl](https://github.com/libjxl/libjxl); all
credit for the algorithm and its calibration belongs to them.

**What this fork adds:** cross-platform SIMD acceleration (x86_64 / aarch64 /
wasm32 via [archmage](https://crates.io/crates/archmage)), a precomputed-reference
batch API, a bounded-memory strip path, cooperative cancellation, `imgref`
support, and `#![forbid(unsafe_code)]`.

## License

BSD-2-Clause, the same license as upstream
[rust-av/ssimulacra2](https://github.com/rust-av/ssimulacra2). See
[LICENSE](https://github.com/imazen/fast-ssim2/blob/main/LICENSE).

We are glad to release our improvements under the original BSD-2-Clause license
if upstream wants to take over maintenance of them — we would rather contribute
back than maintain a parallel codebase. Open an issue or reach out.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] · [zenjxl] · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · [zenraw] · [ultrahdr] |
| Codec internals | [zenjxl-decoder] · [jxl-encoder] · [zenrav1e] · [rav1d-safe] · [zenavif-parse] · [zenavif-serialize] |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · [zenfilters] · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline & framework | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · **fast-ssim2** · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zentiff
[zenpdf]: https://github.com/imazen/zenpdf
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[zenraw]: https://github.com/imazen/zenraw
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zenfilters]: https://github.com/imazen/zenfilters
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[zenwasm]: https://github.com/imazen/zenwasm
[zentract]: https://github.com/imazen/zentract
[zensim]: https://github.com/imazen/zensim
[butteraugli]: https://github.com/imazen/butteraugli
[zenmetrics]: https://github.com/imazen/zenmetrics
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[zenanalyze]: https://github.com/imazen/zenanalyze
[zenpredict]: https://github.com/imazen/zenanalyze
[zenpicker]: https://github.com/imazen/zenanalyze
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
[`enough::Unstoppable`]: https://docs.rs/enough/latest/enough/struct.Unstoppable.html
[`Ssimulacra2Error::Cancelled`]: https://docs.rs/fast-ssim2/latest/fast_ssim2/enum.Ssimulacra2Error.html#variant.Cancelled
