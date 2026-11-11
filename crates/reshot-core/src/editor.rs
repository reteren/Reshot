//! The editing session behind the overlay, free of any UI: the document, its undo history, the
//! per-tool settings, and every drawing gesture (brush, shapes, text, effects, erasers). The
//! overlay feeds it points in frame pixels and draws what it reports.

use std::collections::HashMap;
use std::ops::Range;

use skia_safe::{
    Bitmap, BlendMode, Color, FilterMode, IRect, Image, MipmapMode, Paint, Path, Point, Rect, SamplingOptions,
    TileMode, image_filters,
};

use crate::document::{self, CaptureDocument, Layer};
use crate::history::{LayerRegionCommand, TextCommand, UndoHistory};
use crate::text::TextAnnotation;
use crate::soft::SoftStroke;
use crate::tools::{BrushSettings, VectorKind, VectorObject};

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
        matches!(
            self,
            Self::Blur | Self::Pixelize | Self::Eraser | Self::AbsoluteEraser | Self::FilterEraser
        )
    }

    pub fn is_eraser(self) -> bool {
        matches!(
            self,
            Self::Eraser | Self::AbsoluteEraser | Self::FilterEraser
        )
    }

    pub fn is_effect(self) -> bool {
        matches!(self, Self::Blur | Self::Pixelize)
    }

    pub fn is_shape(self) -> bool {
        matches!(
            self,
            Self::Line | Self::Arrow | Self::Square | Self::Circle | Self::Triangle
        )
    }

    /// Ink tools have a colour; erasers and effects do not.
    pub fn has_color(self) -> bool {
        !self.is_region()
    }

    /// Every tool with an edge has a hardness; text has none.
    pub fn has_hardness(self) -> bool {
        self != Self::Text
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
    let ink = |t: f32| BrushSettings {
        thickness: t,
        opacity: 1.0,
        color: red,
        ..Default::default()
    };
    HashMap::from([
        (ToolKey::Brush, ink(5.0)),
        (ToolKey::Shapes, ink(5.0)),
        (ToolKey::Lines, ink(5.0)),
        (ToolKey::Text, ink(5.0)),
        (
            ToolKey::Eraser,
            BrushSettings {
                thickness: 25.0,
                opacity: 1.0,
                ..Default::default()
            },
        ),
        (
            ToolKey::Pixelize,
            BrushSettings {
                thickness: 30.0,
                opacity: 0.15,
                ..Default::default()
            },
        ),
        (
            ToolKey::Blur,
            BrushSettings {
                thickness: 30.0,
                opacity: 0.50,
                ..Default::default()
            },
        ),
    ])
}

/// Coverage placed in the frame: where it sits, and its pixels as an image.
type PlacedCoverage = (IRect, Image);

/// A soft stroke in progress (brush, effect or eraser), or a Ctrl-dragged rect.
struct RegionGesture {
    tool: DrawTool,
    start: Point,
    rect_mode: bool,
    rect: Rect,
    clip: Option<Path>,
    /// Bumped by every new point, so the per-frame cache below knows when it is stale.
    generation: u64,
    /// The soft stroke (not for a Ctrl-rect), built a piece at a time with the brush's
    /// hardness (see [`soft`]). A move adds only its own pieces instead of redoing the whole
    /// stroke, and the commit uses exactly what the preview showed.
    stroke: Option<SoftStroke>,
    /// What the latest point changed on screen.
    damage: Option<IRect>,
    /// What every tile of one frame shares: the rect's area and the coverage image at
    /// `generation`.
    frame: Option<(u64, Option<Path>, Option<PlacedCoverage>)>,
}

struct TextDrag {
    id: u64,
    pointer: Point,
    origin: Point,
    before: Vec<TextAnnotation>,
}

struct TextEdit {
    index: usize,
    before: Vec<TextAnnotation>,
}

pub struct Editor {
    doc: CaptureDocument,
    base: Bitmap,
    history: UndoHistory<CaptureDocument>,
    pub tool_settings: HashMap<ToolKey, BrushSettings>,
    blur_cache: Option<(f32, Bitmap, Image)>,
    pixel_cache: Option<(i32, Bitmap, Image)>,
    /// The layers as images, at `raster_revision`. Moving text does not invalidate them.
    /// A mutable bitmap's `as_image` copies all of it,
    /// and the overlay paints tile by tile: per tile, per frame, that was 25 MB per layer.
    layer_images: (u64, [Option<Image>; 3]),
    raster_revision: u64,
    shape: Option<(VectorObject, Point)>,
    shape_clip: Option<Path>,
    text: Option<VectorObject>,
    text_clip: Option<Path>,
    text_marked: Option<Range<usize>>,
    text_edit: Option<TextEdit>,
    text_drag: Option<TextDrag>,
    selected_text: Option<u64>,
    next_text_id: u64,
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
            layer_images: (u64::MAX, [None, None, None]),
            raster_revision: 0,
            shape: None,
            shape_clip: None,
            text: None,
            text_clip: None,
            text_marked: None,
            text_edit: None,
            text_drag: None,
            selected_text: None,
            next_text_id: 0,
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
        self.tool_settings
            .get_mut(&tool.settings_key())
            .expect("every key has defaults")
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

    fn apply_region_op(
        &mut self,
        layer: Layer,
        region: IRect,
        op: impl FnOnce(&mut CaptureDocument),
    ) {
        let cmd = LayerRegionCommand::record(&mut self.doc, layer, region, op);
        self.history.push(cmd);
        self.raster_revision += 1;
        self.touched(region);
    }

    // ---- Undo ----------------------------------------------------------------------

    pub fn undo(&mut self) -> bool {
        self.commit_text();
        self.finish_gesture();
        let ok = self.history.undo(&mut self.doc);
        if ok {
            self.raster_revision += 1;
            self.touched(IRect::from_wh(self.width(), self.height()));
        }
        ok
    }

    pub fn redo(&mut self) -> bool {
        self.commit_text();
        self.finish_gesture();
        let ok = self.history.redo(&mut self.doc);
        if ok {
            self.raster_revision += 1;
            self.touched(IRect::from_wh(self.width(), self.height()));
        }
        ok
    }

    // ---- Brush -----------------------------------------------------------------------

    pub fn begin_brush(&mut self, p: Point, clip: Option<Path>) {
        self.begin_region(DrawTool::Brush, p, false, clip);
    }

    pub fn extend(&mut self, p: Point) {
        if let Some(drag) = &self.text_drag {
            let (id, origin, pointer) = (drag.id, drag.origin, drag.pointer);
            let size = (self.width() as f32, self.height() as f32);
            if let Some(text) = self.doc.texts.iter_mut().find(|text| text.id == id) {
                let bounds = text.bounds();
                let mut area = Rect::from_wh(size.0, size.1);
                if let Some(clip) = &text.clip { area.intersect(clip.bounds()); }
                let x = (origin.x + p.x - pointer.x).clamp(area.left, (area.right - bounds.width()).max(area.left));
                let y = (origin.y + p.y - pointer.y).clamp(area.top, (area.bottom - bounds.height()).max(area.top));
                text.object.p1 = Point::new(x, y);
                let dirty = Rect::join2(bounds, text.bounds());
                if let Some(region) = self.to_region(dirty.with_outset((3.0, 3.0))) { self.touched(region); }
            }
            return;
        }
        if let Some((v, start)) = &mut self.shape {
            update_shape(v, *start, p);
        }
        if let Some(r) = self.region.as_mut() {
            r.generation += 1;
            if r.rect_mode {
                r.rect = Rect::from_ltrb(
                    r.start.x.min(p.x),
                    r.start.y.min(p.y),
                    r.start.x.max(p.x),
                    r.start.y.max(p.y),
                );
            } else {
                r.damage = r.stroke.as_mut().and_then(|s| s.add(p));
            }
        }
    }

    /// Every pointer point since the last frame. A soft stroke takes them in one batch; the
    /// other gestures only need where the pointer is now.
    pub fn extend_all(&mut self, points: &[Point]) {
        let Some(&last) = points.last() else { return };
        if let Some(r) = self.region.as_mut()
            && !r.rect_mode
            && let Some(stroke) = r.stroke.as_mut()
        {
            r.generation += 1;
            r.damage = stroke.add_all(points);
            return;
        }
        self.extend(last);
    }

    // ---- Shapes and text ---------------------------------------------------------------

    pub fn begin_shape(&mut self, tool: DrawTool, p: Point, clip: Option<Path>) {
        let s = self.settings(tool);
        let v = VectorObject {
            kind: tool.vector_kind(),
            color: s.color,
            opacity: s.opacity,
            hardness: s.hardness,
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
        let Some((v, _)) = self.shape.take() else {
            return;
        };
        let clip = self.shape_clip.take();
        let b = v.compute_bounds();
        if b.width() >= 3.0 || b.height() >= 3.0 {
            self.commit_vector(&v, clip.as_ref());
        }
    }

    /// Bakes a shape into the paint layer, clipped to the selection, with a
    /// region-snapshot undo. Once baked it is ordinary ink.
    fn commit_vector(&mut self, v: &VectorObject, clip: Option<&Path>) {
        let pad = v.padding();
        let b = v.compute_bounds();
        let Some(region) = self.to_region(Rect::from_ltrb(
            b.left - pad,
            b.top - pad,
            b.right + pad,
            b.bottom + pad,
        )) else {
            return;
        };
        self.apply_region_op(Layer::Paint, region, |d| d.commit_vector(v, clip));
    }

    pub fn begin_text(&mut self, p: Point, clip: Option<Path>) {
        self.commit_text();
        self.text_marked = None;
        self.selected_text = None;
        let s = self.settings(DrawTool::Text);
        let mut t = VectorObject {
            kind: VectorKind::Text,
            p1: p,
            color: s.color,
            opacity: s.opacity,
            font_size: (s.thickness * 5.0).max(16.0),
            font_family: s.font_family.clone(),
            ..Default::default()
        };
        // p1 is the line's top-left; the click is where the first line's middle goes, so the
        // text sits on the cursor rather than below it.
        t.p1.y -= t.line_height() / 2.0;
        self.text = Some(t);
        self.text_clip = clip;
    }

    pub fn text_editing(&self) -> Option<&VectorObject> {
        self.text.as_ref()
    }

    pub fn hit_text(&self, point: Point) -> Option<u64> {
        self.doc.texts.iter().rev().find(|text| text.contains(point)).map(|text| text.id)
    }

    pub fn selected_text_bounds(&self) -> Option<Rect> {
        if let Some(text) = &self.text { return Some(text.compute_bounds()); }
        self.doc.texts.iter().find(|text| Some(text.id) == self.selected_text).map(|text| text.bounds())
    }

    pub fn begin_text_drag(&mut self, point: Point) -> bool {
        self.commit_text();
        self.end_text_drag();
        let Some(id) = self.hit_text(point) else { self.selected_text = None; return false };
        let origin = self.doc.texts.iter().find(|text| text.id == id).unwrap().object.p1;
        self.selected_text = Some(id);
        self.text_drag = Some(TextDrag { id, pointer: point, origin, before: self.doc.texts.clone() });
        true
    }

    pub fn is_dragging_text(&self) -> bool { self.text_drag.is_some() }

    fn end_text_drag(&mut self) {
        if let Some(drag) = self.text_drag.take()
            && self.doc.texts.iter().any(|text| text.id == drag.id && text.object.p1 != drag.origin)
        {
            self.history.push(TextCommand { before: drag.before, after: self.doc.texts.clone() });
        }
    }

    /// A double click reopens the retained annotation instead of placing another on top.
    pub fn edit_text_at(&mut self, point: Point) -> bool {
        self.commit_text();
        let Some(id) = self.hit_text(point) else { return false };
        let index = self.doc.texts.iter().position(|text| text.id == id).unwrap();
        let before = self.doc.texts.clone();
        let text = self.doc.texts.remove(index);
        self.selected_text = Some(id);
        self.text_clip = text.clip;
        let settings = self.settings_mut(DrawTool::Text);
        settings.color = text.object.color;
        settings.opacity = text.object.opacity;
        settings.thickness = text.object.font_size / 5.0;
        settings.font_family = text.object.font_family.clone();
        self.text = Some(text.object);
        self.text_edit = Some(TextEdit { index, before });
        if let Some(region) = self.live_bounds() { self.touched(region); }
        true
    }

    pub fn delete_selected_text(&mut self) -> bool {
        self.commit_text();
        let Some(id) = self.selected_text.take() else { return false };
        let Some(index) = self.doc.texts.iter().position(|text| text.id == id) else { return false };
        let before = self.doc.texts.clone();
        let bounds = self.doc.texts.remove(index).bounds();
        self.history.push(TextCommand { before, after: self.doc.texts.clone() });
        if let Some(region) = self.to_region(bounds.with_outset((3.0, 3.0))) { self.touched(region); }
        true
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }

    pub fn marked_text_range(&self) -> Option<Range<usize>> { self.text_marked.clone() }
    pub fn unmark_text(&mut self) { self.text_marked = None; }

    pub fn text_for_range_utf16(&self, range: Range<usize>) -> Option<(String, Range<usize>)> {
        let text = &self.text.as_ref()?.text;
        let bytes = utf16_byte_range(text, range);
        let start = text[..bytes.start].encode_utf16().count();
        let end = start + text[bytes.clone()].encode_utf16().count();
        Some((text[bytes].to_owned(), start..end))
    }

    /// Native Windows text input (including VK_PACKET and IME) uses UTF-16 offsets and
    /// replaces composition text rather than appending each intermediate candidate.
    pub fn replace_native_text(&mut self, range: Option<Range<usize>>, value: &str, marked: bool) {
        let previous_mark = self.text_marked.take();
        let Some(text) = self.text.as_mut() else { return };
        let end = text.text.encode_utf16().count();
        let range = range.or(previous_mark).unwrap_or(end..end);
        let bytes = utf16_byte_range(&text.text, range);
        let start = text.text[..bytes.start].encode_utf16().count();
        let value: String = value.replace("\r\n", "\n").replace('\r', "\n")
            .chars().filter(|c| !c.is_control() || *c == '\n').collect();
        text.text.replace_range(bytes, &value);
        if marked && !value.is_empty() {
            self.text_marked = Some(start..start + value.encode_utf16().count());
        }
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

    /// Re-typesets the text under the caret after its tool's settings changed: seeing the
    /// sentence change family or colour is why the picker sits in the panel.
    pub fn restyle_text(&mut self, s: &BrushSettings) {
        if let Some(t) = &mut self.text {
            t.color = s.color;
            t.opacity = s.opacity;
            t.font_size = (s.thickness * 5.0).max(16.0);
            t.font_family = s.font_family.clone();
        }
    }

    /// Esc, click-away or tool switch keeps text as a movable annotation.
    pub fn commit_text(&mut self) {
        self.text_marked = None;
        let Some(t) = self.text.take() else { return };
        let clip = self.text_clip.take();
        let bounds = t.compute_bounds();
        let before;
        if let Some(edit) = self.text_edit.take() {
            before = edit.before;
            let original = &before[edit.index];
            self.doc.texts = before.clone();
            if t == original.object {
                if let Some(region) = self.to_region(bounds.with_outset((3.0, 3.0))) { self.touched(region); }
                return;
            }
            if t.text.is_empty() { self.doc.texts.remove(edit.index); }
            else { self.doc.texts[edit.index] = TextAnnotation::new(original.id, t, clip); }
        } else {
            if t.text.is_empty() { return; }
            before = self.doc.texts.clone();
            let id = self.next_text_id;
            self.next_text_id += 1;
            self.selected_text = Some(id);
            self.doc.texts.push(TextAnnotation::new(id, t, clip));
        }
        self.doc.has_paint = true;
        self.history.push(TextCommand { before, after: self.doc.texts.clone() });
        if let Some(region) = self.to_region(bounds.with_outset((3.0, 3.0))) { self.touched(region); }
    }

    // ---- Effects and erasers --------------------------------------------------------------

    pub fn begin_region(&mut self, tool: DrawTool, p: Point, rect_mode: bool, clip: Option<Path>) {
        let stroke = (!rect_mode).then(|| {
            // A press alone leaves a dot.
            let (radius, hardness, strength) = self.stroke_profile(tool);
            SoftStroke::new((self.width(), self.height()), p, radius, hardness, strength)
        });
        let damage = stroke.as_ref().and_then(|s| s.drawn());
        self.region = Some(RegionGesture {
            tool,
            start: p,
            rect_mode,
            rect: Rect::from_xywh(p.x, p.y, 0.0, 0.0),
            clip,
            generation: 0,
            stroke,
            damage,
            frame: None,
        });
    }

    /// A stroke's radius, hardness and coverage strength. The brush is as wide as its size;
    /// effects and erasers keep the 10 px minimum they always had. Erasers scale their
    /// coverage by their opacity (that is how much they remove); the brush applies its opacity
    /// to the finished stroke instead, and an effect's opacity is its strength, not its
    /// coverage.
    fn stroke_profile(&self, tool: DrawTool) -> (f32, f32, f32) {
        let s = self.settings(tool);
        let radius = if tool == DrawTool::Brush { (s.thickness / 2.0).max(0.5) } else { s.thickness.max(10.0) / 2.0 };
        let strength = if tool.is_eraser() { s.opacity.clamp(0.0, 1.0) } else { 1.0 };
        (radius, s.hardness.clamp(0.0, 1.0), strength)
    }

    /// The live gesture's rect area (Ctrl-rect only) and coverage image, built once per change
    /// and shared by every tile the overlay repaints for it.
    fn live_frame(&mut self) -> Option<(DrawTool, Option<Path>, Option<PlacedCoverage>)> {
        let r = self.region.as_ref()?;
        if r.frame.as_ref().is_none_or(|(g, _, _)| *g != r.generation) {
            let (area, cov) = if r.rect_mode {
                let area = Path::rect(r.rect, None);
                let cov = if r.tool.is_eraser() {
                    self.to_region(r.rect).map(|at| (at, self.stroke_coverage(r, at).as_image()))
                } else {
                    None
                };
                (Some(area), cov)
            } else {
                (None, r.stroke.as_ref().and_then(|s| s.coverage()).map(|(at, b)| (at, b.as_image())))
            };
            let generation = r.generation;
            self.region.as_mut().expect("checked above").frame = Some((generation, area, cov));
        }
        let r = self.region.as_ref()?;
        let (_, area, cov) = r.frame.as_ref()?;
        Some((r.tool, area.clone(), cov.clone()))
    }

    /// The document layers as images, rebuilt only after the document changed.
    fn layer_images(&mut self) -> [Option<Image>; 3] {
        if self.layer_images.0 != self.raster_revision {
            let image = |l| self.doc.layer(l).map(Bitmap::as_image);
            let images = [image(Layer::Paint), image(Layer::Effects), image(Layer::Absolute)];
            self.layer_images = (self.raster_revision, images);
        }
        self.layer_images.1.clone()
    }

    /// The effect source for blur or pixelize at the tool's strength. Built from the base
    /// frame and cached until the strength changes.
    pub fn effect_source(&mut self, tool: DrawTool) -> &Bitmap {
        let strength = self.settings(tool).opacity.clamp(0.0, 1.0);
        let (w, h) = (self.width(), self.height());
        if tool == DrawTool::Pixelize {
            // Higher strength → bigger blocks.
            let factor = ((3.0 + strength * 11.25).round() as i32).max(2);
            if self.pixel_cache.as_ref().is_none_or(|(f, _, _)| *f != factor) {
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
                let image = big.as_image();
                self.pixel_cache = Some((factor, big, image));
            }
            return &self.pixel_cache.as_ref().expect("just built").1;
        }
        // Higher strength → bigger sigma.
        let sigma = (strength * 10.9375).max(1.0);
        if self
            .blur_cache
            .as_ref()
            .is_none_or(|(s, _, _)| (s - sigma).abs() > 0.01)
        {
            let mut out = document::new_bitmap(w, h);
            let image = self.base.as_image();
            document::with_canvas(&mut out, |c| {
                let mut p = Paint::default();
                p.set_image_filter(image_filters::blur((sigma, sigma), None, None, None));
                c.draw_image(&image, (0, 0), Some(&p));
            });
            let image = out.as_image();
            self.blur_cache = Some((sigma, out, image));
        }
        &self.blur_cache.as_ref().expect("just built").1
    }

    /// [`Self::effect_source`] as an image, made once per strength.
    fn effect_image(&mut self, tool: DrawTool) -> Image {
        self.effect_source(tool);
        let cached = if tool == DrawTool::Pixelize {
            self.pixel_cache.as_ref().map(|c| c.2.clone())
        } else {
            self.blur_cache.as_ref().map(|c| c.2.clone())
        };
        cached.expect("just built")
    }

    fn end_region(&mut self) {
        let Some(r) = self.region.take() else { return };
        let region = if r.rect_mode { self.to_region(r.rect) } else { r.stroke.as_ref().and_then(|s| s.drawn()) };
        let Some(region) = region else {
            return;
        };
        // The preview was repainted a segment at a time; whatever the commit does (an eraser
        // over nothing does nothing), the whole stroke's preview has to go.
        self.dirty = Some(match self.dirty {
            Some(d) => IRect::join(&d, &region),
            None => region,
        });
        let clip = r.clip.clone();
        match r.tool {
            DrawTool::Brush => {
                let cov = self.stroke_coverage(&r, region);
                let s = self.settings(DrawTool::Brush);
                let (color, opacity) = (s.color, s.opacity);
                self.apply_region_op(Layer::Paint, region, |d| {
                    d.paint_coverage(&cov, region.left, region.top, color, opacity, clip.as_ref())
                });
            }
            DrawTool::Blur | DrawTool::Pixelize => {
                let src = self.effect_source(r.tool).clone();
                if r.rect_mode {
                    let area = Path::rect(r.rect, None);
                    self.apply_region_op(Layer::Effects, region, |d| d.apply_effect(&src, &area, clip.as_ref()));
                } else {
                    let cov = self.stroke_coverage(&r, region);
                    self.apply_region_op(Layer::Effects, region, |d| {
                        d.apply_effect_coverage(&src, &cov, region.left, region.top, clip.as_ref())
                    });
                }
            }
            // A no-op erase must not materialise an untouched layer.
            DrawTool::FilterEraser if self.doc.has_effects => {
                let cov = self.stroke_coverage(&r, region);
                self.apply_region_op(Layer::Effects, region, |d| {
                    d.erase_effects_coverage(&cov, region.left, region.top, clip.as_ref())
                });
            }
            DrawTool::Eraser if self.doc.has_paint || !self.doc.texts.is_empty() => {
                // Raster ink and text erasure masks share one undo step. Effects are the
                // Filter Eraser's job and are left alone.
                let cov = self.stroke_coverage(&r, region);
                self.apply_region_op(Layer::Paint, region, |d| {
                    d.erase_paint_coverage(&cov, region.left, region.top, clip.as_ref())
                });
            }
            DrawTool::AbsoluteEraser => {
                let cov = self.stroke_coverage(&r, region);
                self.apply_region_op(Layer::Absolute, region, |d| {
                    d.absolute_erase_coverage(&cov, region.left, region.top, clip.as_ref())
                });
                let b = if r.rect_mode { r.rect } else { Rect::from_irect(region) };
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

    /// The gesture's coverage over `region`: an opaque greyscale bitmap whose luminance is the
    /// strength. A stroke is the coverage built while it was drawn (hardness and, for erasers,
    /// opacity included), cropped to `region`; a Ctrl-rect fills flat, an eraser's at its
    /// opacity.
    fn stroke_coverage(&self, r: &RegionGesture, region: IRect) -> Bitmap {
        let mut cov = document::new_bitmap(region.width(), region.height());
        cov.erase_color(Color::BLACK);
        document::with_canvas(&mut cov, |c| {
            c.translate((-region.left as f32, -region.top as f32));
            if r.rect_mode {
                let strength = if r.tool.is_eraser() { self.settings(r.tool).opacity.clamp(0.0, 1.0) } else { 1.0 };
                let v = (strength * 255.0).round() as u8;
                let mut fill = Paint::default();
                fill.set_color(Color::from_rgb(v, v, v));
                c.draw_rect(r.rect, &fill);
            } else if let Some((at, stamped)) = r.stroke.as_ref().and_then(|s| s.coverage()) {
                c.draw_image(stamped.as_image(), (at.left, at.top), None);
            }
        });
        cov
    }

    // ---- Painting the overlay ---------------------------------------------------------------

    /// Frame area the live gesture currently affects (stroke, shape, region, text), padded for
    /// stroke width and anti-aliasing. The view repaints the union of this before and after a
    /// change.
    pub fn live_bounds(&self) -> Option<IRect> {
        let mut out: Option<Rect> = None;
        let mut add = |r: Rect, pad: f32| {
            let r = Rect::from_ltrb(r.left - pad, r.top - pad, r.right + pad, r.bottom + pad);
            out = Some(match out {
                Some(o) => Rect::join2(o, r),
                None => r,
            });
        };
        if let Some((v, _)) = &self.shape {
            add(v.compute_bounds(), v.padding());
        }
        // Only what the latest point changed is repainted: the rest of the stroke stays put.
        if let Some(r) = &self.region {
            if r.rect_mode {
                add(r.rect, 2.0);
            } else if let Some(d) = r.damage {
                add(Rect::from_irect(d), 1.0);
            }
        }
        if let Some(t) = &self.text {
            // The caret sits past the end of the last line, a font size tall.
            add(t.compute_bounds(), t.font_size);
        }
        if self.text_drag.is_some() && let Some(bounds) = self.selected_text_bounds() { add(bounds, 3.0); }
        out.and_then(|r| self.to_region(r))
    }

    /// Paints everything the overlay shows above the dimmed frame: committed effects and
    /// paint, the live gesture's preview, the text being typed with its caret, and the
    /// checkerboard over absolute-erased areas. `canvas` is in frame coordinates; the caller
    /// clips it to the part it is repainting.
    pub fn paint(&mut self, canvas: &skia_safe::Canvas) {
        // The overlay calls this once per repainted tile: everything costly is cached per
        // document revision or per gesture step, never rebuilt here.
        let live = self.live_frame();
        let tool = live.as_ref().map(|(t, _, _)| *t);
        let area = live.as_ref().and_then(|(_, a, _)| a.clone());
        let live_cov = live.and_then(|(_, _, c)| c);
        let clip = self.region.as_ref().and_then(|r| r.clip.clone());
        let live_fx = match tool {
            Some(t) if t.is_effect() => Some(self.effect_image(t)),
            _ => None,
        };
        let [paint_img, fx_img, absolute_img] = self.layer_images();

        // A live soft eraser removes its layer's pixels inside an isolated layer, so only that
        // layer fades and the preview matches what the commit will produce.
        let draw_layer = |layer: &Image, erasing: bool| match (&live_cov, erasing) {
            (Some((reg, cov)), true) => {
                canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default());
                canvas.draw_image(layer, (0, 0), None);
                canvas.save();
                if let Some(c) = &clip {
                    canvas.clip_path(c, None, true);
                }
                let mut p = Paint::default();
                p.set_blend_mode(BlendMode::DstOut);
                p.set_color_filter(document::coverage_alpha_filter());
                canvas.draw_image(cov, (reg.left, reg.top), Some(&p));
                canvas.restore();
                canvas.restore();
            }
            _ => {
                canvas.draw_image(layer, (0, 0), None);
            }
        };

        if self.doc.has_effects
            && let Some(fx) = &fx_img
        {
            draw_layer(fx, tool == Some(DrawTool::FilterEraser));
        }

        // Blur and pixelize preview the source under the brushed area, below the paint: a
        // Ctrl-rect hard-edged, a stroke through its soft coverage.
        if let Some(src) = &live_fx {
            canvas.save();
            if let Some(c) = &clip {
                canvas.clip_path(c, None, true);
            }
            if let Some(area) = &area {
                canvas.clip_path(area, None, true);
                canvas.draw_image(src, (0, 0), None);
            } else if let Some((reg, cov)) = &live_cov {
                document::draw_through_coverage(canvas, src, cov, reg.left, reg.top);
            }
            canvas.restore();
        }

        if self.doc.has_paint
            && let Some(paint) = &paint_img
        {
            draw_layer(paint, tool == Some(DrawTool::Eraser));
        }

        // The live brush stroke: its coverage, in the brush colour, at the brush opacity.
        if tool == Some(DrawTool::Brush)
            && let Some((reg, cov)) = &live_cov
        {
            let s = self.settings(DrawTool::Brush);
            canvas.save();
            if let Some(c) = &clip {
                canvas.clip_path(c, None, true);
            }
            canvas.draw_image(cov, (reg.left, reg.top), Some(&document::tint_paint(s.color, s.opacity)));
            canvas.restore();
        }
        if let Some(v) = self.shape_preview() {
            v.draw(canvas);
        }
        if !self.doc.texts.is_empty() {
            canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default());
            self.doc.draw_texts(canvas);
            if tool == Some(DrawTool::Eraser) && let Some((region, coverage)) = &live_cov {
                canvas.save();
                if let Some(clip) = &clip { canvas.clip_path(clip, None, true); }
                let mut paint = Paint::default();
                paint.set_blend_mode(BlendMode::DstOut);
                paint.set_color_filter(document::coverage_alpha_filter());
                canvas.draw_image(coverage, (region.left, region.top), Some(&paint));
                canvas.restore();
            }
            canvas.restore();
        }
        if let Some(t) = &self.text {
            t.draw(canvas);
            draw_caret(canvas, t);
        }

        // Absolute-erased areas show a transparency checkerboard: those pixels become
        // transparent on export, and this is the on-screen cue for it.
        if self.doc.has_absolute
            && let (Some(bounds), Some(mask)) = (self.absolute_bounds, &absolute_img)
        {
            canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default().bounds(&bounds));
            let mut checker = Paint::default();
            checker.set_shader(checker_shader());
            canvas.draw_rect(bounds, &checker);
            let mut keep = Paint::default();
            keep.set_blend_mode(BlendMode::DstIn);
            canvas.draw_image(mask, (0, 0), Some(&keep));
            canvas.restore();
        }

        // The Absolute Eraser previews as a red tint whose strength follows the coverage.
        if tool == Some(DrawTool::AbsoluteEraser)
            && let Some((reg, cov)) = &live_cov
        {
            let r = Rect::from_irect(*reg);
            canvas.save();
            if let Some(c) = &clip {
                canvas.clip_path(c, None, true);
            }
            canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default().bounds(&r));
            let mut red = Paint::default();
            red.set_color(Color::from_argb(0xB0, 0xFF, 0x2D, 0x55));
            canvas.draw_rect(r, &red);
            let mut keep = Paint::default();
            keep.set_blend_mode(BlendMode::DstIn);
            keep.set_color_filter(document::coverage_alpha_filter());
            canvas.draw_image(cov, (reg.left, reg.top), Some(&keep));
            canvas.restore();
            canvas.restore();
        }
    }

    // ---- Gesture plumbing -------------------------------------------------------------------

    pub fn is_drawing(&self) -> bool {
        self.shape.is_some() || self.region.is_some() || self.text_drag.is_some()
    }

    /// Mouse up: commits whatever gesture is live.
    pub fn end_gesture(&mut self) {
        self.end_text_drag();
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

fn utf16_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let mut units = 0;
    let mut start = text.len();
    let mut end = text.len();
    for (byte, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if range.start < next && start == text.len() { start = byte; }
        if range.end <= units { end = byte; break; }
        units = next;
    }
    start.min(end)..end
}

/// A caret at the end of the text being typed, measured with the same typeface the glyphs
/// are drawn with so it does not drift off the line for other families.
fn draw_caret(canvas: &skia_safe::Canvas, t: &VectorObject) {
    let font = skia_safe::Font::from_typeface(crate::fonts::resolve(&t.font_family), t.font_size);
    let lines: Vec<&str> = t.text.split('\n').collect();
    let last = lines.last().copied().unwrap_or("");
    let (spacing, _) = font.metrics();
    let x = t.p1.x + font.measure_str(last, None).0;
    let y = t.p1.y + (lines.len() - 1) as f32 * spacing;
    let mut p = Paint::default();
    p.set_color(t.color);
    p.set_stroke_width((t.font_size / 16.0).max(1.5));
    p.set_anti_alias(true);
    // Ascent to descent, the line's full height, so the caret's middle is the line's.
    canvas.draw_line((x + 1.0, y), (x + 1.0, y + t.line_height()), &p);
}

/// A repeating grey checkerboard: the "transparent" indicator.
fn checker_shader() -> Option<skia_safe::Shader> {
    const CELL: i32 = 8;
    let mut tile = document::new_bitmap(CELL * 2, CELL * 2);
    tile.erase_color(Color::from_rgb(0xC8, 0xC8, 0xC8));
    document::with_canvas(&mut tile, |c| {
        let mut dark = Paint::default();
        dark.set_color(Color::from_rgb(0x80, 0x80, 0x80));
        c.draw_rect(Rect::from_xywh(0.0, 0.0, CELL as f32, CELL as f32), &dark);
        c.draw_rect(
            Rect::from_xywh(CELL as f32, CELL as f32, CELL as f32, CELL as f32),
            &dark,
        );
    });
    tile.as_image().to_shader(
        (TileMode::Repeat, TileMode::Repeat),
        SamplingOptions::default(),
        None,
    )
}

fn update_shape(v: &mut VectorObject, start: Point, cur: Point) {
    if matches!(v.kind, VectorKind::Line | VectorKind::Arrow) {
        v.p2 = cur;
    } else {
        v.bounds = Rect::from_ltrb(
            start.x.min(cur.x),
            start.y.min(cur.y),
            start.x.max(cur.x),
            start.y.max(cur.y),
        );
        // The triangle points the way you drag.
        if v.kind == VectorKind::Triangle {
            v.points_up = cur.y <= start.y;
        }
    }
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
        e.begin_brush(
            Point::new(10.0, 50.0),
            Some(Path::rect(Rect::from_ltrb(0.0, 0.0, 40.0, 100.0), None)),
        );
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
    fn text_is_centred_on_the_click() {
        let mut e = editor();
        e.begin_text(Point::new(50.0, 60.0), None);
        e.type_str("Hi");
        let b = e.text_editing().unwrap().compute_bounds();
        assert_eq!(b.left, 50.0);
        assert!((b.center_y() - 60.0).abs() < 0.5, "line middle at {}", b.center_y());
    }

    #[test]
    fn typed_text_is_retained_on_commit() {
        let mut e = editor();
        e.begin_text(Point::new(5.0, 40.0), None);
        e.type_str("Hi\u{8}");
        e.type_newline();
        e.type_str("x");
        e.backspace();
        assert_eq!(e.text_editing().unwrap().text, "Hi\n");
        e.commit_text();
        assert!(e.document().has_paint);
        assert!(!e.is_typing());
        assert_eq!(e.document().texts().len(), 1);
    }

    #[test]
    fn text_drag_keeps_grab_offset_and_is_one_undo_step() {
        let mut e = editor();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.type_str("Hi");
        e.commit_text();
        let initial = e.document().texts()[0].object.p1;
        let grab = Point::new(initial.x + 3.0, initial.y + 4.0);
        assert!(e.begin_text_drag(grab));
        e.extend(Point::new(grab.x + 10.0, grab.y + 7.0));
        e.extend(Point::new(grab.x + 25.0, grab.y + 12.0));
        e.end_gesture();
        let moved = Point::new(initial.x + 25.0, initial.y + 12.0);
        assert_eq!(e.document().texts()[0].object.p1, moved);
        assert!(e.undo());
        assert_eq!(e.document().texts()[0].object.p1, initial);
        assert!(e.undo());
        assert!(e.document().texts().is_empty());
        assert!(e.redo());
        assert!(e.redo());
        assert_eq!(e.document().texts()[0].object.p1, moved);
    }

    #[test]
    fn moving_text_preserves_underlying_ink_and_export_matches_preview() {
        let mut e = editor();
        e.begin_brush(Point::new(0.0, 50.0), None);
        e.extend(Point::new(100.0, 50.0));
        e.end_gesture();
        let ink = document::pixels(e.document().layer(Layer::Paint).unwrap()).to_vec();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.type_str("Hi");
        e.commit_text();
        let start = e.document().texts()[0].object.p1;
        assert!(e.begin_text_drag(Point::new(start.x + 2.0, start.y + 2.0)));
        e.extend(Point::new(start.x + 32.0, start.y + 22.0));
        e.end_gesture();
        assert_eq!(document::pixels(e.document().layer(Layer::Paint).unwrap()), ink);
        // A bitmap clone shares pixels: allocate a separate composition target instead.
        let mut preview = document::new_bitmap(e.width(), e.height());
        let base = e.base().as_image();
        document::with_canvas(&mut preview, |canvas| {
            canvas.draw_image(&base, (0, 0), None);
            e.paint(canvas);
        });
        let exported = e.document().render_composite(e.base());
        assert_eq!(document::pixels(&preview), document::pixels(&exported));
    }

    #[test]
    fn erased_text_stays_erased_after_a_move_and_undo_restores_it() {
        let mut e = editor();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.type_str("Hi");
        e.commit_text();
        e.settings_mut(DrawTool::Eraser).opacity = 1.0;
        e.begin_region(DrawTool::Eraser, Point::new(0.0, 0.0), true, None);
        e.extend(Point::new(100.0, 100.0));
        e.end_gesture();
        let all_transparent = |e: &Editor| document::pixels(&e.document().render_overlay()).chunks_exact(4).all(|p| p[3] == 0);
        assert!(all_transparent(&e));
        let start = e.document().texts()[0].object.p1;
        assert!(e.begin_text_drag(Point::new(start.x + 2.0, start.y + 2.0)));
        e.extend(Point::new(start.x + 22.0, start.y + 12.0));
        e.end_gesture();
        assert!(all_transparent(&e));
        assert!(e.undo());
        assert!(all_transparent(&e));
        assert!(e.undo());
        assert!(!all_transparent(&e));
    }

    #[test]
    fn double_click_edit_reuses_the_object_and_undo_restores_the_old_text() {
        let mut e = editor();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.type_str("Hi");
        e.commit_text();
        let start = e.document().texts()[0].object.p1;
        let id = e.document().texts()[0].id;
        assert!(e.edit_text_at(Point::new(start.x + 2.0, start.y + 2.0)));
        e.type_str("!");
        e.commit_text();
        assert_eq!(e.document().texts().len(), 1);
        assert_eq!(e.document().texts()[0].id, id);
        assert_eq!(e.document().texts()[0].object.text, "Hi!");
        assert!(e.undo());
        assert_eq!(e.document().texts()[0].object.text, "Hi");
    }

    #[test]
    fn partial_text_erasure_moves_with_the_glyphs() {
        let mut e = editor();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.type_str("Hi");
        e.commit_text();
        let original = e.document().render_overlay();
        e.begin_region(DrawTool::Eraser, Point::new(0.0, 0.0), true, None);
        e.extend(Point::new(20.0, 100.0));
        e.end_gesture();
        let erased = e.document().render_overlay();
        assert_ne!(document::pixels(&original), document::pixels(&erased));
        assert!(document::pixels(&erased).chunks_exact(4).any(|p| p[3] != 0));
        let start = e.document().texts()[0].object.p1;
        assert!(e.begin_text_drag(Point::new(start.x + 2.0, start.y + 2.0)));
        e.extend(Point::new(start.x + 32.0, start.y + 2.0));
        e.end_gesture();
        let moved = e.document().render_overlay();
        for y in 0..100usize {
            for x in 0..70usize {
                let before = y * erased.row_bytes() + x * 4;
                let after = y * moved.row_bytes() + (x + 30) * 4;
                assert_eq!(&document::pixels(&erased)[before..before + 4], &document::pixels(&moved)[after..after + 4]);
            }
        }
    }

    #[test]
    fn clicking_text_without_moving_does_not_add_an_undo_step() {
        let mut e = editor();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.type_str("Hi");
        e.commit_text();
        let p = e.document().texts()[0].object.p1;
        assert!(e.begin_text_drag(Point::new(p.x + 2.0, p.y + 2.0)));
        e.end_gesture();
        assert!(e.undo());
        assert!(e.document().texts().is_empty());
    }

    #[test]
    fn native_input_handles_surrogate_pairs_and_replaces_ime_composition() {
        let mut e = editor();
        e.begin_text(Point::new(10.0, 35.0), None);
        e.replace_native_text(None, "A😀B", false);
        let (emoji, actual) = e.text_for_range_utf16(2..3).unwrap();
        assert_eq!(emoji, "😀");
        assert_eq!(actual, 1..3);
        e.replace_native_text(Some(1..3), "Ж", false);
        assert_eq!(e.text_editing().unwrap().text, "AЖB");
        e.replace_native_text(None, "漢", true);
        e.replace_native_text(None, "漢字", true);
        e.replace_native_text(None, "漢字", false);
        assert_eq!(e.text_editing().unwrap().text, "AЖB漢字");
        assert!(e.marked_text_range().is_none());
        e.replace_native_text(None, "\r\nnext", false);
        assert_eq!(e.text_editing().unwrap().text, "AЖB漢字\nnext");
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
        assert_eq!(
            e.absolute_bounds,
            Some(Rect::from_ltrb(10.0, 10.0, 30.0, 40.0))
        );
        assert!(e.document().has_absolute);
    }

    #[test]
    fn sampling_reads_the_composite() {
        let e = editor();
        assert_eq!(
            e.sample_color(Point::new(3.0, 3.0)),
            Color::from_rgb(0, 128, 0)
        );
    }

    #[test]
    fn painting_shows_committed_and_live_ink() {
        let mut e = editor();
        e.begin_brush(Point::new(10.0, 50.0), None);
        e.extend(Point::new(90.0, 50.0));
        let live = e.live_bounds().unwrap();
        assert!(live.left <= 10 && live.top <= 48 && live.right >= 90 && live.bottom >= 52);
        let mut out = document::new_bitmap(100, 100);
        document::with_canvas(&mut out, |c| e.paint(c));
        assert_eq!(
            document::pixels(&out)[50 * out.row_bytes() + 50 * 4 + 3],
            255
        );
        e.end_gesture();
        assert!(e.live_bounds().is_none());
    }

    #[test]
    fn long_eraser_stroke_erases_along_its_whole_length() {
        let base = document::new_bitmap(1200, 100);
        base.erase_color(Color::from_rgb(0, 128, 0));
        let mut e = Editor::new(base);
        e.begin_brush(Point::new(0.0, 50.0), None);
        e.extend(Point::new(1200.0, 50.0));
        e.end_gesture();
        assert!(paint_alpha(&e, 600, 50) > 0);
        e.settings_mut(DrawTool::Eraser).hardness = 1.0;
        e.begin_region(DrawTool::Eraser, Point::new(20.0, 50.0), false, None);
        // Far past the first allocation and its margin: the coverage has to grow, and what
        // was stamped before each growth must survive it.
        for x in (40..=1000).step_by(20) {
            e.extend(Point::new(x as f32, 50.0));
        }
        e.end_gesture();
        for x in [30, 300, 600, 990] {
            assert_eq!(paint_alpha(&e, x, 50), 0, "erased at x = {x}");
        }
        assert!(paint_alpha(&e, 1150, 50) > 0, "past the stroke's end the paint survives");
    }

    #[test]
    fn live_eraser_preview_hides_paint_before_commit() {
        let mut e = editor();
        e.begin_brush(Point::new(0.0, 50.0), None);
        e.extend(Point::new(100.0, 50.0));
        e.end_gesture();
        e.begin_region(DrawTool::Eraser, Point::new(0.0, 0.0), true, None);
        e.extend(Point::new(100.0, 100.0));
        let mut out = document::new_bitmap(100, 100);
        document::with_canvas(&mut out, |c| e.paint(c));
        assert_eq!(document::pixels(&out)[50 * out.row_bytes() + 50 * 4 + 3], 0);
    }

    #[test]
    fn brush_hardness_softens_only_the_rim() {
        let paint_at = |hardness: f32| {
            let mut e = editor();
            e.settings_mut(DrawTool::Brush).thickness = 20.0;
            e.settings_mut(DrawTool::Brush).hardness = hardness;
            e.begin_brush(Point::new(10.0, 50.0), None);
            e.extend(Point::new(90.0, 50.0));
            e.end_gesture();
            (paint_alpha(&e, 50, 50), paint_alpha(&e, 50, 57))
        };
        assert_eq!(paint_at(1.0), (255, 255), "hard: solid out to the rim");
        let (centre, rim) = paint_at(0.0);
        // Hardness 0 has no solid core at all, as in hive: the centre pixel, 0.5 px off the
        // line, is already a hair below full.
        assert!(centre >= 250, "the centre line stays (almost) full ({centre})");
        assert!(rim < 100, "soft: the rim fades ({rim})");
    }

    #[test]
    fn brush_opacity_applies_once_to_the_whole_stroke() {
        let mut e = editor();
        e.settings_mut(DrawTool::Brush).thickness = 20.0;
        e.settings_mut(DrawTool::Brush).opacity = 0.5;
        e.begin_brush(Point::new(10.0, 50.0), None);
        e.extend(Point::new(90.0, 50.0));
        e.extend(Point::new(10.0, 50.0));
        e.end_gesture();
        assert_eq!(paint_alpha(&e, 50, 50), 128, "crossing itself does not build up");
    }

    #[test]
    fn eraser_opacity_removes_that_fraction() {
        let mut e = editor();
        e.settings_mut(DrawTool::Brush).thickness = 40.0;
        e.begin_brush(Point::new(0.0, 50.0), None);
        e.extend(Point::new(100.0, 50.0));
        e.end_gesture();
        e.settings_mut(DrawTool::Eraser).opacity = 0.5;
        e.settings_mut(DrawTool::Eraser).hardness = 1.0;
        e.begin_region(DrawTool::Eraser, Point::new(20.0, 50.0), false, None);
        e.extend(Point::new(80.0, 50.0));
        e.end_gesture();
        let a = paint_alpha(&e, 50, 50);
        assert!((126..=129).contains(&a), "half erased ({a})");
    }

    #[test]
    fn soft_blur_fades_into_the_frame() {
        let mut e = editor();
        e.settings_mut(DrawTool::Blur).thickness = 40.0;
        e.settings_mut(DrawTool::Blur).hardness = 0.0;
        e.begin_region(DrawTool::Blur, Point::new(20.0, 50.0), false, None);
        e.extend(Point::new(80.0, 50.0));
        e.end_gesture();
        let fx = e.document().layer(Layer::Effects).unwrap();
        let alpha = |x: usize, y: usize| document::pixels(fx)[y * fx.row_bytes() + x * 4 + 3];
        assert_eq!(alpha(50, 50), 255);
        let rim = alpha(50, 66);
        assert!(rim > 0 && rim < 128, "the rim is partial ({rim})");
    }

    #[test]
    fn tool_settings_are_per_slot() {
        let mut e = editor();
        e.settings_mut(DrawTool::Square).thickness = 20.0;
        assert_eq!(e.settings(DrawTool::Circle).thickness, 20.0);
        assert_eq!(e.settings(DrawTool::Brush).thickness, 5.0);
    }
}
