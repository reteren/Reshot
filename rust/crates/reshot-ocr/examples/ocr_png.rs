use std::{fs::File, io::BufReader};

use anyhow::{Context, Result, bail};
use reshot_ocr::{OcrLanguage, recognize};

fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .context("usage: ocr_png <image.png> [auto|ru|en]")?;
    let mode = match std::env::args().nth(2).as_deref() {
        Some("ru") => OcrLanguage::Ru,
        Some("en") => OcrLanguage::En,
        _ => OcrLanguage::Auto,
    };
    let mut decoder = png::Decoder::new(BufReader::new(File::open(&path)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut raw = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw)?;
    let source = &raw[..info.buffer_size()];
    let mut bgra = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    match info.color_type {
        png::ColorType::Rgba => {
            for pixel in source.chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        png::ColorType::Rgb => {
            for pixel in source.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &gray in source {
                bgra.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in source.chunks_exact(2) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => bail!("indexed PNG was not expanded"),
    }
    let lines = recognize(
        &bgra,
        info.width,
        info.height,
        info.width as usize * 4,
        mode,
    );
    for line in &lines {
        println!(
            "{}",
            line.words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    println!("{} line(s)", lines.len());
    Ok(())
}
