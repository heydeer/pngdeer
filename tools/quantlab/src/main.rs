//! quantlab ALGO COLORS DITHER IN.png OUT.png: one palette PNG from another open-source quantizer, for
//! bench/bench.py's side-by-side comparison. ALGO is exoquant (k-means), quantette (Wu + k-means in
//! Oklab), neuquant (color_quant's NeuQuant) or quantizr (imgproxy's); DITHER is 0 (nearest color) or 1
//! (each library's own Floyd–Steinberg; NeuQuant has none, so exoquant's remapper dithers its palette).
//! quantette handles only opaque colors: pixels under half alpha become one transparent entry.

use std::{env, fs, process};

use exoquant::{ditherer, optimizer, Remapper, SimpleColorSpace};
use pngdeer::{codec, RGBA};

type Error = Box<dyn std::error::Error>;

fn exo(c: RGBA) -> exoquant::Color {
    exoquant::Color::new(c.r, c.g, c.b, c.a)
}

/// Indices into `palette` by exoquant's remapper, with or without Floyd–Steinberg.
fn remap(px: &[RGBA], w: usize, palette: &[RGBA], dither: bool) -> Vec<u8> {
    let (pal, img): (Vec<_>, Vec<_>) = (palette.iter().map(|&c| exo(c)).collect(), px.iter().map(|&c| exo(c)).collect());
    let space = SimpleColorSpace::default();
    if dither {
        Remapper::new(&pal, &space, &ditherer::FloydSteinberg::new()).remap(&img, w)
    } else {
        Remapper::new(&pal, &space, &ditherer::None).remap(&img, w)
    }
}

fn quantize(algo: &str, px: &[RGBA], w: usize, h: usize, colors: usize, dither: bool) -> Result<(Vec<RGBA>, Vec<u8>), Error> {
    Ok(match algo {
        "exoquant" => {
            let img: Vec<_> = px.iter().map(|&c| exo(c)).collect();
            let (pal, idx) = if dither {
                exoquant::convert_to_indexed(&img, w, colors, &optimizer::KMeans, &ditherer::FloydSteinberg::new())
            } else {
                exoquant::convert_to_indexed(&img, w, colors, &optimizer::KMeans, &ditherer::None)
            };
            (pal.iter().map(|c| RGBA::new(c.r, c.g, c.b, c.a)).collect(), idx)
        }
        "neuquant" => {
            let bytes: Vec<u8> = px.iter().flat_map(|c| [c.r, c.g, c.b, c.a]).collect();
            let nq = color_quant::NeuQuant::new(10, colors, &bytes);
            let pal: Vec<RGBA> = nq.color_map_rgba().chunks(4).map(|c| RGBA::new(c[0], c[1], c[2], c[3])).collect();
            let idx = if dither { remap(px, w, &pal, true) } else { bytes.chunks(4).map(|p| nq.index_of(p) as u8).collect() };
            (pal, idx)
        }
        "quantizr" => {
            let bytes: Vec<u8> = px.iter().flat_map(|c| [c.r, c.g, c.b, c.a]).collect();
            let image = quantizr::Image::new(&bytes, w, h)?;
            let mut opts = quantizr::Options::default();
            opts.set_max_colors(colors as i32)?;
            let mut res = quantizr::QuantizeResult::quantize(&image, &opts);
            res.set_dithering_level(if dither { 1.0 } else { 0.0 })?;
            let mut idx = vec![0u8; w * h];
            res.remap_image(&image, &mut idx)?;
            let p = res.get_palette();
            (p.entries[..p.count as usize].iter().map(|c| RGBA::new(c.r, c.g, c.b, c.a)).collect(), idx)
        }
        "quantette" => {
            use quantette::{ImageBuf, Pipeline, QuantizeMethod};
            let srgb = |c: &RGBA| palette::Srgb::new(c.r, c.g, c.b);
            let transparent = px.iter().any(|c| c.a < 128);
            let opaque_colors = colors - transparent as usize;
            let pipeline = Pipeline::new()
                .palette_size(u16::try_from(opaque_colors)?.try_into()?)
                .quantize_method(QuantizeMethod::kmeans())
                .ditherer(dither.then(quantette::dither::FloydSteinberg::new));
            if !transparent {
                let image = ImageBuf::new(w as u32, h as u32, px.iter().map(srgb).collect::<Vec<_>>()).map_err(|_| "bad image size")?;
                let (pal, idx) = pipeline.input_image(image.as_ref()).output_srgb8_indexed_image().into_parts();
                (pal.iter().map(|c| RGBA::new(c.red, c.green, c.blue, 255)).collect(), idx)
            } else {
                let opaque: Vec<_> = px.iter().filter(|c| c.a >= 128).map(srgb).collect();
                let mut pal: Vec<RGBA> = if opaque.is_empty() {
                    Vec::new()
                } else {
                    pipeline.input_slice(&opaque)?.output_srgb8_palette().into_vec().iter().map(|c| RGBA::new(c.red, c.green, c.blue, 255)).collect()
                };
                // transparent pixels get an exact palette color, so dithering spreads no error from them
                let filler = pal.first().copied().unwrap_or(RGBA::new(0, 0, 0, 255));
                let solid: Vec<RGBA> = px.iter().map(|c| if c.a < 128 { filler } else { RGBA::new(c.r, c.g, c.b, 255) }).collect();
                let mut idx = remap(&solid, w, &pal, dither);
                for (i, c) in idx.iter_mut().zip(px) {
                    if c.a < 128 {
                        *i = pal.len() as u8;
                    }
                }
                pal.push(RGBA::new(0, 0, 0, 0));
                (pal, idx)
            }
        }
        _ => return Err(format!("unknown quantizer: {algo}").into()),
    })
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 6 {
        eprintln!("usage: quantlab exoquant|quantette|neuquant|quantizr COLORS 0|1 IN.png OUT.png");
        process::exit(2);
    }
    let (colors, dither): (usize, bool) = (args[2].parse()?, args[3] == "1");
    let src = codec::decode(&fs::read(&args[4])?)?;
    let (palette, indices) = quantize(&args[1], &src.pixels, src.width, src.height, colors.clamp(2, 256), dither)?;
    fs::write(&args[5], codec::encode_palette(&palette, &indices, src.width, src.height))?;
    Ok(())
}
