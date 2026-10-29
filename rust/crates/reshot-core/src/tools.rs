//! Brush strokes and vector objects (SPEC §6). Pure Skia, no UI. Shapes and text are kept as
//! vectors only while being drawn; on commit they are baked into the paint layer.

use skia_safe::{
    Canvas, Color, Font, Paint, PaintCap, PaintJoin, PaintStyle, Path, PathBuilder, Point, Rect,
};

use crate::fonts;

/// Brush appearance (SPEC §6.1).
#[derive(Clone, Debug, PartialEq)]
pub struct BrushSettings {
    /// Opaque RGB; opacity is applied separately.
    pub color: Color,
    /// Stroke width in physical pixels.
    pub thickness: f32,
    /// 0..1, applied to the whole stroke so overlaps don't darken.
    pub opacity: f32,
    /// Eraser radial falloff (Photoshop hardness). 1 = the whole disc erases at full strength;
    /// lower values fade from 100% at the centre to this fraction at the rim. Erasers only.
    pub hardness: f32,
    /// Text tool font family. Empty or unknown resolves to the default.
    pub font_family: String,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            color: Color::from_rgb(0xFF, 0x3B, 0x30),
            thickness: 6.0,
            opacity: 1.0,
            hardness: 1.0,
            font_family: fonts::DEFAULT_FAMILY.into(),
        }
    }
}

/// One freehand stroke. Points accumulate into a single path so the stroke is painted exactly
/// once on commit and a semi-transparent stroke does not build up where it crosses itself.
pub struct BrushStroke {
    path: PathBuilder,
    started: bool,
    pub settings: BrushSettings,
}

impl BrushStroke {
    pub fn new(settings: BrushSettings) -> Self {
        Self {
            path: PathBuilder::new(),
            started: false,
            settings,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.started
    }

    pub fn begin(&mut self, p: Point) {
        self.path.move_to(p);
        self.started = true;
    }

    pub fn extend(&mut self, p: Point) {
        if self.started {
            self.path.line_to(p);
        }
    }

    /// The stroke so far, for live preview or commit.
    pub fn path(&self) -> Path {
        self.path.snapshot()
    }

    /// Round cap and join, anti-aliased.
    pub fn paint(&self) -> Paint {
        let a = (self.settings.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        stroke_paint(self.settings.color.with_a(a), self.settings.thickness)
    }
}

pub fn stroke_paint(color: Color, width: f32) -> Paint {
    let mut p = Paint::default();
    p.set_color(color);
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(width);
    p.set_stroke_cap(PaintCap::Round);
    p.set_stroke_join(PaintJoin::Round);
    p
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorKind {
    Circle,
    Square,
    Line,
    Triangle,
    Arrow,
    Text,
}

/// A shape or text block while it is being drawn. Shapes are defined by a bounding rect
/// (circle, square, triangle) or two endpoints (line, arrow); text by its top-left and string.
#[derive(Clone, Debug)]
pub struct VectorObject {
    pub kind: VectorKind,
    pub bounds: Rect,
    /// Line/arrow start, or the text's top-left.
    pub p1: Point,
    pub p2: Point,
    pub color: Color,
    pub thickness: f32,
    pub text: String,
    pub font_size: f32,
    pub font_family: String,
    /// Triangle apex up when dragged upward, down when dragged downward.
    pub points_up: bool,
}

impl Default for VectorObject {
    fn default() -> Self {
        Self {
            kind: VectorKind::Square,
            bounds: Rect::default(),
            p1: Point::default(),
            p2: Point::default(),
            color: Color::BLACK,
            thickness: 4.0,
            text: String::new(),
            font_size: 32.0,
            font_family: fonts::DEFAULT_FAMILY.into(),
            points_up: true,
        }
    }
}

impl VectorObject {
    pub fn draw(&self, canvas: &Canvas) {
        let stroke = stroke_paint(self.color, self.thickness);
        match self.kind {
            VectorKind::Circle => {
                canvas.draw_oval(self.bounds, &stroke);
            }
            VectorKind::Square => {
                canvas.draw_rect(self.bounds, &stroke);
            }
            VectorKind::Triangle => {
                canvas.draw_path(&self.triangle_path(), &stroke);
            }
            VectorKind::Line => {
                canvas.draw_line(self.p1, self.p2, &stroke);
            }
            VectorKind::Arrow => self.draw_arrow(canvas, &stroke),
            VectorKind::Text => self.draw_text(canvas),
        }
    }

    fn triangle_path(&self) -> Path {
        let b = self.bounds;
        let (apex, base) = if self.points_up {
            (b.top, b.bottom)
        } else {
            (b.bottom, b.top)
        };
        let mut path = PathBuilder::new();
        path.move_to((b.center_x(), apex))
            .line_to((b.right, base))
            .line_to((b.left, base))
            .close();
        path.detach()
    }

    fn draw_arrow(&self, canvas: &Canvas, stroke: &Paint) {
        canvas.draw_line(self.p1, self.p2, stroke);
        // Two barbs scaled to the line length, within sane bounds.
        let (dx, dy) = (self.p2.x - self.p1.x, self.p2.y - self.p1.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            return;
        }
        let head = (len * 0.25).clamp(self.thickness * 3.0, 40.0f32.max(self.thickness * 3.0));
        let angle = dy.atan2(dx);
        const SPREAD: f32 = 0.5;
        let barb = |a: f32| Point::new(self.p2.x - head * a.cos(), self.p2.y - head * a.sin());
        canvas.draw_line(self.p2, barb(angle - SPREAD), stroke);
        canvas.draw_line(self.p2, barb(angle + SPREAD), stroke);
    }

    fn font(&self) -> Font {
        Font::from_typeface(fonts::resolve(&self.font_family), self.font_size)
    }

    fn draw_text(&self, canvas: &Canvas) {
        if self.text.is_empty() {
            return;
        }
        let font = self.font();
        let mut paint = Paint::default();
        paint.set_color(self.color);
        paint.set_anti_alias(true);
        let (spacing, metrics) = font.metrics();
        // p1 is the top-left; the first baseline sits one ascent below it.
        let mut y = self.p1.y - metrics.ascent;
        for line in self.text.split('\n') {
            canvas.draw_str(line, (self.p1.x, y), &font, &paint);
            y += spacing;
        }
    }

    /// Axis-aligned bounds of the object: the rasterisation region and, for text, what the
    /// overlay draws its box from and hit-tests against.
    pub fn compute_bounds(&self) -> Rect {
        match self.kind {
            VectorKind::Line | VectorKind::Arrow => Rect::from_ltrb(
                self.p1.x.min(self.p2.x),
                self.p1.y.min(self.p2.y),
                self.p1.x.max(self.p2.x),
                self.p1.y.max(self.p2.y),
            ),
            VectorKind::Text => self.text_bounds(),
            _ => self.bounds,
        }
    }

    /// Measured with this object's own typeface and size, rather than estimated, because a
    /// condensed or monospaced family makes a per-glyph guess visibly disagree with the glyphs.
    fn text_bounds(&self) -> Rect {
        let font = self.font();
        let lines: Vec<&str> = self.text.split('\n').collect();
        let width = lines
            .iter()
            .map(|l| font.measure_str(l, None).0)
            .fold(0.0f32, f32::max);
        let (spacing, m) = font.metrics();
        let height = (lines.len() - 1) as f32 * spacing + (m.descent - m.ascent);
        // The floor keeps an empty or whitespace-only object grabbable.
        let floor = self.font_size * 0.25;
        Rect::from_xywh(self.p1.x, self.p1.y, width.max(floor), height.max(floor))
    }

    /// The baked stroke width of the shape, for padding the undo region.
    pub fn ink_padding(&self) -> f32 {
        match self.kind {
            VectorKind::Text => 2.0,
            VectorKind::Arrow => self.thickness * 2.0 + 2.0,
            _ => self.thickness / 2.0 + 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: Point = Point::new(120.0, 64.0);

    fn text(t: &str, size: f32, family: Option<&str>) -> VectorObject {
        VectorObject {
            kind: VectorKind::Text,
            text: t.into(),
            font_size: size,
            font_family: family.unwrap_or(fonts::DEFAULT_FAMILY).into(),
            p1: ORIGIN,
            color: Color::WHITE,
            ..Default::default()
        }
    }

    fn t(s: &str) -> Rect {
        text(s, 32.0, None).compute_bounds()
    }

    #[test]
    fn single_line_box_starts_exactly_at_p1() {
        let b = t("Reshot");
        assert!((b.left - ORIGIN.x).abs() < 1e-3);
        assert!((b.top - ORIGIN.y).abs() < 1e-3);
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }

    #[test]
    fn height_grows_with_the_line_count() {
        let one = t("Reshot").height();
        let two = t("Reshot\nReshot").height();
        let three = t("Reshot\nReshot\nReshot").height();
        assert!(two > one && three > two);
        let r = three / one;
        assert!((2.2..=3.8).contains(&r), "{r}");
    }

    #[test]
    fn width_follows_the_widest_line_not_the_last_one() {
        let wide = t("MMMMMMMMMMMM").width();
        let narrow = t("i").width();
        assert!(wide > narrow);
        for s in ["MMMMMMMMMMMM\ni", "i\nMMMMMMMMMMMM"] {
            let w = t(s).width();
            assert!(w >= wide * 0.99 && w <= wide * 1.01);
        }
    }

    #[test]
    fn doubling_the_font_size_roughly_doubles_the_box() {
        let small = text("Reshot", 24.0, None).compute_bounds();
        let large = text("Reshot", 48.0, None).compute_bounds();
        let (rw, rh) = (
            large.width() / small.width(),
            large.height() / small.height(),
        );
        assert!((1.8..=2.2).contains(&rw), "{rw}");
        assert!((1.8..=2.2).contains(&rh), "{rh}");
    }

    #[test]
    fn a_longer_string_is_wider() {
        assert!(t("Reshot capture overlay").width() > t("Re").width());
    }

    #[test]
    fn empty_and_whitespace_text_still_produce_a_grabbable_box() {
        for s in ["", " ", "   ", "\n"] {
            let b = t(s);
            assert!(b.width() > 0.0, "{s:?}");
            assert!(b.height() >= 16.0, "{s:?}: {}", b.height());
            assert!((b.left - ORIGIN.x).abs() < 1e-3 && (b.top - ORIGIN.y).abs() < 1e-3);
        }
    }

    #[test]
    fn different_families_measure_differently() {
        const SAMPLE: &str = "Reshot capture overlay";
        let baseline = t(SAMPLE).width();
        let installed: Vec<&str> = [
            "Courier New",
            "Times New Roman",
            "Impact",
            "Consolas",
            "Georgia",
        ]
        .into_iter()
        .filter(|c| {
            fonts::families()
                .iter()
                .any(|f| f.family.eq_ignore_ascii_case(c))
        })
        .collect();
        if installed.is_empty() {
            return;
        }
        assert!(installed.iter().any(|c| {
            (text(SAMPLE, 32.0, Some(c)).compute_bounds().width() - baseline).abs() > 0.5
        }));
    }

    #[test]
    fn an_unknown_or_blank_family_measures_as_the_default() {
        let known = text("Reshot", 32.0, Some(fonts::DEFAULT_FAMILY)).compute_bounds();
        let unknown = text("Reshot", 32.0, Some("NoSuchFontFamily_ZZZ")).compute_bounds();
        let blank = text("Reshot", 32.0, Some("")).compute_bounds();
        assert!((known.width() - unknown.width()).abs() < 1e-3);
        assert!((known.width() - blank.width()).abs() < 1e-3);
    }

    #[test]
    fn new_text_objects_default_to_the_catalogue_default_family() {
        assert_eq!(VectorObject::default().font_family, fonts::DEFAULT_FAMILY);
    }
}
