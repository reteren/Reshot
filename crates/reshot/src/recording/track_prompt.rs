//! Modeless VGUI panel for keeping either, both, or neither recorded audio track.

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowHandle,
    div, px,
};
use reshot_platform::win::{self, PxRect};

use crate::{theme, winutil};

const WIDTH: f32 = 300.0;
/// Fits the content with room to spare: padding 32, title 24, question 37, three rows of the
/// inherited 16 px text at gpui's 1.618 line height (26 each), separator 18 and the Save row
/// 36 come to about 226. 202 cut the Save button off.
const HEIGHT: f32 = 240.0;

#[derive(Clone, Copy)]
pub(super) struct Decision {
    pub keep_system: bool,
    pub keep_mic: bool,
    pub never_ask: bool,
}

pub(super) struct TrackPrompt {
    id: u64,
    focus: FocusHandle,
    has_system: bool,
    has_mic: bool,
    keep_system: bool,
    keep_mic: bool,
    never_ask: bool,
    done: bool,
}

impl TrackPrompt {
    pub(super) fn open(
        id: u64,
        has_system: bool,
        has_mic: bool,
        system_default: bool,
        mic_default: bool,
        cx: &mut App,
    ) -> Option<WindowHandle<Self>> {
        let primary = win::primary_monitor_bounds();
        let work = win::working_area(primary.left + 1, primary.top + 1);
        let scale = winutil::primary_scale(cx);
        let width = (WIDTH * scale).round() as i32;
        let height = (HEIGHT * scale).round() as i32;
        let margin = (16.0 * scale).round() as i32;
        let right = work.right - margin;
        let bottom = work.bottom - margin;
        let rect = PxRect {
            left: right - width,
            top: bottom - height,
            right,
            bottom,
        };
        let handle = cx
            .open_window(
                winutil::popup_options(winutil::logical(rect, scale), true),
                |window, cx| {
                    cx.new(|cx| {
                        let focus = cx.focus_handle();
                        window.focus(&focus);
                        cx.on_release(|this: &mut TrackPrompt, cx| {
                            if !this.done {
                                super::decide(this.id, Decision {
                                    keep_system: this.keep_system,
                                    keep_mic: this.keep_mic,
                                    never_ask: this.never_ask,
                                }, cx);
                            }
                        }).detach();
                        TrackPrompt {
                            id,
                            focus,
                            has_system,
                            has_mic,
                            keep_system: has_system && system_default,
                            keep_mic: has_mic && mic_default,
                            never_ask: false,
                            done: false,
                        }
                    })
                },
            )
            .map_err(|e| log::error!("Audio track prompt: {e}"))
            .ok()?;
        if let Some(hwnd) = handle
            .update(cx, |_, window, _| winutil::hwnd(window))
            .ok()
            .flatten()
        {
            let raw = hwnd.0 as isize;
            winutil::after_update(cx, move || {
                let hwnd = win::hwnd(raw);
                win::place_topmost(hwnd, rect);
                win::force_foreground(hwnd, false);
            });
        }
        Some(handle)
    }

    fn decide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        self.done = true;
        let id = self.id;
        let choice = Decision {
            keep_system: self.keep_system,
            keep_mic: self.keep_mic,
            never_ask: self.never_ask,
        };
        window.remove_window();
        cx.defer(move |cx| super::decide(id, choice, cx));
    }
}

impl Focusable for TrackPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TrackPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let system = theme::check_row("keep-system", self.keep_system, "System audio")
            .opacity(if self.has_system { 1.0 } else { 0.45 })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.has_system {
                    this.keep_system = !this.keep_system;
                    cx.notify();
                }
            }));
        let mic = theme::check_row("keep-mic", self.keep_mic, "Microphone")
            .opacity(if self.has_mic { 1.0 } else { 0.45 })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.has_mic {
                    this.keep_mic = !this.keep_mic;
                    cx.notify();
                }
            }));
        let never = theme::check_row("never-ask", self.never_ask, "Never show this again")
            .on_click(cx.listener(|this, _, _, cx| {
                this.never_ask = !this.never_ask;
                cx.notify();
            }));
        theme::panel()
            .size_full()
            .p(px(16.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.decide(window, cx);
                }
            }))
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(theme::title("AUDIO TRACKS").text_size(px(12.)))
                    .child(
                        theme::button("close-tracks", "✕")
                            .w(px(24.))
                            .on_click(cx.listener(|this, _, window, cx| this.decide(window, cx))),
                    ),
            )
            .child(
                div()
                    .mt(px(12.))
                    .mb(px(6.))
                    .font_family(theme::FONT)
                    .text_size(px(12.))
                    .font_weight(FontWeight::LIGHT)
                    .child("Which audio do you want to keep?"),
            )
            .child(system)
            .child(mic)
            .child(theme::separator().mt(px(10.)).mb(px(6.)))
            .child(never)
            .child(
                // Pinned to the bottom: any spare height goes above it, not below.
                div().w_full().flex().flex_1().items_end().justify_end().mt(px(12.)).child(
                    theme::button("save-tracks", "Save")
                        .min_w(px(68.))
                        .on_click(cx.listener(|this, _, window, cx| this.decide(window, cx))),
                ),
            )
    }
}
