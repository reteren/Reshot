//! Turns a selection of the composited frame into output pixels (SPEC §11). The selection can
//! be any shape: pixels outside the shape become transparent, which the PNG keeps, on the
//! clipboard as well as on disk.

use skia_safe::{
    AlphaType, Bitmap, ColorType, EncodedImageFormat, IRect, ImageInfo, Matrix, Path, images,
    jpeg_encoder, png_encoder,
};

use crate::document::{self, CaptureDocument};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpg,
    Webp,
}

impl ImageFormat {
    /// From the settings value or a file extension; unknown means PNG.
    pub fn parse(s: &str) -> Self {
        match s.trim().trim_start_matches('.').to_lowercase().as_str() {
            "jpg" | "jpeg" => ImageFormat::Jpg,
            "webp" => ImageFormat::Webp,
            _ => ImageFormat::Png,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }
}

/// Premultiplied tight BGRA pixels that can cross threads (skia bitmaps cannot).
#[derive(Clone)]
pub struct Raster {
    pub width: i32,
    pub height: i32,
    pub bgra: Vec<u8>,
}

impl Raster {
    pub fn from_bitmap(b: &Bitmap) -> Self {
        let (w, h) = (b.width(), b.height());
        let row = w as usize * 4;
        let rb = b.row_bytes();
        let src = document::pixels(b);
        let mut bgra = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            bgra.extend_from_slice(&src[y * rb..y * rb + row]);
        }
        Self {
            width: w,
            height: h,
            bgra,
        }
    }

    pub fn to_bitmap(&self) -> Bitmap {
        bitmap_from_bgra(self.width, self.height, self.width as usize * 4, &self.bgra)
    }
}

/// Wraps a tightly or loosely packed BGRA frame as a premultiplied skia bitmap (copying it).
pub fn bitmap_from_bgra(width: i32, height: i32, stride: usize, bgra: &[u8]) -> Bitmap {
    let mut b = document::new_bitmap(width, height);
    let rb = b.row_bytes();
    let row = width as usize * 4;
    let dst = document::pixels_mut(&mut b);
    for y in 0..height as usize {
        dst[y * rb..y * rb + row].copy_from_slice(&bgra[y * stride..y * stride + row]);
    }
    b
}

/// Crops `region` out of base + document, keeping only what lies inside `shape` (given in
/// frame coordinates); everything else is transparent. `shape = None` keeps the whole rect.
pub fn crop_masked(
    base: &Bitmap,
    doc: &CaptureDocument,
    region: IRect,
    shape: Option<&Path>,
) -> Bitmap {
    // Composited at the region's size, then drawn once through the shape, so the anti-aliased
    // edge of the shape is applied to the finished pixels exactly once.
    let composite = doc.render_composite_region(base, region);
    let Some(p) = shape else { return composite };
    let mut out = document::new_bitmap(region.width().max(1), region.height().max(1));
    document::with_canvas(&mut out, |c| {
        c.translate((-region.left as f32, -region.top as f32));
        c.clip_path(p, None, true);
        c.draw_image(composite.as_image(), (region.left, region.top), None);
    });
    out
}

/// Encodes a bitmap. `quality` (1..100) applies to JPG and WebP. JPG has no alpha, so
/// transparent areas are flattened onto black, as every JPEG encoder does.
pub fn encode(bitmap: &Bitmap, format: ImageFormat, quality: i32) -> Option<Vec<u8>> {
    let quality = quality.clamp(1, 100) as u32;
    let pixmap = bitmap.pixmap();
    let mut data = Vec::new();
    let ok = match format {
        ImageFormat::Png => {
            png_encoder::encode(pixmap, &mut data, &png_encoder::Options::default())
        }
        ImageFormat::Jpg => jpeg_encoder::encode(
            pixmap,
            &mut data,
            &jpeg_encoder::Options {
                quality,
                ..Default::default()
            },
        ),
        // The prebuilt Skia ships without its WebP encoder, so libwebp does it: lossy at the
        // configured quality, alpha kept, as the C# build produced.
        ImageFormat::Webp => {
            let mut rgba = to_unpremul_bgra(bitmap);
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            let encoded =
                webp::Encoder::from_rgba(&rgba, bitmap.width() as u32, bitmap.height() as u32)
                    .encode(quality as f32);
            data.extend_from_slice(&encoded);
            true
        }
    };
    ok.then_some(data)
}

/// Unpremultiplied tight BGRA copy, for consumers that do not understand premultiplied alpha.
pub fn to_unpremul_bgra(bitmap: &Bitmap) -> Vec<u8> {
    let info = ImageInfo::new(
        bitmap.dimensions(),
        ColorType::BGRA8888,
        AlphaType::Unpremul,
        None,
    );
    let mut out = vec![0u8; bitmap.width() as usize * bitmap.height() as usize * 4];
    let image = images::raster_from_bitmap(bitmap).expect("bitmap has pixels");
    image.read_pixels(
        &info,
        &mut out,
        info.min_row_bytes(),
        (0, 0),
        skia_safe::image::CachingHint::Allow,
    );
    out
}

/// A path scaled and moved, for converting a selection between coordinate spaces.
pub fn transformed(path: &Path, m: &Matrix) -> Path {
    path.make_transform(m)
}

pub fn encoded_format(format: ImageFormat) -> EncodedImageFormat {
    match format {
        ImageFormat::Png => EncodedImageFormat::PNG,
        ImageFormat::Jpg => EncodedImageFormat::JPEG,
        ImageFormat::Webp => EncodedImageFormat::WEBP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::{Color, Rect};

    #[test]
    fn crop_outside_the_shape_is_transparent() {
        let base = document::new_bitmap(40, 40);
        base.erase_color(Color::RED);
        let doc = CaptureDocument::new(40, 40);
        let oval = Path::oval(Rect::from_ltrb(10.0, 10.0, 30.0, 30.0), None);
        let out = crop_masked(&base, &doc, IRect::from_ltrb(10, 10, 30, 30), Some(&oval));
        let px = |x: usize, y: usize| {
            let i = y * out.row_bytes() + x * 4;
            document::pixels(&out)[i + 3]
        };
        assert_eq!((out.width(), out.height()), (20, 20));
        assert_eq!(px(10, 10), 255);
        assert_eq!(px(0, 0), 0);
    }

    #[test]
    fn encodes_every_format() {
        let b = document::new_bitmap(8, 8);
        b.erase_color(Color::BLUE);
        let png = encode(&b, ImageFormat::Png, 90).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let jpg = encode(&b, ImageFormat::Jpg, 90).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        let webp = encode(&b, ImageFormat::Webp, 90).unwrap();
        assert_eq!(&webp[8..12], b"WEBP");
    }

    #[test]
    fn format_parsing() {
        assert_eq!(ImageFormat::parse(".JPEG"), ImageFormat::Jpg);
        assert_eq!(ImageFormat::parse("webp"), ImageFormat::Webp);
        assert_eq!(ImageFormat::parse("bmp"), ImageFormat::Png);
    }
}
