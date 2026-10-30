//! Rendering the overlay and routing the window's mouse events. Layers, bottom to top, as in
//! the C# window: frozen frame, dim, the editing surface (committed drawings stay visible above
//! the dim while re-selecting), selection chrome, then the HUD.

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, DispatchPhase, InteractiveElement, IntoElement, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render,
    Styled, Window, canvas, div, img, point, px, rgb, rgba, size,
};
use reshot_core::editor::DrawTool;
use reshot_core::selection::{self, Handle};
use reshot_core::skia::Rect;

use super::chrome::{self, ChromeScene};
use super::panel::{PANEL_WIDTH, clamp_to};
use super::toolbar::{TOOLBAR_H, TOOLBAR_W};
use super::{Drag, EDGE_TOLERANCE, HANDLE_SIZE, Mode, Overlay};
use crate::theme;

impl Overlay {
    fn logical(&self, r: Rect) -> (f32, f32, f32, f32) {
        let s = self.scale;
        (r.left / s, r.top / s, r.right / s, r.bottom / s)
    }

    fn cursor_style(&self) -> CursorStyle {
        if self.ocr.active || self.mode == Mode::Draw(DrawTool::Text) {
            return CursorStyle::IBeam;
        }
        if self.mode != Mode::Select {
            return CursorStyle::Crosshair;
        }
        let handle_cursor = |h: Handle| match h {
            Handle::TopLeft | Handle::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Handle::TopRight | Handle::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
            Handle::Top | Handle::Bottom => CursorStyle::ResizeUpDown,
            Handle::Left | Handle::Right => CursorStyle::ResizeLeftRight,
        };
        match self.drag {
            Drag::Resize(h, _) => return handle_cursor(h),
            Drag::Move(_) => return CursorStyle::OpenHand,
            _ => {}
        }
        let Some(p) = self.hover else { return CursorStyle::Crosshair };
        if let Some(b) = self.sel.active.as_ref().filter(|s| s.has_area()).map(|s| s.bounds) {
            if let Some(h) = selection::hit_test_handle(p, b, self.tol(HANDLE_SIZE / 2.0 + 3.0)) {
                return handle_cursor(h);
            }
            if selection::is_on_edge(b, p, self.tol(EDGE_TOLERANCE)) {
                return CursorStyle::OpenHand;
            }
        }
        if self.sel.hit_committed(p, self.tol(EDGE_TOLERANCE)).is_some() {
            return CursorStyle::OpenHand;
        }
        CursorStyle::Crosshair
    }

    /// Exported size in frame pixels, just outside the selection's top-left; tucked inside
    /// when there is no room above, as the toolbar does at the bottom.
    fn size_badge(&self, sel: Rect, screen: (f32, f32, f32, f32)) -> AnyElement {
        let (w, h) = (sel.width().round() as i32, sel.height().round() as i32);
        let text = format!("{w} × {h}");
        let (bw, bh) = (text.chars().count() as f32 * 6.6 + 12.0, 20.0);
        let (l, t, _, _) = self.logical(sel);
        let (sl, st, sr, sb) = screen;
        let gap = 6.0;
        let (mut x, mut y) = (l, t - bh - gap);
        if y < st {
            x = l + gap;
            y = t + gap;
        }
        div()
            .absolute()
            .left(px(clamp_to(x, sl, sr - bw)))
            .top(px(clamp_to(y, st, sb - bh)))
            .bg(theme::panel_bg())
            .rounded(px(3.))
            .px(px(6.))
            .py(px(2.))
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::LIGHT)
            .text_color(theme::text())
            .font_family(theme::FONT)
            .child(text)
            .into_any_element()
    }

    fn brush_ring(&self) -> Option<AnyElement> {
        let tool = self.tool()?;
        if !(tool == DrawTool::Brush || tool.is_region()) || self.eyedropping || self.eyedropper_armed {
            return None;
        }
        let c = self.cursor?;
        let r = (self.editor.settings(tool).thickness / 2.0).max(2.0) / self.scale;
        let (x, y) = (c.x / self.scale, c.y / self.scale);
        // Double-stroked ring, for contrast over anything.
        Some(
            div()
                .absolute()
                .left(px(x - r - 1.))
                .top(px(y - r - 1.))
                .size(px(2. * r + 2.))
                .rounded_full()
                .border_2()
                .border_color(rgba(0x000000A0))
                .child(div().size_full().rounded_full().border_1().border_color(rgba(0xFFFFFFE6)))
                .into_any_element(),
        )
    }

    fn eyedropper_preview(&self, screen_w: f32, screen_h: f32) -> Option<AnyElement> {
        let (p, c) = self.eyedrop?;
        let hex = format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
        let (x, y) = (p.x / self.scale, p.y / self.scale);
        let (w, h) = (110.0, 38.0);
        let (sw, si) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        Some(
            div()
                .absolute()
                .left(px((x + 18.0).clamp(0.0, (screen_w - w).max(0.0))))
                .top(px((y - h - 12.0).clamp(0.0, screen_h)))
                .bg(theme::panel_bg())
                .rounded(px(6.))
                .p(px(5.))
                .flex()
                .items_center()
                .child(sw.size(px(28.)).bg(rgb(u32::from(c.r()) << 16 | u32::from(c.g()) << 8 | u32::from(c.b()))).child(si))
                .child(div().ml(px(8.)).mr(px(4.)).text_size(px(12.)).text_color(theme::text()).font_family(theme::FONT).child(hex))
                .into_any_element(),
        )
    }

    fn banner(text: String, bg: gpui::Rgba, top: f32) -> AnyElement {
        div()
            .absolute()
            .top(px(top))
            .left(px(0.))
            .right(px(0.))
            .flex()
            .justify_center()
            .child(
                div()
                    .bg(bg)
                    .rounded(px(6.))
                    .px(px(12.))
                    .py(px(6.))
                    .max_w(px(620.))
                    .text_size(px(13.))
                    .text_color(theme::text())
                    .font_family(theme::FONT)
                    .child(text),
            )
            .into_any_element()
    }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.scale;
        let (fw, fh) = (self.editor.width() as f32, self.editor.height() as f32);
        let (lw, lh) = (fw / s, fh / s);

        self.surface.update(&mut self.editor);
        let surface = self.surface.prepare(s);

        let anchor = self.sel.active.as_ref().filter(|a| a.has_area()).or(self.sel.committed.last()).map(|a| a.bounds);
        let show_handles = self.sel.active.as_ref().is_some_and(|a| a.has_area()) && self.drag != Drag::New;
        let scene = ChromeScene {
            scale: s,
            size: (fw, fh),
            dim: dim_color(&self.settings),
            outlines: self.sel.all().map(chrome::outline).collect(),
            polygon: self.polygon.clone().map(|p| (p, self.polygon_cursor)),
            handles: if show_handles && self.polygon.is_none() { self.sel.active.as_ref().map(|a| a.bounds) } else { None },
            handle_size: HANDLE_SIZE,
            ocr_words: match self.ocr.selected_range() {
                Some((a, b)) => self.ocr.words.iter().enumerate().map(|(i, w)| (w.rect, i >= a && i <= b)).collect(),
                None => self.ocr.words.iter().map(|w| (w.rect, false)).collect(),
            },
        };

        let entity = cx.entity();
        let layers = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                scene.paint(bounds.origin, window);
                surface.paint(bounds.origin, window);
                // Window-level listeners: like a WPF mouse capture, a drag keeps receiving
                // moves and its release even over the toolbar or the panel.
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        e.update(cx, |this, cx| {
                            if !this.panel_drag_move(ev, cx) {
                                this.on_move(ev, cx);
                            }
                        });
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    e.update(cx, |this, cx| {
                        if ev.button == MouseButton::Left {
                            if !this.panel_drag_end() {
                                this.on_left_up(cx);
                            }
                        } else if ev.button == MouseButton::Right {
                            this.on_right_up(ev, cx);
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full();

        let mut root = div()
            .id("overlay")
            .size_full()
            .relative()
            .font_family(theme::FONT)
            .cursor(self.cursor_style())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.on_key(e, window, cx)))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| this.on_modifiers(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| this.on_left_down(e, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| this.on_right_down(e, cx)))
            .child(img(self.frame_image.clone()).absolute().left(px(0.)).top(px(0.)).w(px(lw)).h(px(lh)))
            .child(layers);

        // The selection's chrome: size badge, toolbar, strip, panel, flyout.
        let busy = self.drag != Drag::None || self.polygon.is_some() || self.editor.is_drawing();
        if let Some(sel) = anchor.filter(|_| self.polygon.is_none()) {
            let screen_r = self.anchor_screen(sel);
            let screen = self.logical(screen_r);
            root = root.child(self.size_badge(sel, screen));
            if self.drag == Drag::None {
                let (sl, st, sr, sb) = screen;
                let (l, _, r, b) = self.logical(sel);
                let margin = 8.0;
                let (mut x, mut y) = (r - TOOLBAR_W, b + margin);
                // No room below: sit inside the selection's bottom-right corner, kept within
                // the frame the user is looking at.
                if y + TOOLBAR_H > sb {
                    y = b.min(sb) - TOOLBAR_H - margin;
                    x = clamp_to(x, l.max(sl), r.min(sr) - TOOLBAR_W);
                }
                let x = clamp_to(x, sl, sr - TOOLBAR_W);
                let y = clamp_to(y, st, sb - TOOLBAR_H);
                root = root.child(self.render_toolbar(x, y, cx));
                let bounds = Bounds::new(point(px(sl), px(st)), size(px(sr - sl), px(sb - st)));
                let settings = self.render_settings(x, y, TOOLBAR_H, bounds, window, cx);
                let strip_visible = settings.is_some();
                root = root.children(settings);
                root = root.children(self.render_flyout(x, y, strip_visible, screen, window, cx));
                root = root.children(self.render_source_menu(screen, cx));
                let _ = PANEL_WIDTH;
            }
        }
        if !busy || self.editor.is_drawing() {
            root = root.children(self.brush_ring());
        }
        root = root.children(self.eyedropper_preview(lw, lh));
        if let Some(h) = &self.ocr.hint {
            root = root.child(Self::banner(h.clone(), theme::panel_bg(), 24.0));
        }
        if let Some(n) = &self.notice {
            root = root.child(Self::banner(n.clone(), rgba(0xB4231CE6), 24.0 + 40.0));
        }
        root
    }
}

pub fn dim_color(s: &reshot_core::settings::AppSettings) -> gpui::Rgba {
    let hex = s.dim.color.trim().trim_start_matches('#');
    let rgb = u32::from_str_radix(hex, 16).unwrap_or(0) & 0xFF_FFFF;
    let a = (s.dim.opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    rgba((rgb << 8) | a)
}
