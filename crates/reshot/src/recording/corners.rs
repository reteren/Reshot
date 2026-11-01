//! Click-through bracket window, in physical virtual-desktop coordinates.

use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, WindowHandle,
    div, px, rgb,
};
use reshot_platform::win::{self, PxRect};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongW, SetWindowLongW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};

use crate::winutil;

const LEG: f32 = 28.0;
const THICK: f32 = 4.0;

pub(super) struct CornerWindow {
    width: f32,
    height: f32,
    color: gpui::Rgba,
}

impl CornerWindow {
    pub(super) fn open(
        rect: PxRect,
        color: &str,
        opacity: f64,
        cx: &mut App,
    ) -> Option<WindowHandle<Self>> {
        if rect.width() <= 0 || rect.height() <= 0 {
            return None;
        }
        let scale = winutil::primary_scale(cx);
        let mut color = parse_color(color);
        color.a *= opacity.clamp(0.0, 1.0) as f32;
        let handle = cx
            .open_window(
                winutil::popup_options(winutil::logical(rect, scale), false),
                |_, cx| {
                    cx.new(|_| Self {
                        width: rect.width() as f32 / scale,
                        height: rect.height() as f32 / scale,
                        color,
                    })
                },
            )
            .map_err(|e| log::error!("Recording corners: {e}"))
            .ok()?;
        if let Some(hwnd) = handle
            .update(cx, |_, window, _| winutil::hwnd(window))
            .ok()
            .flatten()
        {
            prepare_visual_window(cx, hwnd, rect);
        }
        Some(handle)
    }
}

/// Call after gpui has released its App borrow; SetWindowPos synchronously re-enters gpui.
pub(super) fn prepare_visual_window(cx: &App, hwnd: HWND, rect: PxRect) {
    let raw = hwnd.0 as isize;
    winutil::after_update(cx, move || {
        let hwnd = win::hwnd(raw);
        // The bracket and HUD windows must never consume a game click or become foreground.
        unsafe {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ex | WS_EX_TRANSPARENT.0 as i32
                    | WS_EX_LAYERED.0 as i32
                    | WS_EX_TOOLWINDOW.0 as i32
                    | WS_EX_NOACTIVATE.0 as i32,
            );
        }
        // No `win::no_frame` here: the system frame's thin outline and shadow around the
        // recorded area are wanted.
        win::place_topmost(hwnd, rect);
        win::exclude_from_capture(hwnd, true);
    });
}

impl Render for CornerWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = (self.width, self.height);
        let leg_x = LEG.min(w);
        let leg_y = LEG.min(h);
        let thick_x = THICK.min(w);
        let thick_y = THICK.min(h);
        let mut root = div().size_full().relative();
        let bars = [
            (0.0, 0.0, leg_x, thick_y),
            (0.0, 0.0, thick_x, leg_y),
            (w - leg_x, 0.0, leg_x, thick_y),
            (w - thick_x, 0.0, thick_x, leg_y),
            (0.0, h - thick_y, leg_x, thick_y),
            (0.0, h - leg_y, thick_x, leg_y),
            (w - leg_x, h - thick_y, leg_x, thick_y),
            (w - thick_x, h - leg_y, thick_x, leg_y),
        ];
        for (x, y, width, height) in bars {
            root = root.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(width))
                    .h(px(height))
                    .bg(self.color),
            );
        }
        root
    }
}

fn parse_color(hex: &str) -> gpui::Rgba {
    let hex = hex.trim().trim_start_matches('#');
    match hex.len() {
        6 => u32::from_str_radix(hex, 16)
            .map(rgb)
            .unwrap_or_else(|_| rgb(0xFF0000)),
        8 => {
            let value = u32::from_str_radix(hex, 16).unwrap_or(0xFFFF0000);
            let mut color = rgb(value & 0x00FF_FFFF);
            color.a = (value >> 24) as f32 / 255.0;
            color
        }
        _ => rgb(0xFF0000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corner_color_accepts_wpf_alpha_and_falls_back_to_red() {
        assert!((parse_color("#803C9898").a - 128.0 / 255.0).abs() < 0.01);
        assert_eq!(parse_color("bad").r, rgb(0xFF0000).r);
    }
}
