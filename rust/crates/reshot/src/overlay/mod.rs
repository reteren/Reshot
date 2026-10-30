//! The capture session window (SPEC §3-§12): one borderless topmost window over the whole
//! virtual desktop showing the frozen frame dimmed, with the selection cut out at full
//! brightness, the toolbar, and every editing tool.
//!
//! Coordinates: selections and the document live in frame pixels, (0,0) being the virtual
//! desktop's top-left. gpui lays out in logical pixels; `scale` converts. Tolerances the C#
//! build expressed in DIPs are multiplied by `scale`.
//!
//! Input routing, highest priority first (ARCHITECTURE §8): open panels and flyouts (their
//! elements occlude the canvas, so the canvas never sees those clicks), context modifiers,
//! then the active tool.

mod chrome;
mod frame_image;
mod ocr;
mod panel;
mod surface;
mod toolbar;
mod view;

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, KeyDownEvent, Modifiers, ModifiersChangedEvent, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage, Window, WindowHandle,
};
use reshot_capture::CapturedFrame;
use reshot_core::editor::{DrawTool, Editor};
use reshot_core::export::{self, ImageFormat, Raster};
use reshot_core::selection::{self, Handle, RatioLock, Selection, SelectionSet, ShapeKind};
use reshot_core::settings::AppSettings;
use reshot_core::skia::{Color, IRect, Path, Point as SkPoint, Rect};
use reshot_platform::win::{self, PxRect};

use crate::{app, winutil};

pub use panel::PanelState;

/// Resize handle size in DIPs.
const HANDLE_SIZE: f32 = 9.0;
/// Width of the move band along a selection's edge, in DIPs.
const EDGE_TOLERANCE: f32 = 8.0;
/// Clicking this close to a polygon's first vertex closes it, in DIPs.
const POLYGON_CLOSE: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Select,
    Draw(DrawTool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    New,
    Move(Rect),
    Resize(Handle, Rect),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flyout {
    Select,
    Shapes,
    Lines,
    Eraser,
    Effects,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtrlA {
    None,
    Primary,
    All,
}

pub struct FlyoutState {
    pub kind: Flyout,
    pub by_hover: bool,
    pub opened: Instant,
}

pub struct Overlay {
    focus: FocusHandle,
    frame_image: Arc<RenderImage>,
    editor: Editor,
    surface: surface::Surface,
    settings: AppSettings,
    /// Virtual-desktop origin of the frame.
    origin: (i32, i32),
    /// Monitors in frame pixels, with the primary flag.
    monitors: Vec<(Rect, bool)>,
    scale: f32,

    sel: SelectionSet,
    shape: ShapeKind,
    mode: Mode,
    shapes_sub: DrawTool,
    lines_sub: DrawTool,
    eraser_sub: DrawTool,
    effects_sub: DrawTool,

    drag: Drag,
    drag_start: SkPoint,
    ratio: RatioLock,
    lasso: Vec<SkPoint>,
    polygon: Option<Vec<SkPoint>>,
    polygon_cursor: SkPoint,
    ctrl_a: CtrlA,
    /// Last cursor position in frame pixels, for the brush ring.
    cursor: Option<SkPoint>,
    shift_down: bool,

    pub(crate) flyout: Option<FlyoutState>,
    flyout_hovered: bool,
    hover_generation: u64,
    source_menu: Option<toolbar::SourceMenu>,
    /// Pointer position while idle in Select mode, for the hover cursor.
    hover: Option<SkPoint>,
    pub(crate) panel: PanelState,

    eyedropping: bool,
    eyedropper_armed: bool,
    eyedrop: Option<(SkPoint, Color)>,

    ocr: ocr::OcrState,
    notice: Option<String>,
    /// True while the window is hidden for a clipboard write: an invisible overlay is then
    /// a legitimate state, not a stuck one.
    pub exporting: bool,
    closing: bool,
}

impl Overlay {
    pub fn open(frame: CapturedFrame, settings: AppSettings, cx: &mut App) -> Option<WindowHandle<Overlay>> {
        let r = PxRect {
            left: frame.virtual_left,
            top: frame.virtual_top,
            right: frame.virtual_left + frame.width as i32,
            bottom: frame.virtual_top + frame.height as i32,
        };
        let scale = winutil::primary_scale(cx);
        let base = export::bitmap_from_bgra(frame.width as i32, frame.height as i32, frame.stride, &frame.pixels_bgra);
        let frame_image = frame_image::render_image(&frame);
        let origin = (frame.virtual_left, frame.virtual_top);
        let monitors = frame
            .monitors
            .iter()
            .map(|m| {
                let l = (m.left - origin.0) as f32;
                let t = (m.top - origin.1) as f32;
                (Rect::from_xywh(l, t, m.width as f32, m.height as f32), m.is_primary)
            })
            .collect();
        drop(frame);
        reshot_core::fonts::warmup();
        // Decided before the window appears: afterwards the foreground is ours and the answer
        // is always no. Every unlock and input-queue trick exists for the game case alone.
        let game = win::foreground_is_fullscreen_foreign_window();
        if game {
            // After the frame is frozen, so the capture still saw the game.
            win::unlock_input_from_game();
        }

        let handle = cx
            .open_window(winutil::popup_options(winutil::logical(r, scale), true), move |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus);
                    let mut overlay = Overlay {
                        focus,
                        frame_image,
                        editor: Editor::new(base),
                        surface: Default::default(),
                        settings,
                        origin,
                        monitors,
                        scale,
                        sel: SelectionSet::default(),
                        shape: ShapeKind::Rectangle,
                        mode: Mode::Select,
                        shapes_sub: DrawTool::Square,
                        lines_sub: DrawTool::Arrow,
                        eraser_sub: DrawTool::Eraser,
                        effects_sub: DrawTool::Pixelize,
                        drag: Drag::None,
                        drag_start: SkPoint::default(),
                        ratio: RatioLock::default(),
                        lasso: Vec::new(),
                        polygon: None,
                        polygon_cursor: SkPoint::default(),
                        ctrl_a: CtrlA::None,
                        cursor: None,
                        shift_down: false,
                        flyout: None,
                        flyout_hovered: false,
                        hover_generation: 0,
                        source_menu: None,
                        hover: None,
                        panel: PanelState::default(),
                        eyedropping: false,
                        eyedropper_armed: false,
                        eyedrop: None,
                        ocr: Default::default(),
                        notice: None,
                        exporting: false,
                        closing: false,
                    };
                    overlay.panel.load(overlay.editor.settings(DrawTool::Brush));
                    overlay
                })
            })
            .map_err(|e| log::error!("Overlay: failed to open: {e}"))
            .ok()?;

        // Placement and focus run after this update returns; see `winutil::after_update`.
        let (hwnd, hide) = handle
            .update(cx, |this, window, _| {
                this.scale = window.scale_factor();
                (winutil::hwnd(window), this.settings.overlay.hide_from_capture)
            })
            .ok()?;
        if let Some(h) = hwnd {
            let raw = h.0 as isize;
            winutil::after_update(cx, move || {
                let h = win::hwnd(raw);
                win::place_topmost(h, r);
                if hide {
                    win::exclude_from_capture(h, true);
                }
                // The invasive tier gets its single shot here, over a game only.
                let won = win::force_foreground(h, game);
                log::info!("Overlay shown over {}; foreground = {}.", if game { "a game" } else { "desktop" }, if won { "ours" } else { "theirs" });
            });
            if game {
                winutil::keep_over_game(cx, handle, raw);
            }
        }
        Some(handle)
    }

    // ---- Small helpers -------------------------------------------------------------------

    fn to_frame(&self, p: Point<Pixels>) -> SkPoint {
        SkPoint::new(f32::from(p.x) * self.scale, f32::from(p.y) * self.scale)
    }

    fn tol(&self, dip: f32) -> f32 {
        dip * self.scale
    }

    fn frame_rect(&self) -> Rect {
        Rect::from_wh(self.editor.width() as f32, self.editor.height() as f32)
    }

    fn clip(&self) -> Option<Path> {
        self.sel.clip_path()
    }

    fn has_selection(&self) -> bool {
        !self.sel.is_empty()
    }

    /// The monitor a rect mostly lives on: where its chrome goes. Falls back to the whole
    /// frame in the gaps between misaligned screens.
    fn anchor_screen(&self, r: Rect) -> Rect {
        let mut best = (0.0, self.frame_rect());
        for (m, _) in &self.monitors {
            let mut hit = *m;
            if hit.intersect(r) {
                let area = hit.width() * hit.height();
                if area > best.0 {
                    best = (area, *m);
                }
            }
        }
        best.1
    }

    fn primary_monitor(&self) -> Rect {
        self.monitors.iter().find(|(_, p)| *p).or(self.monitors.first()).map_or(self.frame_rect(), |(r, _)| *r)
    }

    pub fn tool(&self) -> Option<DrawTool> {
        match self.mode {
            Mode::Draw(t) => Some(t),
            Mode::Select => None,
        }
    }

    /// Repaints whatever the live gesture touches before and after `f`, plus anything `f`
    /// committed.
    fn with_live<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = self.editor.live_bounds();
        let r = f(self);
        let after = self.editor.live_bounds();
        let dirty = self.editor.take_dirty();
        self.surface.damage(before);
        self.surface.damage(after);
        self.surface.damage(dirty);
        cx.notify();
        r
    }

    // ---- Session end ----------------------------------------------------------------------

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.surface.release(window);
        let _ = window.drop_image(self.frame_image.clone());
        window.remove_window();
        cx.defer(app::overlay_closed);
    }

    // ---- Tools ----------------------------------------------------------------------------

    fn finish_editing(&mut self, cx: &mut Context<Self>) {
        self.with_live(cx, |this| {
            this.editor.commit_text();
            this.editor.finish_gesture();
        });
    }

    pub(crate) fn set_select(&mut self, cx: &mut Context<Self>) {
        self.exit_ocr(cx);
        self.finish_editing(cx);
        self.mode = Mode::Select;
        self.panel.close();
        self.flyout = None;
        self.cursor = None;
        log::info!("Tool → Select.");
        cx.notify();
    }

    pub(crate) fn set_draw(&mut self, tool: DrawTool, cx: &mut Context<Self>) {
        self.exit_ocr(cx);
        self.finish_editing(cx);
        // Picking a different tool folds the panel away: its contents belong to the tool that
        // was active when it opened.
        if self.tool() != Some(tool) {
            self.panel.close();
        }
        match tool {
            DrawTool::Square | DrawTool::Circle | DrawTool::Triangle => self.shapes_sub = tool,
            DrawTool::Line | DrawTool::Arrow => self.lines_sub = tool,
            DrawTool::Eraser | DrawTool::AbsoluteEraser | DrawTool::FilterEraser => self.eraser_sub = tool,
            DrawTool::Blur | DrawTool::Pixelize => self.effects_sub = tool,
            _ => {}
        }
        self.mode = Mode::Draw(tool);
        self.panel.load(self.editor.settings(tool));
        log::info!("Draw sub-tool → {tool:?}.");
        cx.notify();
    }

    pub(crate) fn set_shape(&mut self, kind: ShapeKind, cx: &mut Context<Self>) {
        self.shape = kind;
        log::info!("Selection shape → {kind:?}.");
        if kind != ShapeKind::Polygon {
            self.polygon = None;
        }
        // Convert an existing box-shaped selection in place (rectangle ⇄ ellipse ⇄ triangle).
        if !matches!(kind, ShapeKind::Lasso | ShapeKind::Polygon)
            && let Some(s) = self.sel.active.as_mut().filter(|s| !s.is_freeform())
        {
            s.kind = kind;
            s.normalized.clear();
        }
        self.set_select(cx);
    }

    /// Digits 1-0 activate toolbar slots left to right, exactly as clicking the tab would.
    pub(crate) fn activate_slot(&mut self, slot: u8, window: &mut Window, cx: &mut Context<Self>) {
        self.flyout = None;
        match slot {
            1 => self.set_select(cx),
            2 => self.set_draw(DrawTool::Brush, cx),
            3 => self.set_draw(self.shapes_sub, cx),
            4 => self.set_draw(self.lines_sub, cx),
            5 => self.set_draw(self.eraser_sub, cx),
            6 => self.set_draw(self.effects_sub, cx),
            7 => self.run_ocr(cx),
            8 => self.set_draw(DrawTool::Text, cx),
            9 => self.request_recording(None, window, cx),
            10 => self.start_audio_defaults(window, cx),
            _ => {}
        }
    }

    // ---- Mouse -------------------------------------------------------------------------------

    fn on_left_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.to_frame(e.position);
        self.notice = None;
        self.flyout = None;
        self.source_menu = None;
        self.panel.blur_fields();

        if self.ocr.active {
            self.ocr.start_select(p);
            cx.notify();
            return;
        }
        if self.polygon.is_some() {
            self.polygon_click(p, e.click_count, cx);
            return;
        }
        if let Mode::Draw(tool) = self.mode {
            if !self.has_selection() {
                return; // drawing only works inside a selection (SPEC §3)
            }
            let clip = self.clip();
            self.with_live(cx, |this| match tool {
                DrawTool::Brush => this.editor.begin_brush(p, clip),
                DrawTool::Text => this.editor.begin_text(p, clip),
                t if t.is_region() => this.editor.begin_region(t, p, e.modifiers.control, clip),
                t => this.editor.begin_shape(t, p, clip),
            });
            if tool == DrawTool::Text {
                window.focus(&self.focus);
            }
            return;
        }

        // Editing the active selection (handle → resize, edge → move) beats a new one.
        if let Some(b) = self.sel.active.as_ref().filter(|s| s.has_area()).map(|s| s.bounds) {
            if let Some(h) = selection::hit_test_handle(p, b, self.tol(HANDLE_SIZE / 2.0 + 3.0)) {
                self.begin_gesture(p, e.modifiers);
                self.drag = Drag::Resize(h, b);
                cx.notify();
                return;
            }
            // The interior is excluded so a new selection can start inside an existing one.
            if selection::is_on_edge(b, p, self.tol(EDGE_TOLERANCE)) {
                self.begin_gesture(p, e.modifiers);
                self.drag = Drag::Move(b);
                cx.notify();
                return;
            }
        }
        // Grabbing a committed selection's edge makes it the active one.
        if let Some(i) = self.sel.hit_committed(p, self.tol(EDGE_TOLERANCE)) {
            self.sel.activate(i);
            let b = self.sel.active.as_ref().map(|s| s.bounds).unwrap_or_default();
            self.begin_gesture(p, e.modifiers);
            self.drag = Drag::Move(b);
            cx.notify();
            return;
        }

        // Empty space: with Ctrl, keep the current selection as another region.
        self.sel.commit_active(e.modifiers.control);
        if self.shape == ShapeKind::Polygon {
            self.polygon = Some(vec![p]);
            self.polygon_cursor = p;
            cx.notify();
            return;
        }
        self.begin_gesture(p, e.modifiers);
        self.drag = Drag::New;
        self.lasso.clear();
        if self.shape == ShapeKind::Lasso {
            self.lasso.push(p);
        }
        self.sel.active = Some(Selection::new(self.shape, Rect::from_xywh(p.x, p.y, 0.0, 0.0)));
        cx.notify();
    }

    fn begin_gesture(&mut self, p: SkPoint, m: Modifiers) {
        self.drag_start = p;
        self.ratio = RatioLock::start(m.shift);
        self.ctrl_a = CtrlA::None;
    }

    fn on_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let p = self.to_frame(e.position);
        if self.ocr.active {
            if self.ocr.selecting {
                self.ocr.extend_select(p);
                cx.notify();
            }
            return;
        }
        if self.eyedropping || self.eyedropper_armed {
            self.eyedrop = Some((p, self.editor.sample_color(p)));
            cx.notify();
            return;
        }
        if self.polygon.is_some() {
            self.polygon_cursor = p;
            cx.notify();
            return;
        }
        if self.mode != Mode::Select {
            self.cursor = Some(p);
            if self.editor.is_drawing() {
                self.with_live(cx, |this| this.editor.extend(p));
            } else {
                cx.notify();
            }
            return;
        }
        if self.drag == Drag::None {
            self.hover = Some(p);
        }
        let area = self.frame_rect();
        let shift = e.modifiers.shift;
        let current = self.sel.active.as_ref().map(|s| s.bounds).unwrap_or_default();
        let new_bounds = match self.drag {
            Drag::None => {
                cx.notify(); // the hover cursor follows handles and edges
                return;
            }
            Drag::New if self.shape == ShapeKind::Lasso => {
                self.lasso.push(p);
                if self.lasso.len() >= 2 {
                    self.sel.active = Some(Selection::from_points(ShapeKind::Lasso, &self.lasso));
                }
                None
            }
            Drag::New => {
                let ratio = self.ratio.ratio(shift, current);
                Some(selection::clamp_rect(selection::rect_from_anchor(self.drag_start, p, ratio), area))
            }
            Drag::Move(rect) => {
                Some(selection::clamp_move(rect, p.x - self.drag_start.x, p.y - self.drag_start.y, area))
            }
            Drag::Resize(h, rect) => {
                let ratio = if h.is_corner() { self.ratio.ratio(shift, current) } else { None };
                Some(selection::clamp_rect(selection::resize_rect(h, rect, p, ratio), area))
            }
        };
        if let (Some(b), Some(s)) = (new_bounds, self.sel.active.as_mut()) {
            s.bounds = b;
        }
        cx.notify();
    }

    fn on_left_up(&mut self, cx: &mut Context<Self>) {
        if self.ocr.active {
            if self.ocr.selecting {
                self.ocr.selecting = false;
                // Selecting a span drops it straight onto the clipboard.
                self.copy_ocr(cx);
            }
            return;
        }
        if self.editor.is_drawing() {
            self.with_live(cx, |this| this.editor.end_gesture());
            return;
        }
        if self.drag == Drag::None {
            return;
        }
        let was_new = self.drag == Drag::New;
        self.drag = Drag::None;
        // A stray click while starting a new selection clears it.
        let min = 3.0 * self.scale;
        if was_new && !self.sel.active.as_ref().is_some_and(|s| s.bounds.width() >= min && s.bounds.height() >= min) {
            self.sel.active = None;
        }
        cx.notify();
    }

    fn on_right_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let p = self.to_frame(e.position);
        // Right-click closes an in-progress polygon by the shortest line (SPEC §4.2).
        if self.polygon.is_some() {
            self.close_polygon(cx);
            return;
        }
        // Armed from the panel button: this right-click is the pick, and it ends the mode.
        if self.eyedropper_armed {
            self.pick_color(p, cx);
            self.arm_eyedropper(false, cx);
            return;
        }
        // Draw mode: holding the right button picks a colour.
        if self.mode != Mode::Select {
            self.eyedropping = true;
            self.eyedrop = Some((p, self.editor.sample_color(p)));
            cx.notify();
        }
    }

    fn on_right_up(&mut self, e: &MouseUpEvent, cx: &mut Context<Self>) {
        if !self.eyedropping {
            return;
        }
        self.eyedropping = false;
        self.pick_color(self.to_frame(e.position), cx);
        self.eyedrop = None;
        cx.notify();
    }

    fn polygon_click(&mut self, p: SkPoint, clicks: usize, cx: &mut Context<Self>) {
        if clicks >= 2 {
            self.close_polygon(cx);
            return;
        }
        let close = self.tol(POLYGON_CLOSE);
        let closes = self.polygon.as_ref().is_some_and(|pts| pts.len() >= 3 && (p - pts[0]).length() <= close);
        if closes {
            // Clicking back on the first vertex closes the outline.
            self.close_polygon(cx);
            return;
        }
        if let Some(pts) = &mut self.polygon {
            pts.push(p);
        }
        self.polygon_cursor = p;
        cx.notify();
    }

    fn close_polygon(&mut self, cx: &mut Context<Self>) {
        if let Some(pts) = self.polygon.take()
            && pts.len() >= 3
        {
            self.sel.active = Some(Selection::from_points(ShapeKind::Polygon, &pts));
        }
        cx.notify();
    }

    // ---- Keyboard ---------------------------------------------------------------------------

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let ctrl = k.modifiers.control;
        let key = k.key.as_str();

        // A focused text field in the panel owns its keys.
        if let Some(tool) = self.tool()
            && self.panel.on_key(e, &mut self.editor, tool)
        {
            self.after_panel_edit(cx);
            return;
        }

        // Typing: editing keys go to the text; Esc commits and keeps it.
        if self.editor.is_typing() {
            match key {
                "escape" => self.with_live(cx, |this| this.editor.commit_text()),
                "enter" => self.with_live(cx, |this| this.editor.type_newline()),
                "backspace" => self.with_live(cx, |this| this.editor.backspace()),
                _ if !ctrl => {
                    if let Some(ch) = k.key_char.clone().filter(|c| !c.is_empty()) {
                        self.with_live(cx, |this| this.editor.type_str(&ch));
                    }
                }
                _ => {}
            }
            return;
        }

        // OCR mode owns copy, select-all and exit.
        if self.ocr.active {
            match key {
                "c" if ctrl => return self.copy_ocr(cx),
                "a" if ctrl => {
                    self.ocr.select_all();
                    return self.copy_ocr(cx);
                }
                "escape" => return self.exit_ocr(cx),
                _ => {}
            }
        }

        if !ctrl && let Some(slot) = digit_slot(key) {
            return self.activate_slot(slot, window, cx);
        }

        match key {
            "a" if ctrl => self.apply_ctrl_a(cx),
            "c" if ctrl && self.has_selection() => self.export(ExportKind::Copy, window, cx),
            "s" if ctrl && self.has_selection() => self.export(ExportKind::Save, window, cx),
            "z" if ctrl && k.modifiers.shift => self.redo(cx),
            "z" if ctrl => self.undo(cx),
            "y" if ctrl => self.redo(cx),
            "escape" => {
                // Esc backs out one layer at a time.
                if self.source_menu.take().is_some() {
                    cx.notify();
                } else if self.eyedropper_armed {
                    self.arm_eyedropper(false, cx);
                } else if self.panel.is_open() {
                    self.panel.close();
                    cx.notify();
                } else if self.polygon.is_some() {
                    self.polygon = None;
                    cx.notify();
                } else {
                    self.close(window, cx);
                }
            }
            "enter" => {
                if self.polygon.is_some() {
                    self.close_polygon(cx);
                } else if self.sel.active.as_ref().is_some_and(Selection::has_area) {
                    self.export(ExportKind::Copy, window, cx);
                }
            }
            _ => {}
        }
    }

    fn on_modifiers(&mut self, e: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        // Shift toggles the tool settings while drawing (SPEC §6.5), but not as part of a
        // chord such as Ctrl+Shift+Z.
        let pressed = e.modifiers.shift && !self.shift_down;
        self.shift_down = e.modifiers.shift;
        if pressed && !e.modifiers.control && self.mode != Mode::Select && !self.editor.is_typing() {
            self.toggle_panel(cx);
        }
    }

    pub(crate) fn toggle_panel(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Select || self.ocr.active {
            return;
        }
        self.flyout = None;
        if self.panel.is_open() {
            self.panel.close();
        } else {
            self.panel.open();
        }
        cx.notify();
    }

    fn apply_ctrl_a(&mut self, cx: &mut Context<Self>) {
        // Replacing the selection under a live stroke would leave it clipped to a rectangle
        // that no longer exists, so the stroke lands first.
        self.with_live(cx, |this| this.editor.finish_gesture());
        self.ctrl_a = if self.ctrl_a == CtrlA::Primary { CtrlA::All } else { CtrlA::Primary };
        let bounds = if self.ctrl_a == CtrlA::All { self.frame_rect() } else { self.primary_monitor() };
        self.sel = SelectionSet { active: Some(Selection::new(ShapeKind::Rectangle, bounds)), committed: Vec::new() };
        self.drag = Drag::None;
        log::info!("Ctrl+A → {:?} monitor selection.", self.ctrl_a);
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.with_live(cx, |this| {
            this.editor.undo();
        });
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        self.with_live(cx, |this| {
            this.editor.redo();
        });
    }

    // ---- Colour ------------------------------------------------------------------------------

    fn pick_color(&mut self, p: SkPoint, cx: &mut Context<Self>) {
        let c = self.editor.sample_color(p);
        if let Some(tool) = self.tool() {
            self.panel.set_color(c, &mut self.editor, tool);
        }
        log::info!("Eyedropper: color picked.");
        self.after_panel_edit(cx);
    }

    pub(crate) fn arm_eyedropper(&mut self, armed: bool, cx: &mut Context<Self>) {
        self.eyedropper_armed = armed;
        if !armed {
            self.eyedrop = None;
        }
        log::info!("Eyedropper: {}.", if armed { "armed (right-click to pick)" } else { "disarmed" });
        cx.notify();
    }

    /// Applies a panel change to whatever is live: the text being typed follows its tool's
    /// colour, size and family.
    pub(crate) fn after_panel_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(DrawTool::Text) = self.tool() {
            let s = self.editor.settings(DrawTool::Text).clone();
            self.with_live(cx, |this| this.editor.restyle_text(&s));
        }
        cx.notify();
    }

    // ---- Export ------------------------------------------------------------------------------

    fn export_raster(&mut self) -> Option<Raster> {
        self.editor.commit_text();
        let (w, h) = (self.editor.width(), self.editor.height());
        let union = self.sel.pixel_union(w, h)?;
        let clip = self.sel.clip_path();
        let bmp = export::crop_masked(self.editor.base(), self.editor.document(), union, clip.as_ref());
        Some(Raster::from_bitmap(&bmp))
    }

    pub(crate) fn export(&mut self, kind: ExportKind, window: &mut Window, cx: &mut Context<Self>) {
        log::info!("Export requested: {kind:?}; shape={:?}.", self.sel.active.as_ref().map(|s| s.kind));
        self.notice = None;
        let Some(raster) = self.export_raster() else {
            log::warn!("Export: no valid selection to export.");
            return;
        };
        match kind {
            ExportKind::Copy => {
                // Hidden while the clipboard is written so the editor appears to close
                // instantly; the selection comes back with a reason if the copy fails.
                self.exporting = true;
                let hwnd = winutil::hwnd(window).map(|h| h.0 as isize);
                if let Some(raw) = hwnd {
                    winutil::after_update(cx, move || win::show(win::hwnd(raw), false));
                }
                cx.spawn_in(window, async move |this, cx| {
                    let result = cx
                        .background_spawn(async move {
                            let bitmap = raster.to_bitmap();
                            let png = export::encode(&bitmap, ImageFormat::Png, 100)?;
                            let bgra = export::to_unpremul_bgra(&bitmap);
                            Some(reshot_platform::clipboard::set_image(&reshot_platform::clipboard::ClipboardImage {
                                width: bitmap.width() as u32,
                                height: bitmap.height() as u32,
                                bgra: &bgra,
                                png: &png,
                            }))
                        })
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.exporting = false;
                        let holder = match result {
                            Some(Ok(())) => return this.close(window, cx),
                            Some(Err(busy)) => busy.holder,
                            None => None,
                        };
                        this.notice = Some(match holder {
                            Some(h) => {
                                format!("Clipboard is held by {h} — copy failed. Ctrl+S saves to a file instead, Esc cancels.")
                            }
                            None => "Clipboard is busy — copy failed. Ctrl+S saves to a file instead, Esc cancels.".into(),
                        });
                        log::warn!("Export: copy failed; selection remains open.");
                        if let Some(raw) = hwnd {
                            winutil::after_update(cx, move || win::show(win::hwnd(raw), true));
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            ExportKind::Save => {
                let settings = self.settings.clone();
                self.close(window, cx);
                cx.background_spawn(async move {
                    if let Err(e) = crate::export::save(&raster.to_bitmap(), &settings) {
                        log::error!("Export: save failed: {e:#}");
                    }
                })
                .detach();
            }
            ExportKind::SaveAs => {
                let settings = self.settings.clone();
                let parent = winutil::hwnd(window).map(|h| h.0 as isize);
                cx.spawn_in(window, async move |this, cx| {
                    let Some(path) = crate::export::ask_save_path(&settings, parent).await else { return };
                    let quality = settings.format.quality;
                    let saved = cx
                        .background_spawn(async move { crate::export::write(&raster.to_bitmap(), &path, quality) })
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| match saved {
                        Ok(()) => this.close(window, cx),
                        Err(e) => {
                            log::error!("Export: save as failed: {e:#}");
                            this.notice = Some(format!("Save failed: {e}"));
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    // ---- Recording -----------------------------------------------------------------------------

    pub(crate) fn request_recording(
        &mut self,
        sources: Option<reshot_recording::AudioSources>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (w, h) = (self.editor.width(), self.editor.height());
        let Some(union) = self.sel.pixel_union(w, h).filter(|u| u.width() >= 2 && u.height() >= 2) else {
            return;
        };
        let mask = self.sel.recording_mask(union);
        let rect = PxRect {
            left: self.origin.0 + union.left,
            top: self.origin.1 + union.top,
            right: self.origin.0 + union.right,
            bottom: self.origin.1 + union.bottom,
        };
        // The recording stream must never see the overlay: excluded from capture and hidden
        // before the recorder opens its stream.
        if let Some(h) = winutil::hwnd(window) {
            win::exclude_from_capture(h, true);
            let raw = h.0 as isize;
            winutil::after_update(cx, move || win::show(win::hwnd(raw), false));
        }
        self.close(window, cx);
        cx.defer(move |cx| crate::recording::start_video_with(cx, rect, mask, sources));
    }

    pub(crate) fn start_audio_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Left click records everything; narrowing it down is what the right-click picker is for.
        let sources = reshot_recording::AudioSources {
            mic: true,
            mic_device: Some(self.settings.audio.mic_device.clone()),
            system_full: true,
            include_pids: Vec::new(),
        };
        self.start_audio(sources, window, cx);
    }

    pub(crate) fn start_audio(
        &mut self,
        sources: reshot_recording::AudioSources,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
        cx.defer(move |cx| crate::recording::start_audio(cx, sources));
    }

    // ---- OCR ------------------------------------------------------------------------------------

    pub(crate) fn run_ocr(&mut self, cx: &mut Context<Self>) {
        if self.ocr.busy {
            return;
        }
        if !reshot_ocr::is_available() {
            self.ocr.hint = Some("No OCR language pack installed (Settings ▸ Language ▸ add an OCR feature).".into());
            cx.notify();
            return;
        }
        let (w, h) = (self.editor.width(), self.editor.height());
        let Some(region) = self.sel.pixel_union(w, h) else {
            self.ocr.hint = Some("Select an area first, then press the OCR tool.".into());
            cx.notify();
            return;
        };
        self.finish_editing(cx);
        self.panel.close();
        self.ocr.begin();
        let lang = self.ocr.lang;
        let crop = crop_bgra(self.editor.base(), region);
        cx.spawn(async move |this, cx| {
            let lines = cx
                .background_spawn(async move {
                    let (w, h) = (region.width() as u32, region.height() as u32);
                    reshot_ocr::recognize(&crop, w, h, w as usize * 4, lang)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.ocr.finish(lines, region);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn toggle_ocr_language(&mut self, cx: &mut Context<Self>) {
        self.ocr.cycle_language();
        if self.ocr.active {
            self.run_ocr(cx);
        } else {
            self.ocr.hint = Some(format!("OCR mode: {}", self.ocr.mode_label()));
            cx.notify();
        }
    }

    fn copy_ocr(&mut self, cx: &mut Context<Self>) {
        let Some((text, scope)) = self.ocr.text_to_copy() else { return };
        match reshot_platform::clipboard::set_text(&text) {
            Ok(()) => self.ocr.hint = Some(format!("Copied {scope}, {} chars.", text.chars().count())),
            Err(_) => log::error!("OCR copy failed."),
        }
        cx.notify();
    }

    fn exit_ocr(&mut self, cx: &mut Context<Self>) {
        if self.ocr.active || self.ocr.hint.is_some() {
            self.ocr.reset();
            cx.notify();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Copy,
    Save,
    SaveAs,
}

fn digit_slot(key: &str) -> Option<u8> {
    match key {
        "0" => Some(10),
        d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => Some(d.as_bytes()[0] - b'0'),
        _ => None,
    }
}

/// Tight BGRA copy of `r` from the base frame.
fn crop_bgra(base: &reshot_core::skia::Bitmap, r: IRect) -> Vec<u8> {
    let rb = base.row_bytes();
    let src = reshot_core::document::pixels(base);
    let row = r.width() as usize * 4;
    let mut out = Vec::with_capacity(row * r.height() as usize);
    for y in r.top as usize..r.bottom as usize {
        let s = y * rb + r.left as usize * 4;
        out.extend_from_slice(&src[s..s + row]);
    }
    out
}

impl Focusable for Overlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
