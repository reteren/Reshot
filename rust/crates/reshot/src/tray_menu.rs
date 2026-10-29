//! The tray context menu, drawn as a borderless window instead of a native menu so it carries
//! the same Half-Life 2 styling as the settings dialog. Holds no app logic: every item calls
//! back into [`crate::app`]. Clicking anywhere else dismisses it, like a real context menu.

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowHandle, div, px,
};
use reshot_platform::win::{self, PxRect};

use crate::{app, theme, winutil};

/// Logical size of the menu. gpui has no size-to-content for windows, so the layout below is
/// fixed and this matches it.
const WIDTH: f32 = 186.0;
const HEIGHT: f32 = 178.0;

pub struct TrayMenu {
    paused: bool,
    focus: FocusHandle,
    closing: bool,
}

impl TrayMenu {
    /// Opens the menu near the cursor, inside the work area of the screen it was summoned on.
    /// The tray lives in a corner, so it normally opens up and to the left of the cursor.
    pub fn open(paused: bool, cx: &mut App) -> Option<WindowHandle<TrayMenu>> {
        let (cx_, cy) = win::cursor_pos();
        let work = win::working_area(cx_, cy);
        let scale = winutil::primary_scale(cx);
        let (w, h) = ((WIDTH * scale).round() as i32, (HEIGHT * scale).round() as i32);
        let mut left = cx_ - w;
        if left < work.left {
            left = cx_;
        }
        let mut top = cy - h;
        if top < work.top {
            top = cy;
        }
        let r = PxRect { left: left.min(work.right - w), top: top.min(work.bottom - h), right: 0, bottom: 0 };
        let r = PxRect { right: r.left + w, bottom: r.top + h, ..r };

        let handle = cx
            .open_window(winutil::popup_options(winutil::logical(r, scale), true), |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus);
                    // Losing activation closes the menu.
                    cx.observe_window_activation(window, |this: &mut TrayMenu, window, _| {
                        if !window.is_window_active() && !this.closing {
                            this.closing = true;
                            window.remove_window();
                        }
                    })
                    .detach();
                    TrayMenu { paused, focus, closing: false }
                })
            })
            .map_err(|e| log::error!("Tray menu: failed to open: {e}"))
            .ok()?;
        // Placement runs after the update: SetWindowPos re-enters gpui synchronously.
        if let Some(h) = handle.update(cx, |_, window, _| winutil::hwnd(window)).ok().flatten() {
            let raw = h.0 as isize;
            winutil::after_update(cx, move || {
                let h = win::hwnd(raw);
                win::place_topmost(h, r);
                // Win32 notify-icon rule: assert the foreground on the menu before it is used,
                // so deactivation and focus behave like a native menu.
                win::force_foreground(h, false);
            });
        }
        Some(handle)
    }

    fn close(&mut self, window: &mut Window) {
        if !self.closing {
            self.closing = true;
            window.remove_window();
        }
    }
}

impl Focusable for TrayMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TrayMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, label: &'static str, bold: bool| {
            theme::button(
                id,
                div()
                    .text_size(px(12.))
                    .font_weight(if bold { FontWeight::BOLD } else { FontWeight::LIGHT })
                    .child(label),
            )
        };
        theme::panel()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, _| {
                if e.keystroke.key == "escape" {
                    this.close(window);
                }
            }))
            .flex()
            .flex_col()
            .child(theme::title("RESHOT").ml(px(2.)).mb(px(8.)))
            .child(item("capture", "Capture", true).on_click(cx.listener(|this, _, window, cx| {
                this.close(window);
                cx.defer(|cx| app::capture(cx, "tray menu"));
            })))
            .child(item("settings", "Settings…", false).mt(px(4.)).on_click(cx.listener(|this, _, window, cx| {
                this.close(window);
                cx.defer(app::open_settings);
            })))
            .child(theme::separator().mt(px(8.)))
            .child(
                theme::check_row("pause", self.paused, "Pause hotkey")
                    .mt(px(4.))
                    .text_size(px(12.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let paused = !this.paused;
                        this.close(window);
                        cx.defer(move |cx| app::set_paused(cx, paused));
                    })),
            )
            .child(theme::separator().mt(px(4.)))
            .child(item("quit", "Quit", false).mt(px(8.)).on_click(cx.listener(|_, _, _, cx| {
                cx.defer(app::quit);
            })))
    }
}
