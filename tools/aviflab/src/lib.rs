//! rav1e (ravif 0.13, rav1e 0.8) as an AVIF output would use it: 10-bit 4:4:4 (ravif encodes no
//! subsampled chroma), ravif's psychovisual tuning, one thread. The command line (main.rs) and the
//! wasm build (`encode_png`, driven by experiments/avif/wasm.mjs) both encode through `encode`, so
//! their files can be compared byte for byte.

use rgb::RGBA8;

/// An AVIF file of the pixels at quality 1–100 and speed 1 (slowest) to 10.
pub fn encode(pixels: &[RGBA8], width: usize, height: usize, quality: f32, speed: u8) -> Vec<u8> {
    let encoder = ravif::Encoder::new()
        .with_quality(quality)
        .with_speed(speed)
        .with_bit_depth(ravif::BitDepth::Ten)
        .with_num_threads(Some(1));
    let out = if pixels.iter().all(|p| p.a == 255) {
        let rgb: Vec<_> = pixels.iter().map(|p| p.rgb()).collect();
        encoder.encode_rgb(ravif::Img::new(&rgb[..], width, height))
    } else {
        encoder.encode_rgba(ravif::Img::new(pixels, width, height))
    };
    out.expect("rav1e failed").avif_file
}

/// The pixels of an 8-bit RGB or RGBA PNG (as `aviflab png` writes them).
pub fn decode_png(bytes: &[u8]) -> (Vec<RGBA8>, usize, usize) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().expect("not a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("too large")];
    let info = reader.next_frame(&mut buf).expect("bad PNG");
    let data = &buf[..info.buffer_size()];
    let pixels = match info.color_type {
        png::ColorType::Rgb => data.chunks(3).map(|c| RGBA8::new(c[0], c[1], c[2], 255)).collect(),
        png::ColorType::Rgba => data.chunks(4).map(|c| RGBA8::new(c[0], c[1], c[2], c[3])).collect(),
        other => panic!("an 8-bit RGB or RGBA PNG, not {other:?}"),
    };
    (pixels, info.width as usize, info.height as usize)
}

/// Raw exports for node (no wasm-bindgen glue): copy a PNG in at `alloc(len)`, call `encode_png`,
/// read the AVIF at `out_ptr()`.
#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::sync::Mutex;

    static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    #[no_mangle]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        Box::leak(vec![0u8; len].into_boxed_slice()).as_mut_ptr()
    }

    /// Encodes the PNG at ptr..ptr+len; returns the AVIF's length.
    #[no_mangle]
    pub extern "C" fn encode_png(ptr: *const u8, len: usize, quality: f32, speed: u8) -> usize {
        let (pixels, width, height) = super::decode_png(unsafe { std::slice::from_raw_parts(ptr, len) });
        let avif = super::encode(&pixels, width, height, quality, speed);
        let n = avif.len();
        *OUT.lock().unwrap() = avif;
        n
    }

    #[no_mangle]
    pub extern "C" fn out_ptr() -> *const u8 {
        OUT.lock().unwrap().as_ptr()
    }
}
