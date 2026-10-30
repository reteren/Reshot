//! The editing session behind the overlay, free of any UI: the document, its undo history, the
//! per-tool settings, and every drawing gesture (brush, shapes, text, effects, erasers). The
//! overlay feeds it points in frame pixels and draws what it reports.

use std::collections::HashMap;

use skia_safe::gradient::{Colors, Gradient, Interpolation, shaders};
use skia_safe::{
    Bitmap, BlendMode, Color, Color4f, FilterMode, IRect, Image, MipmapMode, Paint, PaintCap,
    PaintJoin, PaintStyle, Path, PathBuilder, Point, Rect, SamplingOptions, TileMode,
    image_filters,
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

/// A region gesture in progress: brushed path or Ctrl-dragged rect.
struct RegionGesture {
    tool: DrawTool,
    start: Point,
    rect_mode: bool,
    rect: Rect,
    path: PathBuilder,
    clip: Option<Path>,
    /// Bumped by every new point, so the per-frame cache below knows when it is stale.
    generation: u64,
    /// The latest segment: all a move changes on screen.
    prev: Point,
    last: Point,
    /// Soft-eraser coverage built stamp by stamp as the path grows (brush mode). A segment's
    /// stamps depend on that segment alone, so a move adds only its own instead of
    /// re-stamping the whole stroke, and the commit uses exactly what the preview showed.
    coverage: Option<(IRect, Bitmap)>,
    /// What every tile of one frame shares: the area and the coverage image at `generation`.
    frame: Option<(u64, Path, Option<(IRect, Image)>)>,
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
    blur_cache: Option<(f32, Bitmap, Image)>,
    pixel_cache: Option<(i32, Bitmap, Image)>,
    /// The layers as images, at `revision`. A mutable bitmap's `as_image` copies all of it,
    /// and the overlay paints tile by tile: per tile, per frame, that was 25 MB per layer.
    layer_images: (u64, [Option<Image>; 3]),
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
            layer_images: (u64::MAX, [None, None, None]),
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
        if let Some(tool) = self.region.as_ref().map(|r| r.tool) {
            let profile = self.stamp_profile(tool);
            let frame = (self.width(), self.height());
            let r = self.region.as_mut().expect("checked above");
            r.generation += 1;
            if r.rect_mode {
                r.rect = Rect::from_ltrb(
                    r.start.x.min(p.x),
                    r.start.y.min(p.y),
                    r.start.x.max(p.x),
                    r.start.y.max(p.y),
                );
            } else {
                r.path.line_to(p);
                (r.prev, r.last) = (r.last, p);
                if tool.is_eraser() {
                    let points = segment_stamps(r.prev, p, profile.0);
                    stamp(&mut r.coverage, frame, &points, profile);
                }
            }
        }
    }

    /// The live brush stroke and its paint, for the preview.
    pub fn brush_preview(&self) -> Option<(Path, Paint, Option<&Path>)> {
        self.brush
            .as_ref()
            .map(|b| (b.stroke.path(), b.stroke.paint(), b.clip.as_ref()))
    }

    fn end_brush(&mut self) {
        let Some(BrushGesture { stroke, clip }) = self.brush.take() else {
            return;
        };
        if stroke.is_empty() {
            return;
        }
        let path = stroke.path();
        let pad = stroke.settings.thickness / 2.0 + 2.0;
        let b = path.bounds();
        let Some(region) = self.to_region(Rect::from_ltrb(
            b.left - pad,
            b.top - pad,
            b.right + pad,
            b.bottom + pad,
        )) else {
            return;
        };
        let paint = stroke.paint();
        self.apply_region_op(Layer::Paint, region, |d| {
            d.commit_stroke(&path, &paint, clip.as_ref())
        });
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
        let Some((v, _)) = self.shape.take() else {
            return;
        };
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

    /// Re-typesets the text under the caret after its tool's settings changed: seeing the
    /// sentence change family or colour is why the picker sits in the panel.
    pub fn restyle_text(&mut self, s: &BrushSettings) {
        if let Some(t) = &mut self.text {
            t.color = s.color;
            t.font_size = (s.thickness * 5.0).max(16.0);
            t.font_family = s.font_family.clone();
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
        let mut coverage = None;
        if tool.is_eraser() && !rect_mode {
            let profile = self.stamp_profile(tool);
            stamp(&mut coverage, (self.width(), self.height()), &[p], profile);
        }
        self.region = Some(RegionGesture {
            tool,
            start: p,
            rect_mode,
            rect: Rect::from_xywh(p.x, p.y, 0.0, 0.0),
            path,
            clip,
            generation: 0,
            prev: p,
            last: p,
            coverage,
            frame: None,
        });
    }

    /// An eraser stamp's radius and rim strength (the centre is always full).
    fn stamp_profile(&self, tool: DrawTool) -> (f32, f32) {
        let s = self.settings(tool);
        let rim = (s.hardness.clamp(0.0, 1.0) * 255.0).round() / 255.0;
        (s.thickness.max(10.0) / 2.0, rim)
    }

    /// The live region's area and coverage image, built once per change and shared by every
    /// tile the overlay repaints for it.
    fn live_frame(&mut self) -> Option<(DrawTool, Path, Option<(IRect, Image)>)> {
        let r = self.region.as_ref()?;
        if r.frame.as_ref().is_none_or(|(g, _, _)| *g != r.generation) {
            let area = self.region_area(r);
            let cov = match (r.tool.is_eraser(), r.rect_mode) {
                (false, _) => None,
                (true, false) => r.coverage.as_ref().map(|(at, b)| (*at, b.as_image())),
                (true, true) => self
                    .to_region(*area.bounds())
                    .map(|at| (at, self.erase_coverage(r, at).as_image())),
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
        if self.layer_images.0 != self.revision {
            let image = |l| self.doc.layer(l).map(Bitmap::as_image);
            let images = [image(Layer::Paint), image(Layer::Effects), image(Layer::Absolute)];
            self.layer_images = (self.revision, images);
        }
        self.layer_images.1.clone()
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
        let area = self.region_area(&r);
        let Some(region) = self.to_region(*area.bounds()) else {
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
            DrawTool::Blur | DrawTool::Pixelize => {
                let src = self.effect_source(r.tool).clone();
                self.apply_region_op(Layer::Effects, region, |d| {
                    d.apply_effect(&src, &area, clip.as_ref())
                });
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
    /// erase strength. Brush mode is the coverage stamped while the stroke was drawn (radial
    /// discs, centre 100%, rim = hardness, combined with Lighten so overlaps never compound
    /// past the profile), cropped to `region`. Rect mode fills flat at full strength.
    fn erase_coverage(&self, r: &RegionGesture, region: IRect) -> Bitmap {
        let mut cov = document::new_bitmap(region.width(), region.height());
        cov.erase_color(Color::BLACK);
        document::with_canvas(&mut cov, |c| {
            c.translate((-region.left as f32, -region.top as f32));
            if r.rect_mode {
                let mut fill = Paint::default();
                fill.set_color(Color::WHITE);
                c.draw_rect(r.rect, &fill);
            } else if let Some((at, stamped)) = &r.coverage {
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
        if let Some(b) = &self.brush {
            add(
                *b.stroke.path().bounds(),
                b.stroke.settings.thickness / 2.0 + 2.0,
            );
        }
        if let Some((v, _)) = &self.shape {
            add(v.compute_bounds(), v.thickness / 2.0 + 44.0);
        }
        // Only the latest segment changes on screen: earlier stamps and outline stay put.
        if let Some(r) = &self.region {
            if r.rect_mode {
                add(r.rect, 2.0);
            } else {
                let segment = Rect::from_ltrb(
                    r.prev.x.min(r.last.x),
                    r.prev.y.min(r.last.y),
                    r.prev.x.max(r.last.x),
                    r.prev.y.max(r.last.y),
                );
                add(segment, self.settings(r.tool).thickness.max(10.0) / 2.0 + 2.0);
            }
        }
        if let Some(t) = &self.text {
            // The caret sits past the end of the last line, a font size tall.
            add(t.compute_bounds(), t.font_size);
        }
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
        let area = live.as_ref().map(|(_, a, _)| a.clone());
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

        // Blur and pixelize preview the source under the brushed area, below the paint.
        if let (Some(src), Some(area)) = (&live_fx, &area) {
            canvas.save();
            if let Some(c) = &clip {
                canvas.clip_path(c, None, true);
            }
            canvas.clip_path(area, None, true);
            canvas.draw_image(src, (0, 0), None);
            canvas.restore();
        }

        if self.doc.has_paint
            && let Some(paint) = &paint_img
        {
            draw_layer(paint, tool == Some(DrawTool::Eraser));
        }

        if let Some((path, paint, clip)) = self.brush_preview() {
            canvas.save();
            if let Some(c) = clip {
                canvas.clip_path(c, None, true);
            }
            canvas.draw_path(&path, &paint);
            canvas.restore();
        }
        if let Some(v) = self.shape_preview() {
            v.draw(canvas);
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
    canvas.draw_line((x + 1.0, y), (x + 1.0, y + t.font_size), &p);
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

/// Stamp centres along one segment, `radius * 0.35` apart, the end included and the start
/// left to the previous segment: the points [`flatten_polyline`] gives that segment.
fn segment_stamps(a: Point, b: Point, radius: f32) -> Vec<Point> {
    let spacing = (radius * 0.35).max(1.0);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let steps = (((dx * dx + dy * dy).sqrt() / spacing) as i32).max(1);
    (1..=steps)
        .map(|s| {
            let t = s as f32 / steps as f32;
            Point::new(a.x + dx * t, a.y + dy * t)
        })
        .collect()
}

/// Lays radial eraser stamps into a coverage bitmap that grows to hold them. It grows with a
/// margin, so a stroke reallocates a handful of times rather than on every move.
fn stamp(
    coverage: &mut Option<(IRect, Bitmap)>,
    frame: (i32, i32),
    points: &[Point],
    (radius, rim): (f32, f32),
) {
    const MARGIN: i32 = 128;
    let Some(first) = points.first() else { return };
    let mut need = Rect::from_xywh(first.x, first.y, 0.0, 0.0);
    for p in points {
        need.join(Rect::from_xywh(p.x, p.y, 0.0, 0.0));
    }
    let need = IRect::from_ltrb(
        (need.left - radius - 1.0).floor() as i32,
        (need.top - radius - 1.0).floor() as i32,
        (need.right + radius + 1.0).ceil() as i32,
        (need.bottom + radius + 1.0).ceil() as i32,
    );
    let frame_rect = IRect::from_wh(frame.0, frame.1);
    let Some(need) = IRect::intersect(&need, &frame_rect) else {
        return;
    };
    if !coverage.as_ref().is_some_and(|(at, _)| at.contains(need)) {
        let grown = match coverage.as_ref() {
            Some((at, _)) => IRect::join(at, &need),
            None => need,
        };
        let grown = IRect::from_ltrb(
            grown.left - MARGIN,
            grown.top - MARGIN,
            grown.right + MARGIN,
            grown.bottom + MARGIN,
        );
        let grown = IRect::intersect(&grown, &frame_rect).unwrap_or(need);
        let mut bitmap = document::new_bitmap(grown.width(), grown.height());
        bitmap.erase_color(Color::BLACK);
        if let Some((at, old)) = coverage.as_ref() {
            document::with_canvas(&mut bitmap, |c| {
                c.draw_image(old.as_image(), (at.left - grown.left, at.top - grown.top), None);
            });
        }
        *coverage = Some((grown, bitmap));
    }
    let (at, bitmap) = coverage.as_mut().expect("allocated above");
    let at = *at;
    let stops = [Color4f::new(1.0, 1.0, 1.0, 1.0), Color4f::new(rim, rim, rim, 1.0)];
    let positions = [0.0f32, 1.0];
    let gradient = Gradient::new(
        Colors::new(&stops, Some(&positions), TileMode::Clamp, None),
        Interpolation::default(),
    );
    document::with_canvas(bitmap, |c| {
        c.translate((-at.left as f32, -at.top as f32));
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(BlendMode::Lighten);
        for pt in points {
            paint.set_shader(shaders::radial_gradient((*pt, radius), &gradient, None));
            c.draw_circle(*pt, radius, &paint);
        }
    });
}

/// Samples a polyline into points spaced about `spacing` apart.
fn flatten_polyline(path: &Path, spacing: f32) -> Vec<Point> {
    let verts = path.points();
    let mut out = Vec::new();
    let Some(first) = verts.first() else {
        return out;
    };
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
    fn tool_settings_are_per_slot() {
        let mut e = editor();
        e.settings_mut(DrawTool::Square).thickness = 20.0;
        assert_eq!(e.settings(DrawTool::Circle).thickness, 20.0);
        assert_eq!(e.settings(DrawTool::Brush).thickness, 5.0);
    }
}
