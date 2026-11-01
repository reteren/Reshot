//! Vector chrome painted by the GPU every frame: the dim with a hole for every selection,
//! the selection outlines, the polygon being drawn, the resize handles and the OCR word boxes.
//! All of it is cheap to redraw, so none of it goes through the raster surface.

use gpui::{
    BorderStyle, Bounds, Edges, FillOptions, FillRule, PathBuilder, PathStyle, Pixels, Point, Rgba, Window, point,
    px, quad, rgba, size,
};
use reshot_core::selection::{Handle, Selection};
use reshot_core::skia::{Point as SkPoint, Rect};

use crate::theme;

/// Everything the chrome needs, copied out of the overlay so painting can happen in a
/// `'static` canvas closure.
pub struct ChromeScene {
    pub scale: f32,
    pub size: (f32, f32),
    pub dim: Rgba,
    pub outlines: Vec<Vec<SkPoint>>,
    /// The selected region as closed polygons, holes included: what the dim leaves out.
    pub region: Vec<Vec<SkPoint>>,
    pub polygon: Option<(Vec<SkPoint>, SkPoint)>,
    pub handles: Option<Rect>,
    pub handle_size: f32,
    pub ocr_words: Vec<(Rect, bool)>,
}

/// The outline of a selection as a closed polyline in frame pixels.
pub fn outline(s: &Selection) -> Vec<SkPoint> {
    s.polygon()
}

impl ChromeScene {
    fn p(&self, q: SkPoint) -> Point<Pixels> {
        point(px(q.x / self.scale), px(q.y / self.scale))
    }

    pub fn paint(&self, origin: Point<Pixels>, window: &mut Window) {
        let (w, h) = (px(self.size.0 / self.scale), px(self.size.1 / self.scale));
        let at = |q: Point<Pixels>| point(origin.x + q.x, origin.y + q.y);

        // Dim: the whole frame with the selected region cut out (holes from cut-out
        // selections stay dim). While a polygon is being drawn the whole screen stays dim.
        let mut dim = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::default().with_fill_rule(FillRule::EvenOdd)));
        dim.move_to(at(point(px(0.), px(0.))));
        dim.line_to(at(point(w, px(0.))));
        dim.line_to(at(point(w, h)));
        dim.line_to(at(point(px(0.), h)));
        dim.close();
        if self.polygon.is_none() {
            for o in &self.region {
                let pts: Vec<_> = o.iter().map(|q| at(self.p(*q))).collect();
                dim.add_polygon(&pts, true);
            }
        }
        if let Ok(path) = dim.build() {
            window.paint_path(path, self.dim);
        }

        let accent = rgba(0x3C9898FF);
        if self.polygon.is_none() {
            for o in &self.outlines {
                let mut pb = PathBuilder::stroke(px(1.5));
                let pts: Vec<_> = o.iter().map(|q| at(self.p(*q))).collect();
                pb.add_polygon(&pts, true);
                if let Ok(path) = pb.build() {
                    window.paint_path(path, accent);
                }
            }
        }

        // The polygon in progress: committed edges plus a rubber line to the cursor, dashed.
        if let Some((pts, cursor)) = &self.polygon
            && !pts.is_empty()
        {
            let mut pb = PathBuilder::stroke(px(1.5)).dash_array(&[px(4.0 * 1.5), px(3.0 * 1.5)]);
            let mut all: Vec<_> = pts.iter().map(|q| at(self.p(*q))).collect();
            all.push(at(self.p(*cursor)));
            pb.add_polygon(&all, false);
            if let Ok(path) = pb.build() {
                window.paint_path(path, accent);
            }
        }

        // OCR: a thin outline per line, a bold one over the selected span. Outlines rather
        // than fills, so the letters stay readable.
        for (r, selected) in &self.ocr_words {
            let r = Rect::from_ltrb(r.left - 1.5, r.top - 1.5, r.right + 1.5, r.bottom + 1.5);
            let b = Bounds::new(at(self.p(SkPoint::new(r.left, r.top))), size(px(r.width() / self.scale), px(r.height() / self.scale)));
            let (width, color) = if *selected { (2.4, rgba(0x3C9898FF)) } else { (1.3, rgba(0x3C98986E)) };
            window.paint_quad(quad(
                b,
                px(0.),
                rgba(0x00000000),
                Edges::all(px(width / self.scale)),
                color,
                BorderStyle::Solid,
            ));
        }

        if let Some(sel) = self.handles {
            let s = px(self.handle_size);
            for h in Handle::ALL {
                let c = at(self.p(h.center(sel)));
                let b = Bounds::new(point(c.x - s / 2., c.y - s / 2.), size(s, s));
                window.paint_quad(quad(b, px(0.), theme::text(), Edges::all(px(1.5)), accent, BorderStyle::Solid));
            }
        }
    }
}
