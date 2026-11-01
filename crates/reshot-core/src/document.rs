//! The editable state of a capture session (ARCHITECTURE §3). Layers, bottom to top:
//!
//! - **effects**: blur and pixelation over the untouched base, transparent where untouched;
//! - **paint**: brush strokes plus baked shapes and text;
//! - **absolute mask**: coverage of the Absolute Eraser, punched out of the whole composite
//!   (base included) on export.
//!
//! The base frame itself is owned by the caller and never modified, which is what lets the
//! Filter Eraser return the original pixels. Everything is in physical pixels.

use skia_safe::{
    AlphaType, Bitmap, BlendMode, Canvas, Color, ColorFilter, ColorType, IRect, ImageInfo, Paint,
    Path, color_filters,
};

use crate::tools::VectorObject;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    Paint,
    Effects,
    Absolute,
}

pub struct CaptureDocument {
    width: i32,
    height: i32,
    paint: Option<Bitmap>,
    effects: Option<Bitmap>,
    absolute: Option<Bitmap>,
    pub has_paint: bool,
    pub has_effects: bool,
    pub has_absolute: bool,
}

/// A frame-sized premultiplied BGRA bitmap, cleared to transparent.
pub fn new_bitmap(width: i32, height: i32) -> Bitmap {
    let mut b = Bitmap::new();
    let info = ImageInfo::new(
        (width.max(1), height.max(1)),
        ColorType::BGRA8888,
        AlphaType::Premul,
        None,
    );
    assert!(
        b.set_info(&info, None),
        "invalid bitmap size {width}x{height}"
    );
    b.alloc_pixels();
    b.erase_color(Color::TRANSPARENT);
    b
}

/// Runs `f` with a canvas drawing into `bitmap`.
pub fn with_canvas<R>(bitmap: &mut Bitmap, f: impl FnOnce(&Canvas) -> R) -> R {
    let canvas = Canvas::from_bitmap(bitmap, None).expect("bitmap is not drawable");
    f(&canvas)
}

fn clip(canvas: &Canvas, selection: Option<&Path>) {
    if let Some(sel) = selection {
        canvas.clip_path(sel, None, true);
    }
}

impl CaptureDocument {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            paint: None,
            effects: None,
            absolute: None,
            has_paint: false,
            has_effects: false,
            has_absolute: false,
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn has_content(&self) -> bool {
        self.has_paint || self.has_effects || self.has_absolute
    }

    /// The layer's bitmap, allocated on first use.
    pub fn layer_mut(&mut self, layer: Layer) -> &mut Bitmap {
        let (w, h) = (self.width, self.height);
        let slot = match layer {
            Layer::Paint => &mut self.paint,
            Layer::Effects => &mut self.effects,
            Layer::Absolute => &mut self.absolute,
        };
        slot.get_or_insert_with(|| new_bitmap(w, h))
    }

    /// The layer if it was ever drawn into.
    pub fn layer(&self, layer: Layer) -> Option<&Bitmap> {
        match layer {
            Layer::Paint => self.paint.as_ref(),
            Layer::Effects => self.effects.as_ref(),
            Layer::Absolute => self.absolute.as_ref(),
        }
    }

    fn draw(&mut self, layer: Layer, selection: Option<&Path>, f: impl FnOnce(&Canvas)) {
        with_canvas(self.layer_mut(layer), |c| {
            c.save();
            clip(c, selection);
            f(c);
            c.restore();
        });
    }

    // ---- Brush and vectors ------------------------------------------------------------

    pub fn commit_stroke(&mut self, stroke: &Path, paint: &Paint, selection: Option<&Path>) {
        self.draw(Layer::Paint, selection, |c| {
            c.draw_path(stroke, paint);
        });
        self.has_paint = true;
    }

    /// Bakes a finished shape or text block into the paint layer, so the eraser treats it
    /// exactly like a brush stroke: pixel by pixel, never as a whole object.
    pub fn commit_vector(&mut self, v: &VectorObject, selection: Option<&Path>) {
        self.draw(Layer::Paint, selection, |c| v.draw(c));
        self.has_paint = true;
    }

    /// Bakes a soft brush stroke: its greyscale coverage, tinted with the brush colour and
    /// applied at the stroke's opacity in one go, so the stroke never builds up over itself.
    pub fn paint_coverage(
        &mut self,
        coverage: &Bitmap,
        dx: i32,
        dy: i32,
        color: Color,
        opacity: f32,
        selection: Option<&Path>,
    ) {
        let image = coverage.as_image();
        self.draw(Layer::Paint, selection, |c| {
            c.draw_image(&image, (dx, dy), Some(&tint_paint(color, opacity)));
        });
        self.has_paint = true;
    }

    // ---- Effects and hard erasers -----------------------------------------------------

    /// Stamps effect-source pixels through a soft coverage: full strength in the brush's core,
    /// fading over its rim into whatever was there before.
    pub fn apply_effect_coverage(
        &mut self,
        source: &Bitmap,
        coverage: &Bitmap,
        dx: i32,
        dy: i32,
        selection: Option<&Path>,
    ) {
        let (src, cov) = (source.as_image(), coverage.as_image());
        self.draw(Layer::Effects, selection, |c| draw_through_coverage(c, &src, &cov, dx, dy));
        self.has_effects = true;
    }

    /// Stamps effect-source pixels (the blurred or pixelated base) into `area`.
    pub fn apply_effect(&mut self, source: &Bitmap, area: &Path, selection: Option<&Path>) {
        let image = source.as_image();
        self.draw(Layer::Effects, selection, |c| {
            c.clip_path(area, None, true);
            c.draw_image(&image, (0, 0), None);
        });
        self.has_effects = true;
    }

    /// Filter Eraser: clears the effects in `area`, returning the original pixels.
    pub fn erase_effects(&mut self, area: &Path, selection: Option<&Path>) {
        self.clear_area(Layer::Effects, area, selection);
    }

    /// Eraser: clears painted pixels in `area`.
    pub fn erase_paint(&mut self, area: &Path, selection: Option<&Path>) {
        self.clear_area(Layer::Paint, area, selection);
    }

    /// Absolute Eraser: marks `area` to be punched to transparency on export.
    pub fn absolute_erase(&mut self, area: &Path, selection: Option<&Path>) {
        self.draw(Layer::Absolute, selection, |c| {
            c.clip_path(area, None, true);
            let mut p = Paint::default();
            p.set_color(Color::WHITE);
            // draw_paint respects the clip; clear() would not.
            c.draw_paint(&p);
        });
        self.has_absolute = true;
    }

    fn clear_area(&mut self, layer: Layer, area: &Path, selection: Option<&Path>) {
        self.draw(layer, selection, |c| {
            c.clip_path(area, None, true);
            let mut p = Paint::default();
            p.set_blend_mode(BlendMode::Clear);
            c.draw_paint(&p);
        });
    }

    // ---- Soft (Photoshop-style) erasers -------------------------------------------------
    //
    // A coverage bitmap is an opaque greyscale image whose luminance is the per-pixel erase
    // strength (0 = keep, 1 = erase fully), with the eraser's opacity already baked in. The
    // coverage alpha filter turns that luminance into a pure alpha mask, so a single DstOut
    // removes exactly that fraction of the layer with no compounding along the stroke.

    /// Eraser: removes painted pixels by the coverage strength.
    pub fn erase_paint_coverage(
        &mut self,
        coverage: &Bitmap,
        dx: i32,
        dy: i32,
        selection: Option<&Path>,
    ) {
        self.apply_coverage(Layer::Paint, coverage, dx, dy, selection, BlendMode::DstOut);
    }

    /// Filter Eraser: removes effect pixels by the coverage strength.
    pub fn erase_effects_coverage(
        &mut self,
        coverage: &Bitmap,
        dx: i32,
        dy: i32,
        selection: Option<&Path>,
    ) {
        self.apply_coverage(
            Layer::Effects,
            coverage,
            dx,
            dy,
            selection,
            BlendMode::DstOut,
        );
    }

    /// Absolute Eraser: accumulates coverage into the punch-through mask.
    pub fn absolute_erase_coverage(
        &mut self,
        coverage: &Bitmap,
        dx: i32,
        dy: i32,
        selection: Option<&Path>,
    ) {
        self.apply_coverage(
            Layer::Absolute,
            coverage,
            dx,
            dy,
            selection,
            BlendMode::SrcOver,
        );
        self.has_absolute = true;
    }

    fn apply_coverage(
        &mut self,
        layer: Layer,
        coverage: &Bitmap,
        dx: i32,
        dy: i32,
        selection: Option<&Path>,
        blend: BlendMode,
    ) {
        let image = coverage.as_image();
        self.draw(layer, selection, |c| {
            let mut p = Paint::default();
            p.set_blend_mode(blend);
            p.set_color_filter(coverage_alpha_filter());
            c.draw_image(&image, (dx, dy), Some(&p));
        });
    }

    // ---- Composition ------------------------------------------------------------------

    /// Full export composite: base, effects, paint, then the Absolute Eraser punches the
    /// whole thing, base included, to transparency.
    pub fn render_composite(&self, base: &Bitmap) -> Bitmap {
        self.render_composite_region(base, IRect::from_wh(self.width, self.height))
    }

    /// [`Self::render_composite`] for `region` alone, at the region's size: an export of a
    /// small selection on a 4480x1440 desktop composites 25 MB less per pass.
    pub fn render_composite_region(&self, base: &Bitmap, region: IRect) -> Bitmap {
        let mut out = new_bitmap(region.width().max(1), region.height().max(1));
        with_canvas(&mut out, |c| {
            c.translate((-region.left as f32, -region.top as f32));
            c.draw_image(base.as_image(), (0, 0), None);
            self.draw_overlay(c);
            if self.has_absolute
                && let Some(mask) = &self.absolute
            {
                let mut punch = Paint::default();
                punch.set_blend_mode(BlendMode::DstOut);
                c.draw_image(mask.as_image(), (0, 0), Some(&punch));
            }
        });
        out
    }

    /// Effects and paint composited into one frame-sized bitmap, transparent where empty.
    pub fn render_overlay(&self) -> Bitmap {
        let mut out = new_bitmap(self.width, self.height);
        with_canvas(&mut out, |c| self.draw_overlay(c));
        out
    }

    /// Draws effects then paint onto `canvas` at the origin.
    pub fn draw_overlay(&self, canvas: &Canvas) {
        for layer in [&self.effects, &self.paint].into_iter().flatten() {
            canvas.draw_image(layer.as_image(), (0, 0), None);
        }
    }
}

/// Paint that draws a greyscale coverage image as `color` at `opacity`: the coverage becomes
/// alpha, the colour fills it.
pub fn tint_paint(color: Color, opacity: f32) -> Paint {
    // The opacity rides in the fill colour: a paint alpha would be applied to the coverage
    // before the alpha filter, which reads red alone and would drop it.
    let mut p = Paint::default();
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    let fill = color_filters::blend(color.with_a(a), BlendMode::SrcIn);
    p.set_color_filter(fill.and_then(|f| color_filters::compose(f, coverage_alpha_filter())));
    p
}

/// Draws `source` (frame-sized, at the origin) only where `coverage` (placed at `dx, dy`) says,
/// by its strength, over whatever the canvas already holds.
pub fn draw_through_coverage(canvas: &Canvas, source: &skia_safe::Image, coverage: &skia_safe::Image, dx: i32, dy: i32) {
    let bounds = skia_safe::Rect::from_xywh(dx as f32, dy as f32, coverage.width() as f32, coverage.height() as f32);
    canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default().bounds(&bounds));
    canvas.draw_image(source, (0, 0), None);
    let mut keep = Paint::default();
    keep.set_blend_mode(BlendMode::DstIn);
    keep.set_color_filter(coverage_alpha_filter());
    canvas.draw_image(coverage, (dx, dy), Some(&keep));
    canvas.restore();
}

/// Maps a greyscale coverage bitmap to a pure alpha mask: alpha = red, rgb = 0.
pub fn coverage_alpha_filter() -> ColorFilter {
    #[rustfmt::skip]
    let m = [
        0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0,
        1.0, 0.0, 0.0, 0.0, 0.0,
    ];
    color_filters::matrix_row_major(&m, None)
}

// ---- Raw pixel access and region snapshots (undo) -------------------------------------

/// The bitmap's pixel bytes, `row_bytes * height` long.
pub fn pixels(bitmap: &Bitmap) -> &[u8] {
    let len = bitmap.row_bytes() * bitmap.height() as usize;
    // SAFETY: the bitmap owns `row_bytes * height` bytes of allocated pixels for its lifetime.
    unsafe { std::slice::from_raw_parts(bitmap.pixmap().addr() as *const u8, len) }
}

pub fn pixels_mut(bitmap: &mut Bitmap) -> &mut [u8] {
    let len = bitmap.row_bytes() * bitmap.height() as usize;
    // SAFETY: as above, and the &mut borrow guarantees exclusive access.
    unsafe { std::slice::from_raw_parts_mut(bitmap.pixels() as *mut u8, len) }
}

/// `region` clamped to the bitmap; `None` when nothing is left.
pub fn clip_region(bitmap: &Bitmap, region: IRect) -> Option<IRect> {
    let r = IRect::from_ltrb(
        region.left.clamp(0, bitmap.width()),
        region.top.clamp(0, bitmap.height()),
        region.right.clamp(0, bitmap.width()),
        region.bottom.clamp(0, bitmap.height()),
    );
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// Copies a rectangular region of a layer. Parts outside the layer come back transparent,
/// and the snapshot is never smaller than 1x1.
pub fn snapshot_region(layer: &Bitmap, region: IRect) -> Bitmap {
    let clipped = clip_region(layer, region);
    let (w, h) = clipped.map_or((1, 1), |r| (r.width().max(1), r.height().max(1)));
    let mut snap = new_bitmap(w, h);
    if let Some(r) = clipped {
        let (src_rb, dst_rb) = (layer.row_bytes(), snap.row_bytes());
        let row_len = r.width() as usize * 4;
        let src = pixels(layer);
        let dst = pixels_mut(&mut snap);
        for y in 0..r.height() as usize {
            let s = (r.top as usize + y) * src_rb + r.left as usize * 4;
            let d = y * dst_rb;
            dst[d..d + row_len].copy_from_slice(&src[s..s + row_len]);
        }
    }
    snap
}

/// Overwrites a region of a layer with a snapshot taken by [`snapshot_region`].
pub fn restore_region(layer: &mut Bitmap, region: IRect, snapshot: &Bitmap) {
    let Some(clipped) = clip_region(layer, region) else {
        return;
    };
    let sx = if snapshot.width() == region.width() {
        clipped.left - region.left
    } else {
        0
    };
    let sy = if snapshot.height() == region.height() {
        clipped.top - region.top
    } else {
        0
    };
    let w = clipped.width().min(snapshot.width() - sx);
    let h = clipped.height().min(snapshot.height() - sy);
    if w <= 0 || h <= 0 {
        return;
    }
    let (src_rb, dst_rb) = (snapshot.row_bytes(), layer.row_bytes());
    let row_len = w as usize * 4;
    let src = pixels(snapshot);
    let dst = pixels_mut(layer);
    for y in 0..h as usize {
        let s = (sy as usize + y) * src_rb + sx as usize * 4;
        let d = (clipped.top as usize + y) * dst_rb + clipped.left as usize * 4;
        dst[d..d + row_len].copy_from_slice(&src[s..s + row_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::stroke_paint;

    fn px(b: &Bitmap, x: i32, y: i32) -> [u8; 4] {
        let i = y as usize * b.row_bytes() + x as usize * 4;
        pixels(b)[i..i + 4].try_into().unwrap()
    }

    fn solid_base(w: i32, h: i32, c: Color) -> Bitmap {
        let b = new_bitmap(w, h);
        b.erase_color(c);
        b
    }

    fn rect_path(l: f32, t: f32, r: f32, b: f32) -> Path {
        Path::rect(skia_safe::Rect::from_ltrb(l, t, r, b), None)
    }

    #[test]
    fn new_layers_are_transparent_and_lazy() {
        let mut d = CaptureDocument::new(16, 16);
        assert!(d.layer(Layer::Paint).is_none());
        let p = d.layer_mut(Layer::Paint);
        assert_eq!(px(p, 3, 3), [0, 0, 0, 0]);
        assert!(!d.has_content());
    }

    #[test]
    fn composite_stacks_effects_and_paint_over_the_base() {
        let base = solid_base(20, 20, Color::BLUE);
        let mut d = CaptureDocument::new(20, 20);
        let mut red = stroke_paint(Color::RED, 4.0);
        red.set_style(skia_safe::PaintStyle::Fill);
        d.commit_stroke(&rect_path(0.0, 0.0, 10.0, 20.0), &red, None);
        let out = d.render_composite(&base);
        assert_eq!(px(&out, 2, 10), [0, 0, 255, 255]); // BGRA red
        assert_eq!(px(&out, 15, 10), [255, 0, 0, 255]); // base blue
    }

    #[test]
    fn a_region_composite_matches_the_same_pixels_of_the_full_one() {
        let base = solid_base(30, 20, Color::BLUE);
        let mut d = CaptureDocument::new(30, 20);
        let mut red = stroke_paint(Color::RED, 3.0);
        red.set_anti_alias(true);
        d.commit_stroke(&rect_path(4.5, 3.5, 22.5, 15.5), &red, None);
        d.absolute_erase(&rect_path(18.0, 0.0, 30.0, 6.0), None);
        let full = d.render_composite(&base);
        // Partly outside the frame on the left, as a selection dragged past the edge can be.
        let region = IRect::from_ltrb(-3, 2, 25, 18);
        let part = d.render_composite_region(&base, region);
        for y in 0..region.height() {
            for x in 0..region.width() {
                let (fx, fy) = (x + region.left, y + region.top);
                let want = if fx < 0 { [0, 0, 0, 0] } else { px(&full, fx, fy) };
                assert_eq!(px(&part, x, y), want, "at {fx},{fy}");
            }
        }
    }

    #[test]
    fn selection_clip_limits_drawing() {
        let mut d = CaptureDocument::new(20, 20);
        let mut red = Paint::default();
        red.set_color(Color::RED);
        d.commit_stroke(
            &rect_path(0.0, 0.0, 20.0, 20.0),
            &red,
            Some(&rect_path(5.0, 5.0, 10.0, 10.0)),
        );
        let p = d.layer(Layer::Paint).unwrap();
        assert_eq!(px(p, 7, 7)[3], 255);
        assert_eq!(px(p, 15, 15)[3], 0);
    }

    #[test]
    fn absolute_erase_punches_the_base_to_transparent() {
        let base = solid_base(10, 10, Color::GREEN);
        let mut d = CaptureDocument::new(10, 10);
        d.absolute_erase(&rect_path(0.0, 0.0, 5.0, 10.0), None);
        let out = d.render_composite(&base);
        assert_eq!(px(&out, 2, 5)[3], 0);
        assert_eq!(px(&out, 7, 5)[3], 255);
    }

    #[test]
    fn filter_eraser_restores_the_original_under_effects() {
        let base = solid_base(10, 10, Color::GREEN);
        let fx = solid_base(10, 10, Color::RED);
        let mut d = CaptureDocument::new(10, 10);
        let all = rect_path(0.0, 0.0, 10.0, 10.0);
        d.apply_effect(&fx, &all, None);
        d.erase_effects(&rect_path(0.0, 0.0, 5.0, 10.0), None);
        let out = d.render_composite(&base);
        assert_eq!(px(&out, 2, 5), [0, 255, 0, 255]);
        assert_eq!(px(&out, 7, 5), [0, 0, 255, 255]);
    }

    #[test]
    fn soft_coverage_removes_only_its_fraction() {
        let mut d = CaptureDocument::new(4, 4);
        let mut white = Paint::default();
        white.set_color(Color::WHITE);
        d.commit_stroke(&rect_path(0.0, 0.0, 4.0, 4.0), &white, None);
        // 50% grey coverage removes half the alpha.
        let cov = solid_base(4, 4, Color::from_rgb(128, 128, 128));
        d.erase_paint_coverage(&cov, 0, 0, None);
        let a = px(d.layer(Layer::Paint).unwrap(), 1, 1)[3];
        assert!((125..=129).contains(&a), "alpha {a}");
    }

    #[test]
    fn snapshot_and_restore_roundtrip_with_clipping() {
        let mut layer = solid_base(10, 10, Color::RED);
        let region = IRect::from_ltrb(-2, -2, 4, 4);
        let snap = snapshot_region(&layer, region);
        assert_eq!((snap.width(), snap.height()), (4, 4));
        layer.erase_color(Color::BLUE);
        restore_region(&mut layer, region, &snap);
        assert_eq!(px(&layer, 1, 1), [0, 0, 255, 255]);
        assert_eq!(px(&layer, 6, 6), [255, 0, 0, 255]);
    }

    #[test]
    fn snapshot_outside_is_a_transparent_pixel() {
        let layer = solid_base(4, 4, Color::RED);
        let snap = snapshot_region(&layer, IRect::from_ltrb(10, 10, 20, 20));
        assert_eq!((snap.width(), snap.height()), (1, 1));
        assert_eq!(px(&snap, 0, 0), [0, 0, 0, 0]);
    }
}
