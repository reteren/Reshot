//! Modeless VGUI panel for keeping either, both, or neither recorded audio track.

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowHandle,
    div, px, rgb, rgba, svg,
};
use reshot_platform::win::{self, PxRect};

use crate::settings_window::vgui;
use crate::{theme, winutil};

const WIDTH: f32 = 300.0;
/// Padding 32, caption 16, question 15, the two track rows 38 (15 each, 8 apart), separator
/// 2, "never" 15 and the Save button 24, with 12 between the six blocks: 202. A few spare
/// pixels go above the button.
const HEIGHT: f32 = 208.0;

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
                win::no_frame(hwnd);
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
    /// Built from the settings dialog's controls (`settings_window::vgui`), so the face, the
    /// bold caption, the close glyph, the checkboxes and the button match it exactly.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let system = vgui::checkbox("keep-system", self.keep_system, "System audio", self.has_system).on_click(
            cx.listener(|this, _, _, cx| {
                if this.has_system {
                    this.keep_system = !this.keep_system;
                    cx.notify();
                }
            }),
        );
        let mic = vgui::checkbox("keep-mic", self.keep_mic, "Microphone", self.has_mic).on_click(cx.listener(
            |this, _, _, cx| {
                if this.has_mic {
                    this.keep_mic = !this.keep_mic;
                    cx.notify();
                }
            },
        ));
        let never = vgui::checkbox("never-ask", self.never_ask, "Never show this again", true).on_click(
            cx.listener(|this, _, _, cx| {
                this.never_ask = !this.never_ask;
                cx.notify();
            }),
        );
        div()
            .size_full()
            .rounded(px(8.))
            .bg(theme::dialog_bg())
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .font_family(vgui::FONT)
            .font_weight(FontWeight::THIN)
            .text_size(px(12.))
            .line_height(px(vgui::LINE))
            .text_color(theme::text())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.decide(window, cx);
                }
            }))
            .child(
                // The settings dialog's caption row, glyph for glyph.
                div()
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_grow()
                            .mt(px(2.))
                            .text_size(px(10.))
                            .line_height(px(12.5))
                            .font_weight(FontWeight::BOLD)
                            .child("AUDIO TRACKS"),
                    )
                    .child(
                        div()
                            .id("close-tracks")
                            .size(px(16.))
                            .flex_none()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0x00000026)))
                            .child(svg().path("icons/dialog-close.svg").size(px(16.)).text_color(rgb(0xC1C1C1)))
                            .on_click(cx.listener(|this, _, window, cx| this.decide(window, cx))),
                    ),
            )
            .child(vgui::text(12., vgui::LINE).child("Which audio do you want to keep?"))
            .child(div().flex().flex_col().gap(px(8.)).child(system).child(mic))
            .child(theme::separator())
            .child(never)
            .child(
                // Pinned to the bottom: any spare height goes above it, not below.
                div().flex_1().flex().items_end().justify_end().child(
                    vgui::button("save-tracks", "Save", false, false)
                        .on_click(cx.listener(|this, _, window, cx| this.decide(window, cx))),
                ),
            )
    }
}
