//! The capture session window: one borderless topmost window over the whole virtual desktop,
//! showing the frozen frame dimmed, with the selection cut out at full brightness.
//!
//! Coordinates: the document and selections live in physical frame pixels, (0,0) being the
//! virtual desktop's top-left. gpui lays out in logical pixels; `scale` converts.

mod frame_image;

use std::sync::Arc;

use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, RenderImage,
    Styled, Window, WindowHandle, div, img, point, px,
};
use reshot_capture::CapturedFrame;
use reshot_core::document::CaptureDocument;
use reshot_core::export::{self, ImageFormat};
use reshot_core::settings::AppSettings;
use reshot_core::skia::{Bitmap, IRect, Rect};
use reshot_platform::win::{self, PxRect};

use crate::{app, theme, winutil};

pub struct Overlay {
    focus: FocusHandle,
    frame_image: Arc<RenderImage>,
    base: Bitmap,
    doc: CaptureDocument,
    settings: AppSettings,
    /// Virtual-desktop origin of the frame, for converting cursor positions.
    origin: (i32, i32),
    size: (i32, i32),
    scale: f32,
    /// The selection in frame pixels.
    selection: Option<Rect>,
    drag_anchor: Option<Point<f32>>,
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
        let size = (frame.width as i32, frame.height as i32);
        let origin = (frame.virtual_left, frame.virtual_top);
        drop(frame);

        let handle = cx
            .open_window(winutil::popup_options(winutil::logical(r, scale), true), move |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus);
                    Overlay {
                        focus,
                        frame_image,
                        base,
                        doc: CaptureDocument::new(size.0, size.1),
                        settings,
                        origin,
                        size,
                        scale,
                        selection: None,
                        drag_anchor: None,
                        closing: false,
                    }
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
                let game = win::foreground_is_fullscreen_foreign_window();
                if game {
                    win::unlock_input_from_game();
                }
                win::force_foreground(h, game);
            });
        }
        Some(handle)
    }

    fn to_frame(&self, p: Point<Pixels>) -> Point<f32> {
        point(f32::from(p.x) * self.scale, f32::from(p.y) * self.scale)
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        window.remove_window();
        cx.defer(app::overlay_closed);
    }

    fn selection_irect(&self) -> Option<IRect> {
        let s = self.selection?;
        let r = IRect::from_ltrb(s.left.floor() as i32, s.top.floor() as i32, s.right.ceil() as i32, s.bottom.ceil() as i32);
        (r.width() > 0 && r.height() > 0).then_some(r)
    }

    fn render_selection(&self) -> Option<export::Raster> {
        let r = self.selection_irect()?;
        Some(export::Raster::from_bitmap(&export::crop_masked(&self.base, &self.doc, r, None)))
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(raster) = self.render_selection() else { return };
        self.close(window, cx);
        cx.background_spawn(async move {
            let bitmap = raster.to_bitmap();
            let Some(png) = export::encode(&bitmap, ImageFormat::Png, 100) else { return };
            let bgra = export::to_unpremul_bgra(&bitmap);
            let img = reshot_platform::clipboard::ClipboardImage {
                width: bitmap.width() as u32,
                height: bitmap.height() as u32,
                bgra: &bgra,
                png: &png,
            };
            let _ = reshot_platform::clipboard::set_image(&img);
        })
        .detach();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(raster) = self.render_selection() else {
            log::info!("Overlay: save with no selection");
            return;
        };
        let settings = self.settings.clone();
        self.close(window, cx);
        cx.background_spawn(async move {
            if let Err(e) = crate::export::save(&raster.to_bitmap(), &settings) {
                log::error!("Export: save failed: {e}");
            }
        })
        .detach();
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        log::debug!("Overlay key: {k:?}");
        match k.key.as_str() {
            "escape" => {
                if self.selection.take().is_some() {
                    cx.notify();
                } else {
                    self.close(window, cx);
                }
            }
            "enter" => self.copy(window, cx),
            "c" if k.modifiers.control => self.copy(window, cx),
            "s" if k.modifiers.control => self.save(window, cx),
            "a" if k.modifiers.control => {
                let p = win::primary_monitor_bounds();
                self.selection = Some(Rect::from_ltrb(
                    (p.left - self.origin.0) as f32,
                    (p.top - self.origin.1) as f32,
                    (p.right - self.origin.0) as f32,
                    (p.bottom - self.origin.1) as f32,
                ));
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Focusable for Overlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Overlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.scale;
        let lw = px(self.size.0 as f32 / s);
        let lh = px(self.size.1 as f32 / s);
        let dim = dim_color(&self.settings);

        let mut root = div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let p = this.to_frame(e.position);
                    log::debug!("Overlay mouse down at {p:?}");
                    this.drag_anchor = Some(p);
                    this.selection = None;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let (Some(a), true) = (this.drag_anchor, e.pressed_button == Some(MouseButton::Left)) {
                    let p = this.to_frame(e.position);
                    this.selection = Some(Rect::from_ltrb(a.x.min(p.x), a.y.min(p.y), a.x.max(p.x), a.y.max(p.y)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.drag_anchor = None;
                    cx.notify();
                }),
            )
            .child(img(self.frame_image.clone()).absolute().left(px(0.)).top(px(0.)).w(lw).h(lh));

        let logical = |r: Rect| {
            Bounds::new(point(px(r.left / s), px(r.top / s)), gpui::size(px(r.width() / s), px(r.height() / s)))
        };
        match self.selection.filter(|r| r.width() >= 1.0 && r.height() >= 1.0) {
            None => root = root.child(div().absolute().size_full().bg(dim)),
            Some(sel) => {
                let b = logical(sel);
                let (right, bottom) = (b.origin.x + b.size.width, b.origin.y + b.size.height);
                for (x, y, w, h) in [
                    (px(0.), px(0.), lw, b.origin.y),
                    (px(0.), bottom, lw, lh - bottom),
                    (px(0.), b.origin.y, b.origin.x, b.size.height),
                    (right, b.origin.y, lw - right, b.size.height),
                ] {
                    root = root.child(div().absolute().left(x).top(y).w(w).h(h).bg(dim));
                }
                root = root.child(
                    div()
                        .absolute()
                        .left(b.origin.x)
                        .top(b.origin.y)
                        .w(b.size.width)
                        .h(b.size.height)
                        .border_1()
                        .border_color(theme::accent()),
                );
            }
        }
        root
    }
}

fn dim_color(s: &AppSettings) -> gpui::Rgba {
    let hex = s.dim.color.trim_start_matches('#');
    let rgb = u32::from_str_radix(hex, 16).unwrap_or(0) & 0xFF_FFFF;
    let a = (s.dim.opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    gpui::rgba((rgb << 8) | a)
}
