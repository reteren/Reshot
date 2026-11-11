//! Quick adjustments: hold S, F or R and turn the wheel to change the current tool's size,
//! opacity (an effect's strength) or hardness without opening the settings panel. While the
//! wheel moves, a small readout under the pointer shows the value, the way a painting program
//! does.

use gpui::{AnyElement, Context, IntoElement, ParentElement, ScrollWheelEvent, Styled, Window, div, px};
use reshot_core::i18n::tr;

use super::Overlay;
use super::panel::{Slider, set_slider};
use crate::theme;

/// The key being held and whether the wheel has moved since, which is when the readout shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adjust {
    slider: Slider,
    shown: bool,
}

const READOUT_W: f32 = 150.0;
const READOUT_H: f32 = 40.0;

impl Overlay {
    /// S, F or R pressed while a drawing tool is active. Returns whether the key was taken.
    pub(crate) fn adjust_key_down(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(tool) = self.tool() else { return false };
        let slider = match key {
            "s" => Slider::Size,
            "f" => Slider::Opacity,
            "r" if tool.has_hardness() => Slider::Hardness,
            _ => return false,
        };
        // Key repeat while held keeps the readout as it is.
        if self.adjust.is_none_or(|a| a.slider != slider) {
            self.adjust = Some(Adjust { slider, shown: false });
            cx.notify();
        }
        true
    }

    pub(crate) fn adjust_key_up(&mut self, key: &str, cx: &mut Context<Self>) {
        let released = match key {
            "s" => Slider::Size,
            "f" => Slider::Opacity,
            "r" => Slider::Hardness,
            _ => return,
        };
        if self.adjust.is_some_and(|a| a.slider == released) {
            self.adjust = None;
            cx.notify();
        }
    }

    /// The wheel while S, F or R is held: one notch moves size by about a tenth (at least
    /// 1 px), opacity and hardness by 5 %.
    pub(crate) fn adjust_wheel(&mut self, e: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(adjust), Some(tool)) = (self.adjust, self.tool()) else { return };
        let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
        if dy == 0.0 {
            return;
        }
        let (value, _, _) = self.panel.slider_value(adjust.slider, self.editor.settings(tool));
        let step = match adjust.slider {
            Slider::Size => (value * 0.1).round().max(1.0),
            Slider::Opacity | Slider::Hardness => 5.0,
        };
        set_slider(&mut self.editor, tool, adjust.slider, value + step * dy.signum());
        self.adjust = Some(Adjust { shown: true, ..adjust });
        cx.stop_propagation();
        self.after_panel_edit(cx);
    }

    /// The value readout under the pointer while a wheel adjustment is under way.
    pub(crate) fn adjust_readout(&self, screen_w: f32, screen_h: f32) -> Option<AnyElement> {
        let adjust = self.adjust.filter(|a| a.shown)?;
        let tool = self.tool()?;
        let c = self.cursor.or(self.hover)?;
        let (value, min, max) = self.panel.slider_value(adjust.slider, self.editor.settings(tool));
        let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let (name, shown) = match adjust.slider {
            Slider::Size => (tr("Size"), format!("{} px", value.round() as i32)),
            Slider::Opacity if tool.is_effect() => (tr("Strength"), format!("{}%", value.round() as i32)),
            Slider::Opacity => (tr("Opacity"), format!("{}%", value.round() as i32)),
            Slider::Hardness => (tr("Hardness"), format!("{}%", value.round() as i32)),
        };
        let (x, y) = (c.x / self.scale, c.y / self.scale);
        // Below and right of the pointer, kept clear of the brush ring; flipped near the edges.
        let ring = (self.editor.settings(tool).thickness / 2.0) / self.scale;
        let offset = ring.min(120.0) + 14.0;
        let left = if x + offset + READOUT_W > screen_w { x - offset - READOUT_W } else { x + offset };
        let top = if y + offset + READOUT_H > screen_h { y - offset - READOUT_H } else { y + offset };
        let (well, inner) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        Some(
            div()
                .absolute()
                .left(px(left.max(0.0)))
                .top(px(top.max(0.0)))
                .w(px(READOUT_W))
                .bg(theme::panel_bg())
                .rounded(px(6.))
                .px(px(8.))
                .py(px(6.))
                .font_family(theme::FONT)
                .text_size(px(12.))
                .text_color(theme::text())
                .child(div().flex().justify_between().child(name).child(shown))
                .child(
                    well.mt(px(4.)).h(px(6.)).relative().child(inner).child(
                        div()
                            .absolute()
                            .left(px(1.))
                            .top(px(1.))
                            .bottom(px(1.))
                            .w(gpui::relative(fraction * 0.98))
                            .bg(theme::accent()),
                    ),
                )
                .into_any_element(),
        )
    }
}
