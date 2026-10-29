//! The tool settings panel (SPEC §6.5): pulled out of the dotted strip above the toolbar by a
//! click or Shift. It adapts to the tool: erasers show size and the Photoshop-style opacity
//! (hardness), effects show size and strength, ink tools show size, opacity and the colour
//! picker, and the Text tool adds the font family.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, Div, ElementId, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, ScrollWheelEvent, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, div, linear_color_stop, linear_gradient, px, rgb, rgba, svg, uniform_list,
};
use reshot_core::editor::{DrawTool, Editor};
use reshot_core::fonts;
use reshot_core::skia::Color;
use reshot_core::tools::BrushSettings;

use super::Overlay;
use crate::theme;

pub const PANEL_WIDTH: f32 = 272.0;
pub const STRIP_HEIGHT: f32 = 14.0;
const ANIM: Duration = Duration::from_millis(300);
const SV_W: f32 = 204.0;
const SV_H: f32 = 120.0;
const PALETTE: [u32; 16] = [
    0xFF3B30, 0xFF9500, 0xFFCC00, 0x34C759, 0x00C7BE, 0x30B0C7, 0x007AFF, 0x5856D6, 0xAF52DE, 0xFF2D55, 0xA2845E,
    0x8E8E93, 0xFFFFFF, 0xC7C7CC, 0x48484A, 0x000000,
];
const RESET_COLOR: u32 = 0xFF3B30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slider {
    Size,
    Opacity,
    Hardness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Slider(Slider),
    Sv,
    Hue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Hex,
    FontSearch,
}

pub struct PanelState {
    open: bool,
    /// Start of the running open/close animation and whether it is opening.
    anim: Option<(Instant, bool)>,
    hue: f32,
    sat: f32,
    val: f32,
    hex: String,
    field: Option<Field>,
    font_open: bool,
    font_search: String,
    drag: Option<Drag>,
    bounds: [Rc<Cell<Bounds<Pixels>>>; 5],
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            open: false,
            anim: None,
            hue: 0.0,
            sat: 0.0,
            val: 0.0,
            hex: String::new(),
            field: None,
            font_open: false,
            font_search: String::new(),
            drag: None,
            bounds: Default::default(),
        }
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn hex_of(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

fn parse_hex(s: &str) -> Option<Color> {
    let h = s.trim().trim_start_matches('#');
    let v = match h.len() {
        6 => u32::from_str_radix(h, 16).ok()?,
        3 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            ((r * 17) << 16) | ((g * 17) << 8) | (b * 17)
        }
        _ => return None,
    };
    Some(Color::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn hsv_to_color(h: f32, s: f32, v: f32) -> Color {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round() as u8;
    Color::from_rgb(to(r), to(g), to(b))
}

fn color_to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

impl PanelState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.anim = Some((Instant::now(), true));
        }
    }

    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.anim = Some((Instant::now(), false));
        }
        self.field = None;
        self.font_open = false;
        self.drag = None;
    }

    /// 0 = closed, 1 = fully open, eased; and whether an animation is still running.
    fn progress(&mut self) -> (f32, bool) {
        let target = if self.open { 1.0 } else { 0.0 };
        let Some((start, opening)) = self.anim else { return (target, false) };
        let t = (start.elapsed().as_secs_f32() / ANIM.as_secs_f32()).min(1.0);
        if t >= 1.0 {
            self.anim = None;
            return (target, false);
        }
        let e = ease_out_cubic(t);
        (if opening { e } else { 1.0 - e }, true)
    }

    /// A click on the canvas takes the keys back from the hex field or the font search.
    pub fn blur_fields(&mut self) {
        self.field = None;
        self.font_open = false;
    }

    /// Pulls the tool's colour into the picker.
    pub fn load(&mut self, s: &BrushSettings) {
        (self.hue, self.sat, self.val) = color_to_hsv(s.color);
        self.hex = hex_of(s.color);
    }

    pub fn set_color(&mut self, c: Color, editor: &mut Editor, tool: DrawTool) {
        let c = Color::from_rgb(c.r(), c.g(), c.b());
        editor.settings_mut(tool).color = c;
        (self.hue, self.sat, self.val) = color_to_hsv(c);
        self.hex = hex_of(c);
    }

    fn apply_hsv(&mut self, editor: &mut Editor, tool: DrawTool) {
        let c = hsv_to_color(self.hue, self.sat, self.val);
        editor.settings_mut(tool).color = c;
        self.hex = hex_of(c);
    }

    /// Typing into the hex field or the font search. Returns whether the key was consumed.
    pub fn on_key(&mut self, e: &KeyDownEvent, editor: &mut Editor, tool: DrawTool) -> bool {
        let Some(field) = self.field else { return false };
        let k = &e.keystroke;
        let text = match field {
            Field::Hex => &mut self.hex,
            Field::FontSearch => &mut self.font_search,
        };
        match k.key.as_str() {
            "escape" => {
                // Escape closes the dropdown or leaves the field, back to the overlay.
                self.field = None;
                self.font_open = false;
                return true;
            }
            "enter" | "tab" => {
                self.field = None;
                return true;
            }
            "backspace" => {
                text.pop();
            }
            _ if k.modifiers.control => return true,
            _ => match k.key_char.as_deref() {
                Some(ch) if !ch.is_empty() && !ch.chars().any(char::is_control) => text.push_str(ch),
                _ => return true,
            },
        }
        if field == Field::Hex
            && let Some(c) = parse_hex(&self.hex)
        {
            // A partial hex while typing is ignored until it parses.
            editor.settings_mut(tool).color = c;
            (self.hue, self.sat, self.val) = color_to_hsv(c);
        }
        true
    }

    fn slider_value(&self, s: Slider, b: &BrushSettings) -> (f32, f32, f32) {
        match s {
            Slider::Size => (b.thickness, 1.0, 200.0),
            Slider::Opacity => (b.opacity * 100.0, 0.0, 100.0),
            Slider::Hardness => (b.hardness * 100.0, 0.0, 100.0),
        }
    }
}

fn set_slider(editor: &mut Editor, tool: DrawTool, s: Slider, v: f32) {
    let b = editor.settings_mut(tool);
    match s {
        Slider::Size => b.thickness = v.round().clamp(1.0, 200.0),
        Slider::Opacity => b.opacity = (v.round() / 100.0).clamp(0.0, 1.0),
        Slider::Hardness => b.hardness = (v.round() / 100.0).clamp(0.0, 1.0),
    }
}

/// Natural height of the panel for a tool: sections vary.
pub fn panel_height(tool: DrawTool) -> f32 {
    let mut h = 20.0 + 26.0 + 26.0;
    if tool == DrawTool::Text {
        h += 32.0;
    }
    if tool.has_color() {
        h += 12.0 + 130.0 + 30.0 + 38.0;
    }
    h
}

fn label(text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.)).font_weight(gpui::FontWeight::LIGHT).text_color(theme::text()).child(text.into())
}

/// Records an element's bounds during paint, for mapping the mouse onto it.
fn track_bounds(cell: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().size_full()
}

fn frac(cell: &Cell<Bounds<Pixels>>, p: gpui::Point<Pixels>) -> (f32, f32) {
    let b = cell.get();
    let w = f32::from(b.size.width).max(1.0);
    let h = f32::from(b.size.height).max(1.0);
    (
        ((f32::from(p.x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0),
        ((f32::from(p.y) - f32::from(b.origin.y)) / h).clamp(0.0, 1.0),
    )
}

impl Overlay {
    fn panel_drag_to(&mut self, d: Drag, p: gpui::Point<Pixels>) {
        let Some(tool) = self.tool() else { return };
        match d {
            Drag::Slider(s) => {
                let idx = s as usize;
                let (fx, _) = frac(&self.panel.bounds[idx], p);
                let (_, min, max) = self.panel.slider_value(s, self.editor.settings(tool));
                set_slider(&mut self.editor, tool, s, min + fx * (max - min));
            }
            Drag::Sv => {
                let (fx, fy) = frac(&self.panel.bounds[3], p);
                self.panel.sat = fx;
                self.panel.val = 1.0 - fy;
                self.panel.apply_hsv(&mut self.editor, tool);
            }
            Drag::Hue => {
                let (_, fy) = frac(&self.panel.bounds[4], p);
                self.panel.hue = fy * 360.0;
                self.panel.apply_hsv(&mut self.editor, tool);
            }
        }
    }

    /// While a panel control is being dragged the mouse is captured: moves and the release
    /// arrive here from the window-level listeners in `view`.
    pub(super) fn panel_drag_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.panel.drag else { return false };
        self.panel_drag_to(d, e.position);
        self.after_panel_edit(cx);
        true
    }

    pub(super) fn panel_drag_end(&mut self) -> bool {
        self.panel.drag.take().is_some()
    }

    fn begin_panel_drag(&mut self, d: Drag, e: &MouseDownEvent, cx: &mut Context<Self>) {
        self.panel.drag = Some(d);
        self.panel.field = None;
        self.panel_drag_to(d, e.position);
        self.after_panel_edit(cx);
    }

    fn slider_row(&self, s: Slider, name: &'static str, tool: DrawTool, cx: &mut Context<Self>) -> Div {
        let (v, min, max) = self.panel.slider_value(s, self.editor.settings(tool));
        let f = ((v - min) / (max - min)).clamp(0.0, 1.0);
        let value = if s == Slider::Size { format!("{} px", v.round() as i32) } else { format!("{}%", v.round() as i32) };
        let (well, inner) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        let track = div()
            .id(ElementId::Name(format!("slider-{name}").into()))
            .flex_1()
            .h(px(18.))
            .relative()
            .cursor_default()
            .child(track_bounds(self.panel.bounds[s as usize].clone()))
            .child(well.absolute().left(px(0.)).right(px(0.)).top(px(6.)).h(px(6.)).child(inner))
            .child(
                // Raised thumb.
                div()
                    .absolute()
                    .top(px(0.))
                    .left(gpui::relative(f))
                    .ml(px(-5.))
                    .w(px(10.))
                    .h(px(18.))
                    .bg(theme::btn_bg())
                    .border_t_1()
                    .border_l_1()
                    .border_color(theme::bevel_light())
                    .child(div().size_full().border_b_1().border_r_1().border_color(theme::bevel_dark())),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.begin_panel_drag(Drag::Slider(s), e, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, window, cx| {
                // The wheel nudges the value without pressing anything.
                let Some(tool) = this.tool() else { return };
                let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                let (v, _, _) = this.panel.slider_value(s, this.editor.settings(tool));
                set_slider(&mut this.editor, tool, s, v + dy.signum());
                this.after_panel_edit(cx);
            }));
        div()
            .flex()
            .items_center()
            .mb(px(8.))
            .child(label(name).w(px(88.)))
            .child(track)
            .child(label(value).w(px(40.)).flex().justify_end())
    }

    fn color_section(&self, tool: DrawTool, cx: &mut Context<Self>) -> AnyElement {
        let color = self.editor.settings(tool).color;
        let hue = hsv_to_color(self.panel.hue, 1.0, 1.0);
        let hue_rgb = rgb(u32::from(hue.r()) << 16 | u32::from(hue.g()) << 8 | u32::from(hue.b()));
        let (sv_well, sv_inner) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        let sv = sv_well.child(
            sv_inner.child(
                div()
                    .id("sv")
                    .relative()
                    .w(px(SV_W))
                    .h(px(SV_H))
                    .overflow_hidden()
                    .bg(hue_rgb)
                    .child(track_bounds(self.panel.bounds[3].clone()))
                    .child(div().absolute().size_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(rgba(0xFFFFFFFF), 0.),
                        linear_color_stop(rgba(0xFFFFFF00), 1.),
                    )))
                    .child(div().absolute().size_full().bg(linear_gradient(
                        180.,
                        linear_color_stop(rgba(0x00000000), 0.),
                        linear_color_stop(rgba(0x000000FF), 1.),
                    )))
                    .child(
                        div()
                            .absolute()
                            .left(px(self.panel.sat * SV_W - 6.))
                            .top(px((1.0 - self.panel.val) * SV_H - 6.))
                            .size(px(11.))
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(0xFFFFFF)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.begin_panel_drag(Drag::Sv, e, cx);
                        }),
                    ),
            ),
        );
        // Six two-stop segments make the seven-stop hue ramp.
        let stops = [0xFF0000, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x0000FF, 0xFF00FF, 0xFF0000u32];
        let mut bar = div()
            .id("hue")
            .relative()
            .w(px(16.))
            .h(px(SV_H))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(track_bounds(self.panel.bounds[4].clone()));
        for w in stops.windows(2) {
            bar = bar.child(div().w_full().flex_1().bg(linear_gradient(
                180.,
                linear_color_stop(rgb(w[0]), 0.),
                linear_color_stop(rgb(w[1]), 1.),
            )));
        }
        let bar = bar
            .child(
                div()
                    .absolute()
                    .left(px(-1.))
                    .top(px(self.panel.hue / 360.0 * SV_H - 2.))
                    .w(px(18.))
                    .h(px(4.))
                    .border_2()
                    .border_color(rgb(0xFFFFFF)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.begin_panel_drag(Drag::Hue, e, cx);
                }),
            );
        let (hue_well, hue_inner) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());

        let preview_rgb = rgb(u32::from(color.r()) << 16 | u32::from(color.g()) << 8 | u32::from(color.b()));
        let (pw, pi) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        let armed = self.eyedropper_armed;
        let eyedropper = theme::icon_button("eyedropper", 26., 22., armed, true)
            .child(svg().path("icons/eyedropper.svg").size(px(14.)).text_color(theme::text()))
            .tooltip(theme::tooltip(
                "Color picker. Right-click and hold on the image to pick a colour. Click here to keep it armed until you right-click.",
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                let armed = !this.eyedropper_armed;
                this.arm_eyedropper(armed, cx);
            }));
        let hex_focused = self.panel.field == Some(Field::Hex);
        let (hw, hi) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        let hex = hw.w(px(84.)).h(px(22.)).child(
            hi.id("hex")
                .flex()
                .items_center()
                .px(px(4.))
                .text_size(px(11.))
                .text_color(theme::text())
                .cursor_text()
                .child(self.panel.hex.clone())
                .when_caret(hex_focused)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.panel.field = Some(Field::Hex);
                        cx.notify();
                    }),
                ),
        );
        let reset = theme::button("reset-color", label("Reset").px(px(2.)))
            .min_h(px(22.))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(tool) = this.tool() {
                    let c = Color::from_rgb((RESET_COLOR >> 16) as u8, (RESET_COLOR >> 8) as u8, RESET_COLOR as u8);
                    this.panel.set_color(c, &mut this.editor, tool);
                    this.after_panel_edit(cx);
                }
            }));

        let mut swatches = div().flex().flex_wrap().w_full();
        for (i, c) in PALETTE.iter().copied().enumerate() {
            let (sw, si) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
            swatches = swatches.child(
                div().w(gpui::relative(1.0 / 8.0)).p(px(1.5)).child(
                    sw.id(("swatch", i))
                        .bg(rgb(c))
                        .h(px(16.))
                        .cursor_pointer()
                        .child(si)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(tool) = this.tool() {
                                    let col = Color::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
                                    this.panel.set_color(col, &mut this.editor, tool);
                                    this.after_panel_edit(cx);
                                }
                            }),
                        ),
                ),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(div().h(px(1.)).mt(px(2.)).bg(theme::rule_dark()))
            .child(div().h(px(1.)).mb(px(8.)).bg(theme::rule_light()))
            .child(div().flex().mb(px(8.)).child(sv).child(hue_well.ml(px(8.)).child(hue_inner.child(bar))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .mb(px(8.))
                    .child(pw.size(px(22.)).mr(px(6.)).bg(preview_rgb).child(pi))
                    .child(eyedropper)
                    .child(div().flex_1())
                    .child(hex)
                    .child(reset.ml(px(6.))),
            )
            .child(swatches)
            .into_any_element()
    }

    fn font_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let family: SharedString = self.editor.settings(DrawTool::Text).font_family.clone().into();
        let open = self.panel.font_open;
        let (well, inner) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        let combo = div()
            .id("font-combo")
            .flex_1()
            .h(px(24.))
            .cursor_default()
            .child(
                well.size_full()
                    .when_open_style(open)
                    .hover(|s| s.bg(theme::accent()))
                    .child(
                        inner
                            .flex()
                            .items_center()
                            .pl(px(7.))
                            .pr(px(7.))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(family.clone())
                                    .text_size(px(14.))
                                    .child(family.clone()),
                            )
                            .child(svg().path("icons/chevron.svg").w(px(7.)).h(px(4.5)).text_color(theme::text())),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.panel.font_open = !this.panel.font_open;
                    // Every open starts with a clean search, which takes the keys.
                    this.panel.font_search.clear();
                    this.panel.field = this.panel.font_open.then_some(Field::FontSearch);
                    cx.notify();
                }),
            );
        div()
            .flex()
            .items_center()
            .mb(px(8.))
            .child(label("Font").w(px(88.)))
            .child(combo)
            .into_any_element()
    }

    /// The font dropdown, floating over the panel below the combo.
    pub(super) fn font_dropdown(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.panel.font_open {
            return None;
        }
        let needle = self.panel.font_search.to_lowercase();
        let entries: Rc<Vec<fonts::FontEntry>> = Rc::new(
            fonts::families().iter().filter(|f| needle.is_empty() || f.family.to_lowercase().contains(&needle)).cloned().collect(),
        );
        let current = self.editor.settings(DrawTool::Text).font_family.clone();
        let entity = cx.entity();
        let rows = entries.clone();
        let list = uniform_list("fonts", entries.len(), move |range, _, _| {
            range
                .map(|i| {
                    let e = &rows[i];
                    let family: SharedString = e.family.clone().into();
                    let selected = e.family.eq_ignore_ascii_case(&current);
                    let entity = entity.clone();
                    let mut row = div()
                        .id(("font", i))
                        .h(px(24.))
                        .px(px(7.))
                        .flex()
                        .items_center()
                        .when_bg(selected, theme::accent_dim())
                        .hover(|s| s.bg(theme::accent()))
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_family(family.clone())
                                .text_size(px(14.))
                                .text_color(theme::text())
                                .child(family.clone()),
                        )
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let family = family.to_string();
                            entity.update(cx, |this, cx| {
                                this.editor.settings_mut(DrawTool::Text).font_family = family;
                                this.panel.font_open = false;
                                this.panel.field = None;
                                this.after_panel_edit(cx);
                            });
                        });
                    // A family without Cyrillic draws Russian as boxes in the screenshot, and
                    // this row's own preview cannot show that: it is dimmed instead.
                    if !e.supports_cyrillic {
                        row = row.opacity(0.45).tooltip(theme::tooltip("No Cyrillic glyphs in this font"));
                    }
                    row
                })
                .collect()
        })
        .h(px((entries.len() as f32 * 24.0).min(280.0)));

        let (sw, si) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
        let search_text = if self.panel.font_search.is_empty() {
            div().opacity(0.45).child("Search fonts")
        } else {
            div().child(self.panel.font_search.clone())
        };
        Some(
            div()
                .id("font-dropdown")
                .occlude()
                .w(px(PANEL_WIDTH - 20.0 - 88.0))
                .bg(theme::panel_bg())
                .border_t_1()
                .border_l_1()
                .border_color(theme::bevel_light())
                .child(
                    div()
                        .border_b_1()
                        .border_r_1()
                        .border_color(theme::bevel_dark())
                        .p(px(2.))
                        .child(
                            sw.m(px(2.)).mb(px(4.)).h(px(22.)).child(
                                si.flex()
                                    .items_center()
                                    .px(px(4.))
                                    .text_size(px(11.))
                                    .text_color(theme::text())
                                    .child(search_text)
                                    .when_caret(true),
                            ),
                        )
                        .child(list),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element(),
        )
    }

    /// The strip and the panel above the toolbar. `toolbar` is the toolbar's top-left and
    /// `screen` the monitor rect, both logical.
    pub(super) fn render_settings(
        &mut self,
        toolbar_x: f32,
        toolbar_y: f32,
        toolbar_h: f32,
        screen: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tool = self.tool()?;
        if self.ocr.active {
            return None;
        }
        const GAP: f32 = 3.0;
        let (sl, st) = (f32::from(screen.origin.x), f32::from(screen.origin.y));
        let sr = sl + f32::from(screen.size.width);
        let x = clamp_to(toolbar_x, sl, sr - PANEL_WIDTH);
        let height = panel_height(tool);
        let mut strip_closed = toolbar_y - STRIP_HEIGHT - GAP;
        let mut open_top = toolbar_y - GAP - height - STRIP_HEIGHT;
        if strip_closed < st {
            // No room above: the strip parks below the toolbar and the panel opens downwards.
            strip_closed = toolbar_y + toolbar_h + GAP;
            open_top = strip_closed;
        } else if open_top < st {
            open_top = st; // too tall to fit above the toolbar: pinned to the top edge
        }
        let (progress, animating) = self.panel.progress();
        if animating {
            window.request_animation_frame();
        }
        let strip_y = strip_closed + (open_top - strip_closed) * progress;
        let panel_h = height * progress;

        let strip = div()
            .id("settings-strip")
            .occlude()
            .absolute()
            .left(px(x))
            .top(px(strip_y))
            .w(px(PANEL_WIDTH))
            .h(px(STRIP_HEIGHT))
            .rounded(px(6.))
            .bg(theme::panel_bg())
            .cursor_default()
            .group("strip")
            .child(
                // Brightens under the pointer, like every other HUD control.
                div().absolute().size_full().rounded(px(6.)).bg(theme::accent()).opacity(0.).group_hover("strip", |s| s.opacity(0.5)),
            )
            .child(
                div().size_full().flex().items_center().justify_center().children(
                    (0..3).map(|_| div().mx(px(2.)).size(px(3.)).rounded_full().bg(rgba(0xFFFFFFDD))),
                ),
            )
            .tooltip(theme::tooltip("Tool settings"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(cx)));

        let mut content = div().w(px(PANEL_WIDTH)).p(px(10.)).flex().flex_col().font_family(theme::FONT);
        content = content.child(self.slider_row(Slider::Size, "Size", tool, cx));
        if tool == DrawTool::Text {
            content = content.child(self.font_row(cx));
        }
        if tool.is_eraser() {
            // Erasers show the Photoshop hardness as "Opacity": the centre always erases 100%,
            // lower values fade the erase toward the rim.
            content = content.child(self.slider_row(Slider::Hardness, "Opacity", tool, cx));
        } else {
            let name = if tool.is_effect() { "Strength" } else { "Opacity" };
            content = content.child(self.slider_row(Slider::Opacity, name, tool, cx));
        }
        if tool.has_color() {
            content = content.child(self.color_section(tool, cx));
        }

        let panel = (panel_h > 0.5).then(|| {
            div()
                .id("settings-panel")
                .occlude()
                .absolute()
                .left(px(x))
                .top(px(strip_y + STRIP_HEIGHT))
                .w(px(PANEL_WIDTH))
                .h(px(panel_h))
                .overflow_hidden()
                .rounded(px(8.))
                .bg(theme::panel_bg())
                .child(content)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        });

        let dropdown = (progress >= 1.0)
            .then(|| self.font_dropdown(cx))
            .flatten()
            .map(|d| div().absolute().left(px(x + 10.0 + 88.0)).top(px(strip_y + STRIP_HEIGHT + 10.0 + 26.0 + 25.0)).child(d));

        Some(div().absolute().size_full().child(strip).children(panel).children(dropdown).into_any_element())
    }
}

/// Clamp without the panic: a panel wider than its screen pins to the left edge.
pub fn clamp_to(v: f32, min: f32, max: f32) -> f32 {
    if max <= min { min } else { v.clamp(min, max) }
}

trait PanelStyleExt: Sized {
    fn when_caret(self, on: bool) -> Self;
    fn when_open_style(self, open: bool) -> Self;
    fn when_bg(self, on: bool, c: gpui::Rgba) -> Self;
}

impl<E: ParentElement + Styled> PanelStyleExt for E {
    fn when_caret(self, on: bool) -> Self {
        if on { self.child(div().ml(px(1.)).w(px(1.)).h(px(13.)).bg(theme::text())) } else { self }
    }

    fn when_open_style(self, open: bool) -> Self {
        // Open reads as pressed: the dimmed accent, like a latched button.
        if open { self.bg(theme::accent_dim()) } else { self }
    }

    fn when_bg(self, on: bool, c: gpui::Rgba) -> Self {
        if on { self.bg(c) } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrips_through_rgb() {
        for c in PALETTE {
            let col = Color::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
            let (h, s, v) = color_to_hsv(col);
            assert_eq!(hsv_to_color(h, s, v), col, "{c:06X}");
        }
    }

    #[test]
    fn hex_parsing_accepts_short_and_long_forms() {
        assert_eq!(parse_hex("#ff3b30"), Some(Color::from_rgb(0xFF, 0x3B, 0x30)));
        assert_eq!(parse_hex("fff"), Some(Color::WHITE));
        assert_eq!(parse_hex("#12"), None);
        assert_eq!(hex_of(Color::from_rgb(1, 2, 3)), "#010203");
    }

    #[test]
    fn panel_heights_follow_the_tool() {
        assert!(panel_height(DrawTool::Text) > panel_height(DrawTool::Brush));
        assert!(panel_height(DrawTool::Brush) > panel_height(DrawTool::Eraser));
        assert_eq!(panel_height(DrawTool::Eraser), panel_height(DrawTool::Blur));
    }
}
