//! Turning pixel buffers into gpui images. gpui stores BGRA, but on Windows it blends image
//! sprites as straight alpha (SRC_ALPHA, INV_SRC_ALPHA). The capture is opaque, so it is a
//! plain repack; Skia layers are premultiplied and go through [`from_premultiplied_bgra`].

use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, RgbaImage};
use reshot_capture::CapturedFrame;
use smallvec::SmallVec;

pub fn from_bgra(width: u32, height: u32, stride: usize, bgra: &[u8]) -> Arc<RenderImage> {
    let row = width as usize * 4;
    let buf = if stride == row {
        bgra[..row * height as usize].to_vec()
    } else {
        let mut v = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            v.extend_from_slice(&bgra[y * stride..y * stride + row]);
        }
        v
    };
    let image = RgbaImage::from_raw(width, height, buf).expect("buffer matches the image size");
    Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(image), 1)))
}

/// [`from_bgra`] for premultiplied pixels. Handed over as they are, every partly
/// transparent pixel would be multiplied by its alpha a second time when blended: the
/// anti-aliased rim of a blur or a stroke came out darker than either side of it, a thin dark
/// outline around every effect.
pub fn from_premultiplied_bgra(width: u32, height: u32, stride: usize, bgra: &[u8]) -> Arc<RenderImage> {
    let row = width as usize * 4;
    let mut buf = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        buf.extend_from_slice(&bgra[y * stride..y * stride + row]);
    }
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let image = RgbaImage::from_raw(width, height, buf).expect("buffer matches the image size");
    Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(image), 1)))
}

/// Takes the frame's pixels over when its rows are packed, as a capture's always are: the
/// frame is dropped right after, and copying a 4480x1440 desktop once more costs a 25 MB
/// allocation and several milliseconds before the overlay can appear.
pub fn render_image(frame: &mut CapturedFrame) -> Arc<RenderImage> {
    let (width, height) = (frame.width, frame.height);
    let len = width as usize * 4 * height as usize;
    if frame.stride != width as usize * 4 || frame.pixels_bgra.len() < len {
        return from_bgra(width, height, frame.stride, &frame.pixels_bgra);
    }
    let mut buf = std::mem::take(&mut frame.pixels_bgra);
    buf.truncate(len);
    let image = RgbaImage::from_raw(width, height, buf).expect("buffer matches the image size");
    Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(image), 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_pixels_reach_gpui_straight() {
        // Half-transparent mid grey, premultiplied: 64 of 255 at alpha 128.
        let premul = [64u8, 64, 64, 128, 0, 0, 0, 0, 10, 20, 30, 255];
        let image = from_premultiplied_bgra(3, 1, 12, &premul);
        let bytes = image.as_bytes(0).expect("one frame");
        assert_eq!(&bytes[..4], &[128, 128, 128, 128], "straight colour, alpha kept");
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0], "fully transparent stays untouched");
        assert_eq!(&bytes[8..], &[10, 20, 30, 255], "opaque pixels are already straight");
    }
}
