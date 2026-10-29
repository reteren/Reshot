//! R0 spike: can gpui give us a transparent, topmost, borderless window over the whole
//! virtual desktop, with a dim fill and an undimmed hole, closed by Esc?

use gpui::{
    App, Application, Bounds, Context, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    div, point, prelude::*, px, rgba, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HWND_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_SHOWWINDOW, SetWindowPos,
};

struct Spike {
    anchor: Option<Point<Pixels>>,
    cursor: Point<Pixels>,
    placed: bool,
}

impl Render for Spike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.placed {
            self.placed = true;
            if let Ok(handle) = HasWindowHandle::window_handle(window)
                && let RawWindowHandle::Win32(h) = handle.as_raw()
            {
                let hwnd = HWND(h.hwnd.get() as *mut _);
                unsafe {
                    let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
                    let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
                    let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                    let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW);
                }
            }
        }
        let sel = self.anchor.map(|a| {
            let (x0, x1) = (a.x.min(self.cursor.x), a.x.max(self.cursor.x));
            let (y0, y1) = (a.y.min(self.cursor.y), a.y.max(self.cursor.y));
            Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0))
        });
        let mut root = div()
            .size_full()
            .relative()
            .on_key_down(|e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.quit();
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.anchor = Some(e.position);
                    this.cursor = e.position;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button == Some(MouseButton::Left) {
                    this.cursor = e.position;
                    cx.notify();
                }
            }));
        // Dim = four rects around the hole, which is how the overlay will do it too.
        let dim = rgba(0x0000_0080);
        let vp = window.viewport_size();
        match sel {
            None => root = root.child(div().absolute().size_full().bg(dim)),
            Some(b) => {
                let r = b.origin.x + b.size.width;
                let btm = b.origin.y + b.size.height;
                for (x, y, w, h) in [
                    (px(0.), px(0.), vp.width, b.origin.y),
                    (px(0.), btm, vp.width, vp.height - btm),
                    (px(0.), b.origin.y, b.origin.x, b.size.height),
                    (r, b.origin.y, vp.width - r, b.size.height),
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
                        .border_color(rgba(0x3C98_98ff)),
                );
            }
        }
        root
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(800.), px(600.)),
            ))),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        let w = cx
            .open_window(opts, |_, cx| {
                cx.new(|_| Spike { anchor: None, cursor: point(px(0.), px(0.)), placed: false })
            })
            .unwrap();
        w.update(cx, |_, window, _| window.activate_window()).ok();
        cx.activate(true);
    });
}
