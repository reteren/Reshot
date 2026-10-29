//! Half-Life 2 / Source VGUI styling shared by every window: the tray menu, the overlay HUD,
//! the radial menu, the recording HUD and the settings dialog. Values are the reference ones
//! from github.com/arlagonix/half-life-screen, as the C# build used them; the one deliberate
//! change is the accent, HL2 orange #ff9c00 → Reshot teal #3C9898.
//!
//! VGUI bevels colour the top/left and bottom/right edges differently. gpui, like WPF, draws
//! one border colour per element, so a bevel is two nested elements, exactly as the XAML did.

// The full reference palette is kept in one place; the settings dialog (R9) uses the rest.
#![allow(dead_code)]

use gpui::{
    AnyElement, Div, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Rgba, SharedString,
    Stateful, StatefulInteractiveElement, Styled, div, px, rgb, rgba, svg,
};

pub const FONT: &str = "Segoe UI";

pub fn panel_bg() -> Rgba {
    rgba(0x767676D9)
}
/// The settings dialog's lighter panel (reference 0.85, Reshot runs 0.6).
pub fn dialog_bg() -> Rgba {
    rgba(0x76767699)
}
pub fn text() -> Rgba {
    rgb(0xFFFFFF)
}
pub fn accent() -> Rgba {
    rgb(0x3C9898)
}
pub fn rule_dark() -> Rgba {
    rgb(0x434343)
}
pub fn rule_light() -> Rgba {
    rgb(0xBBBABA)
}
pub fn btn_bg() -> Rgba {
    rgb(0x7B7B7B)
}
pub fn bevel_light() -> Rgba {
    rgb(0xBBBBBB)
}
pub fn bevel_dark() -> Rgba {
    rgb(0x454545)
}
pub fn check_bg() -> Rgba {
    rgb(0x555555)
}
pub fn check_dark() -> Rgba {
    rgb(0x424343)
}
pub fn check_light() -> Rgba {
    rgb(0xBABABA)
}
pub fn input_bg() -> Rgba {
    rgb(0x5A5A5A)
}
pub fn input_dark() -> Rgba {
    rgb(0x343434)
}
pub fn input_light() -> Rgba {
    rgb(0xB7B7B7)
}
pub fn tab_light() -> Rgba {
    rgb(0xC0BFBF)
}
pub fn tab_dark() -> Rgba {
    rgb(0x454543)
}

/// Body text: Roboto/Segoe UI 12, white, light weight.
pub fn body() -> Div {
    div().font_family(FONT).text_size(px(12.)).text_color(text()).font_weight(FontWeight::LIGHT)
}

/// The small bold caps title over a panel ("RESHOT").
pub fn title(s: impl Into<SharedString>) -> Div {
    div().font_family(FONT).text_size(px(10.)).font_weight(FontWeight::BOLD).text_color(text()).child(s.into())
}

/// The translucent rounded panel every popup sits on.
pub fn panel() -> Div {
    div().bg(panel_bg()).rounded(px(8.)).p(px(8.)).font_family(FONT).text_color(text())
}

/// A raised button: light top/left, dark bottom/right; hover fills with the accent and a
/// press inverts the bevel, exactly as the reference Button does.
pub fn button(id: impl Into<ElementId>, content: impl IntoElement) -> Stateful<Div> {
    let id = id.into();
    let group: SharedString = format!("btn-{id}").into();
    div()
        .id(id)
        .group(group.clone())
        .min_h(px(24.))
        .flex()
        .bg(btn_bg())
        .hover(|s| s.bg(accent()))
        .border_t_1()
        .border_l_1()
        .border_color(bevel_light())
        .active(|s| s.border_color(bevel_dark()))
        .cursor_pointer()
        .child(
            div()
                .id(SharedString::from(format!("{group}-inner")))
                .flex_1()
                .flex()
                .items_center()
                .px(px(6.))
                .border_b_1()
                .border_r_1()
                .border_color(bevel_dark())
                .group_active(group, |s| s.border_color(bevel_light()))
                .child(content),
        )
}

/// A sunken well (inputs, checkbox boxes, sliders): dark top/left, light bottom/right.
pub fn inset(dark: Rgba, light: Rgba, bg: Rgba) -> (Div, Div) {
    let outer = div().bg(bg).border_t_1().border_l_1().border_color(dark);
    let inner = div().size_full().border_b_1().border_r_1().border_color(light);
    (outer, inner)
}

/// The engraved two-tone rule between groups.
pub fn separator() -> Div {
    div()
        .w_full()
        .border_t_1()
        .border_color(rule_dark())
        .child(div().w_full().border_t_1().border_color(rule_light()))
}

/// The VGUI checkbox square: sunken, with a white tick when checked.
pub fn check_box(checked: bool) -> AnyElement {
    let (outer, inner) = inset(check_dark(), check_light(), check_bg());
    outer
        .size(px(12.))
        .flex_none()
        .child(inner.flex().items_center().justify_center().when_some_tick(checked))
        .into_any_element()
}

trait TickExt {
    fn when_some_tick(self, checked: bool) -> Self;
}

impl TickExt for Div {
    fn when_some_tick(self, checked: bool) -> Self {
        if checked {
            self.child(svg().path("icons/tick.svg").size(px(8.)).text_color(text()))
        } else {
            self
        }
    }
}

/// A checkbox row: box plus label, the whole row lighting up with the accent on hover.
pub fn check_row(id: impl Into<ElementId>, checked: bool, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .min_h(px(24.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(accent()))
        .child(check_box(checked))
        .child(label.into())
}
