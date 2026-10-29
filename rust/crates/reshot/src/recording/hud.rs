//! The always-on-top REC indicator and optional selection brackets.

use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, AsyncApp, Context, FontWeight, IntoElement, ParentElement, Render, Styled,
    Window, WindowHandle, div, px, rgb, rgba,
};
use reshot_platform::win::{self, PxRect};

use super::corners::{CornerWindow, prepare_visual_window};
use crate::{theme, winutil};

const WIDTH: f32 = 236.0;
const HEIGHT: f32 = 38.0;

pub(super) struct HudWindows {
    indicator: WindowHandle<RecordingHud>,
    corners: Option<WindowHandle<CornerWindow>>,
}

impl HudWindows {
    pub(super) fn open(
        resolution: String,
        region: Option<PxRect>,
        corner_color: &str,
        corner_opacity: f64,
        cx: &mut App,
    ) -> Option<Self> {
        let primary = win::primary_monitor_bounds();
        let scale = winutil::primary_scale(cx);
        let w = (WIDTH * scale).round() as i32;
        let h = (HEIGHT * scale).round() as i32;
        let margin = (18.0 * scale).round() as i32;
        let right = primary.right - margin;
        let top = primary.top + margin;
        let rect = PxRect {
            left: right - w,
            top,
            right,
            bottom: top + h,
        };
        let handle = cx
            .open_window(
                winutil::popup_options(winutil::logical(rect, scale), false),
                |_, cx| {
                    cx.new(|_| RecordingHud {
                        resolution,
                        started: Instant::now(),
                    })
                },
            )
            .map_err(|e| log::error!("Recording HUD: {e}"))
            .ok()?;
        if let Some(hwnd) = handle
            .update(cx, |_, window, _| winutil::hwnd(window))
            .ok()
            .flatten()
        {
            prepare_visual_window(cx, hwnd, rect);
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if cx
                    .update(|cx| handle.update(cx, |_, _, cx| cx.notify()))
                    .ok()
                    .and_then(Result::ok)
                    .is_none()
                {
                    break;
                }
            }
        })
        .detach();
        let corners = region.and_then(|r| CornerWindow::open(r, corner_color, corner_opacity, cx));
        Some(Self {
            indicator: handle,
            corners,
        })
    }

    pub(super) fn close(self, cx: &mut App) {
        let _ = self
            .indicator
            .update(cx, |_, window, _| window.remove_window());
        if let Some(corners) = self.corners {
            let _ = corners.update(cx, |_, window, _| window.remove_window());
        }
    }
}

struct RecordingHud {
    resolution: String,
    started: Instant,
}

impl Render for RecordingHud {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let elapsed = self.started.elapsed();
        let seconds = elapsed.as_secs();
        let blink = elapsed.as_millis() / 500 % 2 == 1;
        let label = format!(
            "REC  {:02}:{:02}   ·   {}",
            seconds / 60,
            seconds % 60,
            self.resolution
        );
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x18181BCC))
            .border_1()
            .border_color(rgba(0xFFFFFF66))
            .rounded(px(6.))
            .px(px(10.))
            .gap(px(8.))
            .child(
                div()
                    .size(px(11.))
                    .rounded_full()
                    .bg(rgb(0xFF0000))
                    .opacity(if blink { 0.25 } else { 1.0 }),
            )
            .child(
                div()
                    .font_family(theme::FONT)
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(label),
            )
    }
}
