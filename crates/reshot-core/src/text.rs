//! Retained text annotations. Erasures travel with the annotation when it is moved;
//! the frozen screenshot and raster ink underneath are never restored or overwritten.

use skia_safe::{BlendMode, Canvas, Contains, Image, Paint, Path, Point, Rect};

use crate::{document, tools::VectorObject};

#[derive(Clone)]
struct Erasure {
    coverage: Image,
    offset: Point,
}

#[derive(Clone)]
pub struct TextAnnotation {
    pub id: u64,
    pub object: VectorObject,
    pub clip: Option<Path>,
    erasures: Vec<Erasure>,
}

impl TextAnnotation {
    pub fn new(id: u64, object: VectorObject, clip: Option<Path>) -> Self {
        Self { id, object, clip, erasures: Vec::new() }
    }

    pub fn bounds(&self) -> Rect { self.object.compute_bounds() }

    pub fn contains(&self, point: Point) -> bool {
        self.bounds().contains(point) && self.clip.as_ref().is_none_or(|clip| clip.contains(point))
    }

    pub fn draw(&self, canvas: &Canvas) {
        canvas.save();
        if let Some(clip) = &self.clip { canvas.clip_path(clip, None, true); }
        if !self.erasures.is_empty() {
            canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default());
        }
        self.object.draw(canvas);
        for erasure in &self.erasures {
            let mut paint = Paint::default();
            paint.set_blend_mode(BlendMode::DstOut);
            paint.set_color_filter(document::coverage_alpha_filter());
            canvas.draw_image(&erasure.coverage,
                (self.object.p1.x + erasure.offset.x, self.object.p1.y + erasure.offset.y), Some(&paint));
        }
        if !self.erasures.is_empty() { canvas.restore(); }
        canvas.restore();
    }

    pub(crate) fn erase(&mut self, coverage: &Image, origin: Point) {
        self.erasures.push(Erasure {
            coverage: coverage.clone(),
            offset: Point::new(origin.x - self.object.p1.x, origin.y - self.object.p1.y),
        });
    }
}
