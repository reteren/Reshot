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

/// Holds a window in front of a fullscreen game for as long as it is open. Call it only when a
/// foreign fullscreen window was in front when the window opened: against the desktop there is
/// nothing to hold off and the loop is not free.
///
/// Two phases, because they cost very different things. Stealing the foreground attaches our
/// input queue to the game's, which the user sees as a blinking cursor, so it is tried a
/// bounded number of times. Re-asserting topmost touches no input state, so it keeps going at
/// a slow tick: a game that re-asserts its own topmost every frame would otherwise bury the
/// window a second after it appeared.
pub fn keep_over_game<V: 'static>(cx: &mut App, handle: gpui::WindowHandle<V>, raw: isize) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    const MAX_STEAL_ATTEMPTS: usize = 8; // about 0.6 s
    let won = Arc::new(AtomicBool::new(false));
    cx.spawn(async move |cx: &mut gpui::AsyncApp| {
        let mut attempts = 0;
        loop {
            let stealing = attempts < MAX_STEAL_ATTEMPTS && !won.load(Ordering::Relaxed);
            let tick = if stealing { 80 } else { 250 };
            cx.background_executor().timer(Duration::from_millis(tick)).await;
            if cx.update(|cx| handle.update(cx, |_, _, _| ())).ok().and_then(Result::ok).is_none() {
                break; // the window is gone
            }
            let flag = Arc::clone(&won);
            let _ = cx.update(|cx| {
                after_update(cx, move || {
                    let hwnd = win::hwnd(raw);
                    if stealing {
                        // Side-effect free, unlike the rest of the unlock: a game re-confines
                        // the pointer every frame until it accepts it lost the foreground.
                        win::release_cursor_clip();
                        if win::force_foreground(hwnd, false) {
                            flag.store(true, Ordering::Relaxed);
                        }
                    } else {
                        win::assert_topmost(hwnd);
                    }
                })
            });
            if stealing {
                attempts += 1;
                if attempts == MAX_STEAL_ATTEMPTS {
                    log::warn!("Foreground: the game kept it after {MAX_STEAL_ATTEMPTS} attempts; holding topmost only.");
                }
            }
        }
    })
    .detach();
}
