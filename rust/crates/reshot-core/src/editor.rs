//! The editing session behind the overlay, free of any UI: the document, its undo history, the
//! per-tool settings, and every drawing gesture (brush, shapes, text, effects, erasers). The
//! overlay feeds it points in frame pixels and draws what it reports.

use std::collections::HashMap;

use skia_safe::gradient::{Colors, Gradient, Interpolation, shaders};
use skia_safe::{
    BlendMode, Bitmap, Color, Color4f, FilterMode, IRect, MipmapMode, Paint, PaintCap, PaintJoin, PaintStyle, Path,
    PathBuilder, Point, Rect, SamplingOptions, TileMode, image_filters,
};

use crate::document::{self, CaptureDocument, Layer};
use crate::history::{LayerRegionCommand, UndoHistory};
use crate::tools::{BrushSettings, BrushStroke, VectorKind, VectorObject};

/// Every drawing sub-tool (SPEC §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DrawTool {
    Brush,
    Line,
    Arrow,
    Square,
    Circle,
    Triangle,
    Text,
    Blur,
    Pixelize,
    Eraser,
    AbsoluteEraser,
    FilterEraser,
}

impl DrawTool {
    /// Region tools paint an area (brushed or Ctrl-rect) rather than ink.
    pub fn is_region(self) -> bool {
        matches!(self, Self::Blur | Self::Pixelize | Self::Eraser | Self::AbsoluteEraser | Self::FilterEraser)
    }

    pub fn is_eraser(self) -> bool {
        matches!(self, Self::Eraser | Self::AbsoluteEraser | Self::FilterEraser)
    }

    pub fn is_effect(self) -> bool {
        matches!(self, Self::Blur | Self::Pixelize)
    }

    pub fn is_shape(self) -> bool {
        matches!(self, Self::Line | Self::Arrow | Self::Square | Self::Circle | Self::Triangle)
    }

    /// Ink tools have a colour; erasers and effects do not.
    pub fn has_color(self) -> bool {
        !self.is_region()
    }

    /// Which settings slot the tool draws from. Shapes share one, lines share one, the three
    /// erasers share one; blur and pixelize each get their own (different defaults).
    pub fn settings_key(self) -> ToolKey {
        match self {
            Self::Square | Self::Circle | Self::Triangle => ToolKey::Shapes,
            Self::Line | Self::Arrow => ToolKey::Lines,
            Self::Text => ToolKey::Text,
            Self::Eraser | Self::AbsoluteEraser | Self::FilterEraser => ToolKey::Eraser,
            Self::Blur => ToolKey::Blur,
            Self::Pixelize => ToolKey::Pixelize,
            Self::Brush => ToolKey::Brush,
        }
    }

    fn vector_kind(self) -> VectorKind {
        match self {
            Self::Line => VectorKind::Line,
            Self::Arrow => VectorKind::Arrow,
            Self::Circle => VectorKind::Circle,
            Self::Triangle => VectorKind::Triangle,
            _ => VectorKind::Square,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKey {
    Brush,
    Shapes,
    Lines,
    Text,
    Eraser,
    Blur,
    Pixelize,
}

/// Per-tool defaults, as the C# build shipped them. Effects keep their strength (0..1) in
/// `opacity`.
pub fn default_tool_settings() -> HashMap<ToolKey, BrushSettings> {
    let red = Color::from_rgb(0xFF, 0x3B, 0x30);
    let ink = |t: f32| BrushSettings { thickness: t, opacity: 1.0, color: red, ..Default::default() };
    HashMap::from([
        (ToolKey::Brush, ink(5.0)),
        (ToolKey::Shapes, ink(5.0)),
        (ToolKey::Lines, ink(5.0)),
        (ToolKey::Text, ink(5.0)),
        (ToolKey::Eraser, BrushSettings { thickness: 25.0, opacity: 1.0, ..Default::default() }),
        (ToolKey::Pixelize, BrushSettings { thickness: 30.0, opacity: 0.15, ..Default::default() }),
        (ToolKey::Blur, BrushSettings { thickness: 30.0, opacity: 0.50, ..Default::default() }),
    ])
}

/// A region gesture in progress: brushed path or Ctrl-dragged rect.
struct RegionGesture {
    tool: DrawTool,
    start: Point,
    rect_mode: bool,
    rect: Rect,
    path: PathBuilder,
    clip: Option<Path>,
}

struct BrushGesture {
    stroke: BrushStroke,
    clip: Option<Path>,
}

pub struct Editor {
    doc: CaptureDocument,
    base: Bitmap,
    history: UndoHistory<CaptureDocument>,
    pub tool_settings: HashMap<ToolKey, BrushSettings>,
    blur_cache: Option<(f32, Bitmap)>,
    pixel_cache: Option<(i32, Bitmap)>,
    brush: Option<BrushGesture>,
    shape: Option<(VectorObject, Point)>,
    shape_clip: Option<Path>,
    text: Option<VectorObject>,
    text_clip: Option<Path>,
    region: Option<RegionGesture>,
    /// Union of everything ever drawn, so the overlay can bound its repaint.
    pub content_bounds: Option<IRect>,
    /// Union of absolute-erased areas: drawn with a checkerboard in the overlay.
    pub absolute_bounds: Option<Rect>,
    /// Bumped on every change to the document, so the view knows to re-upload.
    pub revision: u64,
    /// The area changed by the last change, for a partial re-upload.
    pub dirty: Option<IRect>,
}

impl Editor {
    pub fn new(base: Bitmap) -> Self {
        let doc = CaptureDocument::new(base.width(), base.height());
        Self {
            doc,
            base,
            history: UndoHistory::new(),
            tool_settings: default_tool_settings(),
            blur_cache: None,
            pixel_cache: None,
            brush: None,
            shape: None,
            shape_clip: None,
            text: None,
            text_clip: None,
            region: None,
            content_bounds: None,
            absolute_bounds: None,
            revision: 0,
            dirty: None,
        }
    }

    pub fn document(&self) -> &CaptureDocument {
        &self.doc
    }

    pub fn base(&self) -> &Bitmap {
        &self.base
    }

    pub fn width(&self) -> i32 {
        self.base.width()
    }

    pub fn height(&self) -> i32 {
        self.base.height()
    }

    pub fn settings(&self, tool: DrawTool) -> &BrushSettings {
        &self.tool_settings[&tool.settings_key()]
    }

    pub fn settings_mut(&mut self, tool: DrawTool) -> &mut BrushSettings {
        self.tool_settings.get_mut(&tool.settings_key()).expect("every key has defaults")
    }

    fn touched(&mut self, region: IRect) {
        self.revision += 1;
        self.dirty = Some(match self.dirty {
            Some(d) => IRect::join(&d, &region),
            None => region,
        });
        self.content_bounds = Some(match self.content_bounds {
            Some(c) => IRect::join(&c, &region),
            None => region,
        });
    }

    /// Takes the accumulated dirty area (whole frame after undo or redo).
    pub fn take_dirty(&mut self) -> Option<IRect> {
        self.dirty.take()
    }

    /// Rect clamped to the frame, rounded outwards.
    pub fn to_region(&self, b: Rect) -> Option<IRect> {
        let r = IRect::from_ltrb(
            (b.left.floor() as i32).clamp(0, self.width()),
            (b.top.floor() as i32).clamp(0, self.height()),
            (b.right.ceil() as i32).clamp(0, self.width()),
            (b.bottom.ceil() as i32).clamp(0, self.height()),
        );
        (r.width() > 0 && r.height() > 0).then_some(r)
    }

    fn apply_region_op(&mut self, layer: Layer, region: IRect, op: impl FnOnce(&mut CaptureDocument)) {
        let cmd = LayerRegionCommand::record(&mut self.doc, layer, region, op);
        self.history.push(cmd);
        self.touched(region);
    }

    // ---- Undo ----------------------------------------------------------------------

    pub fn undo(&mut self) -> bool {
        self.finish_gesture();
        let ok = self.history.undo(&mut self.doc);
        if ok {
            self.touched(IRect::from_wh(self.width(), self.height()));
        }
        ok
    }

    pub fn redo(&mut self) -> bool {
        self.finish_gesture();
        let ok = self.history.redo(&mut self.doc);
        if ok {
            self.touched(IRect::from_wh(self.width(), self.height()));
        }
        ok
    }

    // ---- Brush -----------------------------------------------------------------------

    pub fn begin_brush(&mut self, p: Point, clip: Option<Path>) {
        let mut stroke = BrushStroke::new(self.settings(DrawTool::Brush).clone());
        stroke.begin(p);
        self.brush = Some(BrushGesture { stroke, clip });
    }

    pub fn extend(&mut self, p: Point) {
        if let Some(b) = &mut self.brush {
            b.stroke.extend(p);
        }
        if let Some((v, start)) = &mut self.shape {
            update_shape(v, *start, p);
        }
        if let Some(r) = &mut self.region {
            if r.rect_mode {
                r.rect = Rect::from_ltrb(r.start.x.min(p.x), r.start.y.min(p.y), r.start.x.max(p.x), r.start.y.max(p.y));
            } else {
                r.path.line_to(p);
            }
        }
    }

    /// The live brush stroke and its paint, for the preview.
    pub fn brush_preview(&self) -> Option<(Path, Paint, Option<&Path>)> {
        self.brush.as_ref().map(|b| (b.stroke.path(), b.stroke.paint(), b.clip.as_ref()))
    }

    fn end_brush(&mut self) {
        let Some(BrushGesture { stroke, clip }) = self.brush.take() else { return };
        if stroke.is_empty() {
            return;
        }
        let path = stroke.path();
        let pad = stroke.settings.thickness / 2.0 + 2.0;
        let b = path.bounds();
        let Some(region) = self.to_region(Rect::from_ltrb(b.left - pad, b.top - pad, b.right + pad, b.bottom + pad))
        else {
            return;
        };
        let paint = stroke.paint();
        self.apply_region_op(Layer::Paint, region, |d| d.commit_stroke(&path, &paint, clip.as_ref()));
    }

    // ---- Shapes and text ---------------------------------------------------------------

    pub fn begin_shape(&mut self, tool: DrawTool, p: Point, clip: Option<Path>) {
        let s = self.settings(tool);
        let v = VectorObject {
            kind: tool.vector_kind(),
            color: s.color,
            thickness: s.thickness,
            p1: p,
            p2: p,
            bounds: Rect::from_xywh(p.x, p.y, 0.0, 0.0),
            ..Default::default()
        };
        self.shape = Some((v, p));
        self.shape_clip = clip;
    }

    pub fn shape_preview(&self) -> Option<&VectorObject> {
        self.shape.as_ref().map(|(v, _)| v)
    }

    fn end_shape(&mut self) {
        let Some((v, _)) = self.shape.take() else { return };
        let clip = self.shape_clip.take();
        let b = v.compute_bounds();
        if b.width() >= 3.0 || b.height() >= 3.0 {
            self.commit_vector(&v, clip.as_ref());
        }
    }

    /// Bakes a shape or text block into the paint layer, clipped to the selection, with a
    /// region-snapshot undo. Once baked it is ordinary ink.
    fn commit_vector(&mut self, v: &VectorObject, clip: Option<&Path>) {
        // 44 px covers the largest arrowhead barb (clamped to 40) plus anti-aliasing.
        let pad = v.thickness / 2.0 + 44.0;
        let b = v.compute_bounds();
        let Some(region) = self.to_region(Rect::from_ltrb(b.left - pad, b.top - pad, b.right + pad, b.bottom + pad))
        else {
            return;
        };
        self.apply_region_op(Layer::Paint, region, |d| d.commit_vector(v, clip));
    }

    pub fn begin_text(&mut self, p: Point, clip: Option<Path>) {
        self.commit_text();
        let s = self.settings(DrawTool::Text);
        self.text = Some(VectorObject {
            kind: VectorKind::Text,
            p1: p,
            color: s.color,
            font_size: (s.thickness * 5.0).max(16.0),
            font_family: s.font_family.clone(),
            ..Default::default()
        });
        self.text_clip = clip;
    }

    pub fn text_editing(&self) -> Option<&VectorObject> {
        self.text.as_ref()
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }

    pub fn type_str(&mut self, s: &str) {
        if let Some(t) = &mut self.text {
            t.text.extend(s.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn type_newline(&mut self) {
        if let Some(t) = &mut self.text {
            t.text.push('\n');
        }
    }

    pub fn backspace(&mut self) {
        if let Some(t) = &mut self.text {
            t.text.pop();
        }
    }

    /// Esc, click-away or tool switch: keeps what was typed.
    pub fn commit_text(&mut self) {
        let Some(t) = self.text.take() else { return };
        let clip = self.text_clip.take();
        if !t.text.is_empty() {
            self.commit_vector(&t, clip.as_ref());
        }
    }

    // ---- Effects and erasers --------------------------------------------------------------

    pub fn begin_region(&mut self, tool: DrawTool, p: Point, rect_mode: bool, clip: Option<Path>) {
        let mut path = PathBuilder::new();
        if !rect_mode {
            path.move_to(p);
        }
        self.region = Some(RegionGesture {
            tool,
            start: p,
            rect_mode,
            rect: Rect::from_xywh(p.x, p.y, 0.0, 0.0),
            path,
            clip,
        });
    }

    /// The area the current region gesture covers, for the preview.
    pub fn region_preview(&self) -> Option<(DrawTool, Path)> {
        self.region.as_ref().map(|r| (r.tool, self.region_area(r)))
    }

    fn region_area(&self, r: &RegionGesture) -> Path {
        if r.rect_mode {
            return Path::rect(r.rect, None);
        }
        let mut stroke = Paint::default();
        stroke.set_style(PaintStyle::Stroke);
        stroke.set_stroke_width(self.settings(r.tool).thickness.max(10.0));
        stroke.set_stroke_cap(PaintCap::Round);
        stroke.set_stroke_join(PaintJoin::Round);
        let src = r.path.snapshot();
        let mut out = PathBuilder::new();
        if skia_safe::path_utils::fill_path_with_paint(&src, &stroke, &mut out, None, None) {
            out.detach()
        } else {
            Path::new()
        }
    }

    /// The effect source for blur or pixelize at the tool's strength. Built from the base
    /// frame and cached until the strength changes.
    pub fn effect_source(&mut self, tool: DrawTool) -> &Bitmap {
        let strength = self.settings(tool).opacity.clamp(0.0, 1.0);
        let (w, h) = (self.width(), self.height());
        if tool == DrawTool::Pixelize {
            // Higher strength → bigger blocks.
            let factor = ((3.0 + strength * 11.25).round() as i32).max(2);
            if self.pixel_cache.as_ref().is_none_or(|(f, _)| *f != factor) {
                let (sw, sh) = ((w / factor).max(1), (h / factor).max(1));
                let image = self.base.as_image();
                let mut small = document::new_bitmap(sw, sh);
                document::with_canvas(&mut small, |c| {
                    c.draw_image_rect_with_sampling_options(
                        &image,
                        None,
                        Rect::from_wh(sw as f32, sh as f32),
                        SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
                        &Paint::default(),
                    );
                });
                let small_image = small.as_image();
                let mut big = document::new_bitmap(w, h);
                document::with_canvas(&mut big, |c| {
                    c.draw_image_rect_with_sampling_options(
                        &small_image,
                        None,
                        Rect::from_wh(w as f32, h as f32),
                        SamplingOptions::new(FilterMode::Nearest, MipmapMode::None),
                        &Paint::default(),
                    );
                });
                self.pixel_cache = Some((factor, big));
            }
            return &self.pixel_cache.as_ref().expect("just built").1;
        }
        // Higher strength → bigger sigma.
        let sigma = (strength * 10.9375).max(1.0);
        if self.blur_cache.as_ref().is_none_or(|(s, _)| (s - sigma).abs() > 0.01) {
            let mut out = document::new_bitmap(w, h);
            let image = self.base.as_image();
            document::with_canvas(&mut out, |c| {
                let mut p = Paint::default();
                p.set_image_filter(image_filters::blur((sigma, sigma), None, None, None));
                c.draw_image(&image, (0, 0), Some(&p));
            });
            self.blur_cache = Some((sigma, out));
        }
        &self.blur_cache.as_ref().expect("just built").1
    }

    fn end_region(&mut self) {
        let Some(r) = self.region.take() else { return };
        let area = self.region_area(&r);
        let Some(region) = self.to_region(*area.bounds()) else { return };
        let clip = r.clip.clone();
        match r.tool {
            DrawTool::Blur | DrawTool::Pixelize => {
                let src = self.effect_source(r.tool).clone();
                self.apply_region_op(Layer::Effects, region, |d| d.apply_effect(&src, &area, clip.as_ref()));
            }
            // A no-op erase must not materialise an untouched layer.
            DrawTool::FilterEraser if self.doc.has_effects => {
                let cov = self.erase_coverage(&r, region);
                self.apply_region_op(Layer::Effects, region, |d| {
                    d.erase_effects_coverage(&cov, region.left, region.top, clip.as_ref())
                });
            }
            DrawTool::Eraser if self.doc.has_paint => {
                // Shapes and text are baked into paint, so this erases them by touch; the
                // effects layer is the Filter Eraser's job and is left alone.
                let cov = self.erase_coverage(&r, region);
                self.apply_region_op(Layer::Paint, region, |d| {
                    d.erase_paint_coverage(&cov, region.left, region.top, clip.as_ref())
                });
            }
            DrawTool::AbsoluteEraser => {
                let cov = self.erase_coverage(&r, region);
                self.apply_region_op(Layer::Absolute, region, |d| {
                    d.absolute_erase_coverage(&cov, region.left, region.top, clip.as_ref())
                });
                let b = *area.bounds();
                self.absolute_bounds = Some(match self.absolute_bounds {
                    Some(mut a) => {
                        a.join(b);
                        a
                    }
                    None => b,
                });
            }
            _ => {}
        }
    }

    /// Soft-eraser coverage over `region`: an opaque greyscale bitmap whose luminance is the
    /// erase strength. Brush mode stamps radial discs (centre 100%, rim = hardness) combined
    /// with Lighten (max), so overlapping stamps never compound past the profile. Rect mode
    /// fills flat at full strength.
    fn erase_coverage(&self, r: &RegionGesture, region: IRect) -> Bitmap {
        let s = self.settings(r.tool);
        let rim = (s.hardness.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut cov = document::new_bitmap(region.width(), region.height());
        cov.erase_color(Color::BLACK);
        document::with_canvas(&mut cov, |c| {
            c.translate((-region.left as f32, -region.top as f32));
            if r.rect_mode {
                let mut fill = Paint::default();
                fill.set_color(Color::WHITE);
                c.draw_rect(r.rect, &fill);
                return;
            }
            let radius = s.thickness.max(10.0) / 2.0;
            let rim = rim as f32 / 255.0;
            let stops = [Color4f::new(1.0, 1.0, 1.0, 1.0), Color4f::new(rim, rim, rim, 1.0)];
            let positions = [0.0f32, 1.0];
            let gradient =
                Gradient::new(Colors::new(&stops, Some(&positions), TileMode::Clamp, None), Interpolation::default());
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_blend_mode(BlendMode::Lighten);
            for pt in flatten_polyline(&r.path.snapshot(), radius * 0.35) {
                paint.set_shader(shaders::radial_gradient((pt, radius), &gradient, None));
                c.draw_circle(pt, radius, &paint);
            }
        });
        cov
    }

    // ---- Gesture plumbing -------------------------------------------------------------------

    pub fn is_drawing(&self) -> bool {
        self.brush.is_some() || self.shape.is_some() || self.region.is_some()
    }

    /// Mouse up: commits whatever gesture is live.
    pub fn end_gesture(&mut self) {
        self.end_brush();
        self.end_shape();
        self.end_region();
    }

    /// Ends any live gesture before the tool or the selection changes underneath it.
    pub fn finish_gesture(&mut self) {
        self.end_gesture();
    }

    /// Sample colour under a frame point: the composite of base, effects and paint.
    pub fn sample_color(&self, p: Point) -> Color {
        let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
        if x < 0 || y < 0 || x >= self.width() || y >= self.height() {
            return Color::BLACK;
        }
        let mut px = document::new_bitmap(1, 1);
        let base = self.base.as_image();
        document::with_canvas(&mut px, |c| {
            c.translate((-x as f32, -y as f32));
            c.draw_image(&base, (0, 0), None);
            self.doc.draw_overlay(c);
        });
        let b = document::pixels(&px);
        Color::from_rgb(b[2], b[1], b[0])
    }
}

fn update_shape(v: &mut VectorObject, start: Point, cur: Point) {
    if matches!(v.kind, VectorKind::Line | VectorKind::Arrow) {
        v.p2 = cur;
    } else {
        v.bounds = Rect::from_ltrb(start.x.min(cur.x), start.y.min(cur.y), start.x.max(cur.x), start.y.max(cur.y));
        // The triangle points the way you drag.
        if v.kind == VectorKind::Triangle {
            v.points_up = cur.y <= start.y;
        }
    }
}

/// Samples a polyline into points spaced about `spacing` apart.
fn flatten_polyline(path: &Path, spacing: f32) -> Vec<Point> {
    let verts = path.points();
    let mut out = Vec::new();
    let Some(first) = verts.first() else { return out };
    let spacing = spacing.max(1.0);
    out.push(*first);
    for w in verts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let steps = (((dx * dx + dy * dy).sqrt() / spacing) as i32).max(1);
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            out.push(Point::new(a.x + dx * t, a.y + dy * t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let base = document::new_bitmap(100, 100);
        base.erase_color(Color::from_rgb(0, 128, 0));
        Editor::new(base)
    }

    fn paint_alpha(e: &Editor, x: i32, y: i32) -> u8 {
        let b = e.document().layer(Layer::Paint).unwrap();
        document::pixels(b)[y as usize * b.row_bytes() + x as usize * 4 + 3]
    }

    #[test]
    fn brush_stroke_commits_and_undoes() {
        let mut e = editor();
        e.begin_brush(Point::new(10.0, 50.0), None);
        e.extend(Point::new(90.0, 50.0));
        e.end_gesture();
        assert_eq!(paint_alpha(&e, 50, 50), 255);
        assert!(e.take_dirty().is_some());
        assert!(e.undo());
        assert_eq!(paint_alpha(&e, 50, 50), 0);
        assert!(e.redo());
        assert_eq!(paint_alpha(&e, 50, 50), 255);
    }

    #[test]
    fn stroke_is_clipped_to_the_selection() {
        let mut e = editor();
        e.begin_brush(Point::new(10.0, 50.0), Some(Path::rect(Rect::from_ltrb(0.0, 0.0, 40.0, 100.0), None)));
        e.extend(Point::new(90.0, 50.0));
        e.end_gesture();
        assert_eq!(paint_alpha(&e, 20, 50), 255);
        assert_eq!(paint_alpha(&e, 60, 50), 0);
    }

    #[test]
    fn tiny_shapes_are_dropped() {
        let mut e = editor();
        e.begin_shape(DrawTool::Square, Point::new(10.0, 10.0), None);
        e.extend(Point::new(11.0, 11.0));
        e.end_gesture();
        assert!(e.document().layer(Layer::Paint).is_none());
    }

    #[test]
    fn typed_text_bakes_on_commit() {
        let mut e = editor();
        e.begin_text(Point::new(5.0, 5.0), None);
        e.type_str("Hi\u{8}");
        e.type_newline();
        e.type_str("x");
        e.backspace();
        assert_eq!(e.text_editing().unwrap().text, "Hi\n");
        e.commit_text();
        assert!(e.document().has_paint);
        assert!(!e.is_typing());
    }

    #[test]
    fn blur_writes_effects_and_filter_eraser_removes_them() {
        let mut e = editor();
        e.begin_region(DrawTool::Blur, Point::new(0.0, 0.0), true, None);
        e.extend(Point::new(100.0, 100.0));
        e.end_gesture();
        assert!(e.document().has_effects);
        e.begin_region(DrawTool::FilterEraser, Point::new(0.0, 0.0), true, None);
        e.extend(Point::new(100.0, 100.0));
        e.end_gesture();
        let fx = e.document().layer(Layer::Effects).unwrap();
        assert_eq!(document::pixels(fx)[50 * fx.row_bytes() + 50 * 4 + 3], 0);
    }

    #[test]
    fn pixelize_strength_changes_the_block_size() {
        let mut e = editor();
        e.settings_mut(DrawTool::Pixelize).opacity = 0.0;
        e.effect_source(DrawTool::Pixelize);
        let small = e.pixel_cache.as_ref().unwrap().0;
        e.settings_mut(DrawTool::Pixelize).opacity = 1.0;
        e.effect_source(DrawTool::Pixelize);
        assert!(e.pixel_cache.as_ref().unwrap().0 > small);
    }

    #[test]
    fn eraser_on_untouched_paint_does_nothing() {
        let mut e = editor();
        e.begin_region(DrawTool::Eraser, Point::new(10.0, 10.0), false, None);
        e.extend(Point::new(50.0, 50.0));
        e.end_gesture();
        assert!(e.document().layer(Layer::Paint).is_none());
        assert!(!e.undo());
    }

    #[test]
    fn absolute_eraser_tracks_its_bounds() {
        let mut e = editor();
        e.begin_region(DrawTool::AbsoluteEraser, Point::new(10.0, 10.0), true, None);
        e.extend(Point::new(30.0, 40.0));
        e.end_gesture();
        assert_eq!(e.absolute_bounds, Some(Rect::from_ltrb(10.0, 10.0, 30.0, 40.0)));
        assert!(e.document().has_absolute);
    }

    #[test]
    fn sampling_reads_the_composite() {
        let e = editor();
        assert_eq!(e.sample_color(Point::new(3.0, 3.0)), Color::from_rgb(0, 128, 0));
    }

    #[test]
    fn tool_settings_are_per_slot() {
        let mut e = editor();
        e.settings_mut(DrawTool::Square).thickness = 20.0;
        assert_eq!(e.settings(DrawTool::Circle).thickness, 20.0);
        assert_eq!(e.settings(DrawTool::Brush).thickness, 5.0);
    }
}
