//! Turning pixel buffers into gpui images. gpui keeps images as premultiplied BGRA, which is
//! exactly what the capture and the skia layers produce, so this is a repack, not a convert.

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

pub fn render_image(frame: &CapturedFrame) -> Arc<RenderImage> {
    from_bgra(frame.width, frame.height, frame.stride, &frame.pixels_bgra)
}
