use std::{fs::File, io::BufWriter, path::PathBuf};

use anyhow::Result;
use reshot_capture::ScreenCaptureService;

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("capture.png"));
    let frame = ScreenCaptureService::new().snapshot_all_monitors()?;
    let mut rgba = frame.pixels_bgra;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(&output)?),
        frame.width,
        frame.height,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    println!(
        "{}x{} @ ({},{}), {} monitors -> {}",
        frame.width,
        frame.height,
        frame.virtual_left,
        frame.virtual_top,
        frame.monitors.len(),
        output.display()
    );
    Ok(())
}
