//! aviflab png IN OUT.png                IN (PNG or JPEG) as pngdeer decodes it, a JPEG turned upright,
//!                                       as an 8-bit PNG (RGB when opaque) for encoders that read PNG
//! aviflab enc IN OUT.avif Q SPEED       rav1e at quality Q (1–100) and speed SPEED (1–10), as
//!                                       aviflab::encode; prints the encode's milliseconds
//!
//! For experiments/avif.

use std::{env, fs, process, time::Instant};

use pngdeer::{codec, jpeg, RGBA};

fn decode(path: &str) -> (Vec<RGBA>, usize, usize) {
    let bytes = fs::read(path).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
    if jpeg::is_jpeg(&bytes) {
        let photo = jpeg::decode(&bytes).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
        jpeg::oriented(photo.pixels, photo.width, photo.height, photo.orientation)
    } else {
        let image = codec::decode(&bytes).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
        (image.pixels, image.width, image.height)
    }
}

fn fail(message: &str) -> ! {
    eprintln!("aviflab: {message}");
    process::exit(1);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["png", input, output] => {
            let (pixels, w, h) = decode(input);
            let opaque = pixels.iter().all(|p| p.a == 255);
            let data: Vec<u8> = pixels.iter().flat_map(|p| if opaque { vec![p.r, p.g, p.b] } else { vec![p.r, p.g, p.b, p.a] }).collect();
            let mut encoder = png::Encoder::new(fs::File::create(output).unwrap_or_else(|e| fail(&format!("{output}: {e}"))), w as u32, h as u32);
            encoder.set_color(if opaque { png::ColorType::Rgb } else { png::ColorType::Rgba });
            encoder.write_header().and_then(|mut w| w.write_image_data(&data)).unwrap_or_else(|e| fail(&e.to_string()));
        }
        ["enc", input, output, quality, speed] => {
            let (pixels, w, h) = decode(input);
            let (quality, speed) = (quality.parse().unwrap_or_else(|_| fail("bad Q")), speed.parse().unwrap_or_else(|_| fail("bad SPEED")));
            let start = Instant::now();
            let avif = aviflab::encode(&pixels, w, h, quality, speed);
            println!("{}", start.elapsed().as_millis());
            fs::write(output, avif).unwrap_or_else(|e| fail(&format!("{output}: {e}")));
        }
        _ => fail("usage: aviflab png IN OUT.png | aviflab enc IN OUT.avif Q SPEED"),
    }
}
