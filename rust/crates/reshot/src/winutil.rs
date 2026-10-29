//! Glue between gpui windows and the Win32 helpers in `reshot-platform`.

use gpui::{App, Bounds, Pixels, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use reshot_platform::win::{self, PxRect};
use windows::Win32::Foundation::HWND;

pub fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(win::hwnd(h.hwnd.get())),
        _ => None,
    }
}

/// A borderless, transparent, topmost-capable popup. gpui places it on a display; callers
/// then pin the exact physical bounds with [`place`].
pub fn popup_options(logical: Bounds<Pixels>, focus: bool) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(logical)),
        titlebar: None,
        focus,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// Logical bounds for a physical rect, given the scale of the monitor it is on.
pub fn logical(r: PxRect, scale: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(px(r.left as f32 / scale), px(r.top as f32 / scale)),
        size(px(r.width() as f32 / scale), px(r.height() as f32 / scale)),
    )
}

/// The DPI scale of the primary display, which is what logical sizes are laid out in.
pub fn primary_scale(cx: &App) -> f32 {
    let _ = cx;
    // SAFETY: plain query.
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() };
    dpi as f32 / 96.0
}

/// Runs Win32 calls that re-enter gpui's window procedure synchronously (SetWindowPos,
/// SetForegroundWindow, ...) on the next turn of the main loop. A foreground task does not hold
/// the `App` borrow, whereas anything inside `update` or `defer` does, and the re-entrant
/// window procedure would then find it already borrowed.
pub fn after_update(cx: &App, f: impl FnOnce() + 'static) {
    cx.spawn(async move |_| f()).detach();
}
