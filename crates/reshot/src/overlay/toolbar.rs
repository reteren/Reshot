//! The toolbar (SPEC §5): icon tabs left to right in the order the digit keys follow, the
//! sub-tool flyouts (right-click a tab, or rest on it for half a second), and the audio
//! source picker behind a right-click on the two record tools.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px, svg,
};
use reshot_core::editor::DrawTool;
use reshot_core::selection::ShapeKind;
use reshot_recording::{AudioSources, WindowAudioSource};

use super::{Flyout, FlyoutState, Mode, Overlay};
use crate::theme;

const BTN_W: f32 = 30.0;
const BTN_H: f32 = 28.0;
/// Each button has a 1 px margin on both sides.
const SLOT: f32 = BTN_W + 2.0;
const PAD: f32 = 4.0;
/// The engraved divider between groups: 4+1+4 then 0+1+4 pixels.
const DIVIDER: f32 = 14.0;
pub const TOOLBAR_W: f32 = PAD * 2.0 + SLOT * 14.0 + DIVIDER * 2.0;
pub const TOOLBAR_H: f32 = BTN_H + PAD * 2.0;
const FLYOUT_PAD: f32 = 3.0;
const HOVER_OPEN: Duration = Duration::from_millis(500);
const HOVER_CLOSE: Duration = Duration::from_millis(450);
const FLYOUT_ANIM: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Select,
    Brush,
    Shapes,
    Lines,
    Eraser,
    Effects,
    Ocr,
    Text,
    Record,
    Audio,
    Copy,
    Save,
    SaveAs,
    Close,
}

impl Tab {
    const ALL: [Tab; 14] = [
        Tab::Select,
        Tab::Brush,
        Tab::Shapes,
        Tab::Lines,
        Tab::Eraser,
        Tab::Effects,
        Tab::Ocr,
        Tab::Text,
        Tab::Record,
        Tab::Audio,
        Tab::Copy,
        Tab::Save,
        Tab::SaveAs,
        Tab::Close,
    ];

    /// The tab's left edge inside the toolbar.
    fn x(self) -> f32 {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        let dividers = match i {
            0..=9 => 0.0,
            10..=12 => DIVIDER,
            _ => DIVIDER * 2.0,
        };
        PAD + i as f32 * SLOT + dividers + 1.0
    }

    fn flyout(self) -> Option<Flyout> {
        match self {
            Tab::Select => Some(Flyout::Select),
            Tab::Shapes => Some(Flyout::Shapes),
            Tab::Lines => Some(Flyout::Lines),
            Tab::Eraser => Some(Flyout::Eraser),
            Tab::Effects => Some(Flyout::Effects),
            _ => None,
        }
    }

    fn icon(self) -> (&'static str, f32, f32) {
        match self {
            Tab::Select => ("icons/sel-rect.svg", 17., 14.),
            Tab::Brush => ("icons/brush.svg", 20., 20.),
            Tab::Shapes => ("icons/square.svg", 15., 15.),
            Tab::Lines => ("icons/arrow.svg", 18., 18.),
            Tab::Eraser => ("icons/eraser.svg", 20., 20.),
            Tab::Effects => ("icons/pixelate.svg", 18., 18.),
            Tab::Ocr => ("icons/ocr.svg", 18., 20.),
            Tab::Text => ("", 0., 0.),
            Tab::Record => ("icons/record.svg", 20., 19.),
            Tab::Audio => ("icons/audio.svg", 18., 20.),
            Tab::Copy => ("icons/copy.svg", 19., 19.),
            Tab::Save => ("icons/save.svg", 19., 19.),
            Tab::SaveAs => ("icons/save-as.svg", 21., 19.),
            Tab::Close => ("icons/close.svg", 20., 20.),
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Tab::Select => "Selection (1)",
            Tab::Brush => "Brush (2). Right click for color picker",
            Tab::Shapes => "Shapes (3)",
            Tab::Lines => "Lines (4)",
            Tab::Eraser => "Eraser (5)",
            Tab::Effects => "Effects (6)",
            Tab::Ocr => "Copy text from image, OCR (7). Right-click: AUTO / RU / EN",
            Tab::Text => "Text (8)",
            Tab::Record => "Record video (9). Right-click for audio sources",
            Tab::Audio => "Record audio (0). Right-click for sources",
            Tab::Copy => "Copy to clipboard",
            Tab::Save => "Save",
            Tab::SaveAs => "Save as…",
            Tab::Close => "Close (Esc)",
        }
    }
}

fn tool_icon(t: DrawTool) -> (&'static str, f32, f32, &'static str) {
    match t {
        DrawTool::Square => ("icons/square.svg", 15., 15., "Square"),
        DrawTool::Circle => ("icons/circle.svg", 15., 15., "Circle"),
        DrawTool::Triangle => ("icons/triangle.svg", 18., 17., "Triangle"),
        DrawTool::Arrow => ("icons/arrow.svg", 18., 18., "Arrow"),
        DrawTool::Line => ("icons/line.svg", 18., 18., "Line"),
        DrawTool::Eraser => ("icons/eraser.svg", 20., 20., "Eraser"),
        DrawTool::FilterEraser => ("icons/filter-eraser.svg", 22., 20., "Filter eraser (removes blur / pixelation)"),
        DrawTool::AbsoluteEraser => ("icons/abs-eraser.svg", 22., 20., "Absolute eraser (to transparency)"),
        DrawTool::Pixelize => ("icons/pixelate.svg", 18., 18., "Pixelate"),
        DrawTool::Blur => ("icons/blur.svg", 18., 18., "Blur"),
        DrawTool::Brush => ("icons/brush.svg", 20., 20., "Brush"),
        DrawTool::Text => ("", 0., 0., "Text"),
    }
}

fn shape_icon(k: ShapeKind) -> (&'static str, f32, f32, &'static str) {
    match k {
        ShapeKind::Rectangle => ("icons/sel-rect.svg", 17., 14., "Rectangle"),
        ShapeKind::Ellipse => ("icons/sel-ellipse.svg", 17., 15., "Ellipse"),
        ShapeKind::Triangle => ("icons/sel-triangle.svg", 18., 17., "Triangle"),
        ShapeKind::Lasso => ("icons/sel-lasso.svg", 20., 20., "Lasso"),
        ShapeKind::Polygon => ("icons/sel-polygon.svg", 20., 20., "Polygon"),
    }
}

fn icon(path: &'static str, w: f32, h: f32) -> AnyElement {
    if path.is_empty() {
        // The Text tab is a bold "T", not an icon.
        return div().text_size(px(17.)).font_weight(gpui::FontWeight::BOLD).text_color(theme::text()).mt(px(-2.)).child("T").into_any_element();
    }
    svg().path(path).w(px(w)).h(px(h)).text_color(theme::text()).into_any_element()
}

fn divider() -> Div {
    div()
        .flex()
        .child(div().w(px(1.)).mx(px(4.)).my(px(3.)).h(px(BTN_H - 6.)).bg(theme::rule_dark()))
        .child(div().w(px(1.)).mr(px(4.)).my(px(3.)).h(px(BTN_H - 6.)).bg(theme::rule_light()))
}

/// The audio source picker opened by right-clicking Record or Audio.
pub struct SourceMenu {
    pub for_video: bool,
    pub system: bool,
    pub mic: bool,
    pub windows: Vec<(WindowAudioSource, bool)>,
    pub x: f32,
    pub y: f32,
}

impl Overlay {
    fn tab_is_on(&self, tab: Tab) -> bool {
        if self.ocr.active {
            return tab == Tab::Ocr;
        }
        match (tab, self.mode) {
            (Tab::Select, Mode::Select) => true,
            (Tab::Brush, Mode::Draw(DrawTool::Brush)) => true,
            (Tab::Text, Mode::Draw(DrawTool::Text)) => true,
            (Tab::Shapes, Mode::Draw(t)) => matches!(t, DrawTool::Square | DrawTool::Circle | DrawTool::Triangle),
            (Tab::Lines, Mode::Draw(t)) => matches!(t, DrawTool::Line | DrawTool::Arrow),
            (Tab::Eraser, Mode::Draw(t)) => t.is_eraser(),
            (Tab::Effects, Mode::Draw(t)) => t.is_effect(),
            _ => false,
        }
    }

    fn click_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_flyout();
        self.source_menu = None;
        match tab {
            Tab::Select => self.set_select(cx),
            Tab::Brush => self.set_draw(DrawTool::Brush, cx),
            Tab::Shapes => self.set_draw(self.shapes_sub, cx),
            Tab::Lines => self.set_draw(self.lines_sub, cx),
            Tab::Eraser => self.set_draw(self.eraser_sub, cx),
            Tab::Effects => self.set_draw(self.effects_sub, cx),
            Tab::Ocr => self.run_ocr(cx),
            Tab::Text => self.set_draw(DrawTool::Text, cx),
            Tab::Record => self.request_recording(None, window, cx),
            Tab::Audio => self.start_audio_defaults(window, cx),
            Tab::Copy => self.export(super::ExportKind::Copy, window, cx),
            Tab::Save => self.export(super::ExportKind::Save, window, cx),
            Tab::SaveAs => self.export(super::ExportKind::SaveAs, window, cx),
            Tab::Close => self.close(window, cx),
        }
    }

    fn right_click_tab(&mut self, tab: Tab, tx: f32, ty: f32, cx: &mut Context<Self>) {
        self.hover_generation += 1;
        match tab {
            Tab::Ocr => return self.toggle_ocr_language(cx),
            Tab::Record | Tab::Audio => return self.open_source_menu(tab == Tab::Record, tx + tab.x(), ty, cx),
            _ => {}
        }
        let Some(f) = tab.flyout() else { return };
        let reopen = self.flyout.as_ref().is_none_or(|o| o.kind != f);
        self.close_flyout();
        if reopen {
            self.flyout = Some(FlyoutState { kind: f, by_hover: false, opened: Instant::now() });
        }
        self.panel.close();
        cx.notify();
    }

    fn hover_tab(&mut self, tab: Tab, hovered: bool, cx: &mut Context<Self>) {
        self.hover_generation += 1;
        let generation = self.hover_generation;
        let Some(f) = tab.flyout() else { return };
        if hovered {
            // Resting on a tab for a moment reveals its sub-tools.
            if self.flyout.as_ref().is_some_and(|o| o.kind == f) {
                return;
            }
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOVER_OPEN).await;
                let _ = this.update(cx, |this, cx| {
                    if this.hover_generation == generation && this.flyout.as_ref().is_none_or(|o| o.kind != f) {
                        this.flyout = Some(FlyoutState { kind: f, by_hover: true, opened: Instant::now() });
                        this.panel.close();
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.arm_flyout_close(cx);
        }
    }

    /// A flyout opened by hovering goes away once the pointer has left both it and its tab;
    /// the delay covers the gap crossed on the way from one to the other. One opened with the
    /// right button stays until dismissed deliberately.
    fn arm_flyout_close(&mut self, cx: &mut Context<Self>) {
        if !self.flyout.as_ref().is_some_and(|f| f.by_hover) {
            return;
        }
        let generation = self.hover_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_CLOSE).await;
            let _ = this.update(cx, |this, cx| {
                if this.hover_generation == generation && !this.flyout_hovered {
                    this.close_flyout();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_source_menu(&mut self, for_video: bool, x: f32, y: f32, cx: &mut Context<Self>) {
        self.close_flyout();
        self.panel.close();
        let windows = reshot_recording::list_window_audio_sources().into_iter().map(|w| (w, true)).collect();
        self.source_menu = Some(SourceMenu { for_video, system: true, mic: true, windows, x, y });
        cx.notify();
    }

    fn start_from_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.source_menu.take() else { return };
        let pids: Vec<u32> = m.windows.iter().filter(|(_, on)| *on).map(|(w, _)| w.process_id).collect();
        let all = !m.windows.is_empty() && pids.len() == m.windows.len();
        let full = m.system && (m.windows.is_empty() || all);
        let sources = AudioSources {
            mic: if m.for_video { self.settings.video.audio.mic } else { m.mic },
            mic_device: Some(self.settings.audio.mic_device.clone()),
            system_full: full,
            include_pids: if full { Vec::new() } else { pids },
        };
        if m.for_video {
            self.request_recording(Some(sources), window, cx);
        } else {
            self.start_audio(sources, window, cx);
        }
    }

    pub(super) fn render_toolbar(&self, x: f32, y: f32, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().flex().items_center();
        for (i, tab) in Tab::ALL.into_iter().enumerate() {
            if i == 10 || i == 13 {
                row = row.child(divider());
            }
            let (path, w, h) = tab.icon();
            let mut b = theme::icon_button(ElementId::Name(format!("tab-{tab:?}").into()), BTN_W, BTN_H, self.tab_is_on(tab), false, icon(path, w, h))
                .mx(px(1.))
                .tooltip(theme::tooltip(tab.tooltip()))
                .on_click(cx.listener(move |this, _, window, cx| this.click_tab(tab, window, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.right_click_tab(tab, x, y, cx);
                    }),
                );
            if tab.flyout().is_some() {
                b = b.on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_tab(tab, *hovered, cx)));
            }
            row = row.child(b);
        }
        div()
            .id("toolbar")
            .occlude()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(TOOLBAR_W))
            .h(px(TOOLBAR_H))
            .p(px(PAD))
            .rounded(px(8.))
            .bg(theme::panel_bg())
            .font_family(theme::FONT)
            .child(row)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }

    /// The open flyout, centred over its tab above the toolbar (or below it when there is no
    /// room), rising into place out of the tab.
    pub(super) fn render_flyout(
        &mut self,
        toolbar_x: f32,
        toolbar_y: f32,
        strip_visible: bool,
        screen: (f32, f32, f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let geo = (toolbar_x, toolbar_y, strip_visible, screen);
        let mut out = Vec::new();
        if let Some((kind, since)) = self.closing_flyout {
            let t = (since.elapsed().as_secs_f32() / FLYOUT_ANIM.as_secs_f32()).min(1.0);
            let still_open = self.flyout.as_ref().is_some_and(|f| f.kind == kind);
            if t >= 1.0 || still_open {
                self.closing_flyout = None;
            } else {
                window.request_animation_frame();
                out.push(self.flyout_element(kind, (1.0 - t).powi(3), false, geo, cx));
            }
        }
        if let Some(f) = &self.flyout {
            let t = (f.opened.elapsed().as_secs_f32() / FLYOUT_ANIM.as_secs_f32()).min(1.0);
            if t < 1.0 {
                window.request_animation_frame();
            }
            out.push(self.flyout_element(f.kind, 1.0 - (1.0 - t).powi(3), true, geo, cx));
        }
        out
    }

    /// One flyout at visibility `e` (0 hidden, 1 in place): it rises out of its tab while
    /// appearing and sinks back while fading. Only the open one takes clicks.
    fn flyout_element(
        &self,
        kind: Flyout,
        e: f32,
        interactive: bool,
        (toolbar_x, toolbar_y, strip_visible, screen): (f32, f32, bool, (f32, f32, f32, f32)),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (tab, items): (Tab, Vec<FlyoutEntry>) = match kind {
            Flyout::Select => (
                Tab::Select,
                [ShapeKind::Rectangle, ShapeKind::Ellipse, ShapeKind::Triangle, ShapeKind::Lasso, ShapeKind::Polygon]
                    .into_iter()
                    .map(|k| {
                        let (p, w, h, tip) = shape_icon(k);
                        (ElementId::Name(format!("shape-{k:?}").into()), p, w, h, tip, self.shape == k, FlyoutItem::Shape(k))
                    })
                    .collect(),
            ),
            kind => {
                let (tab, tools): (Tab, &[DrawTool]) = match kind {
                    Flyout::Shapes => (Tab::Shapes, &[DrawTool::Square, DrawTool::Circle, DrawTool::Triangle]),
                    Flyout::Lines => (Tab::Lines, &[DrawTool::Arrow, DrawTool::Line]),
                    Flyout::Eraser => {
                        (Tab::Eraser, &[DrawTool::Eraser, DrawTool::FilterEraser, DrawTool::AbsoluteEraser])
                    }
                    _ => (Tab::Effects, &[DrawTool::Pixelize, DrawTool::Blur]),
                };
                (
                    tab,
                    tools
                        .iter()
                        .map(|&t| {
                            let (p, w, h, tip) = tool_icon(t);
                            let on = self.mode == Mode::Draw(t);
                            (ElementId::Name(format!("tool-{t:?}").into()), p, w, h, tip, on, FlyoutItem::Tool(t))
                        })
                        .collect(),
                )
            }
        };
        let fw = FLYOUT_PAD * 2.0 + SLOT;
        let fh = FLYOUT_PAD * 2.0 + BTN_H * items.len() as f32;
        let (sl, st, sr, sb) = screen;
        let mut fy = toolbar_y - fh - 4.0;
        if strip_visible {
            fy -= super::panel::STRIP_HEIGHT + 3.0;
        }
        if fy < st {
            fy = toolbar_y + TOOLBAR_H + 4.0;
        }
        let fx = super::panel::clamp_to(toolbar_x + tab.x() + (BTN_W - fw) / 2.0, sl, sr - fw);
        let fy = super::panel::clamp_to(fy, st, sb - fh);

        let mut col = div().flex().flex_col();
        for (id, path, w, h, tip, on, item) in items {
            col = col.child(
                theme::icon_button(id, BTN_W, BTN_H, on, false, icon(path, w, h))
                    .mx(px(1.))
                    .tooltip(theme::tooltip(tip))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_flyout();
                        match item {
                            FlyoutItem::Shape(k) => this.set_shape(k, cx),
                            FlyoutItem::Tool(t) => this.set_draw(t, cx),
                        }
                    })),
            );
        }
        let el = div()
            .absolute()
            .left(px(fx))
            .top(px(fy + (1.0 - e) * 14.0))
            .opacity(e)
            .p(px(FLYOUT_PAD))
            .rounded(px(6.))
            .bg(theme::panel_bg())
            .child(col);
        if !interactive {
            return el.into_any_element();
        }
        el.id("flyout")
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.flyout_hovered = *hovered;
                if *hovered {
                    this.hover_generation += 1;
                } else {
                    this.arm_flyout_close(cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }

    /// The source picker, styled like the rest of the HUD. Everything starts checked (record
    /// all sound); unchecking windows narrows it to per-process loopback.
    pub(super) fn render_source_menu(&self, screen: (f32, f32, f32, f32), cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.source_menu.as_ref()?;
        let row = |id: ElementId, checked: Option<bool>, text: String| {
            div()
                .id(id)
                .px(px(7.))
                .py(px(4.))
                .flex()
                .items_center()
                .hover(|s| s.bg(theme::accent()))
                .child(div().w(px(20.)).flex_none().children(checked.map(|c| {
                    let (w, i) = theme::inset(theme::input_dark(), theme::input_light(), theme::input_bg());
                    w.size(px(12.)).child(i.flex().items_center().justify_center().children(
                        c.then(|| svg().path("icons/tick.svg").size(px(8.)).text_color(theme::text())),
                    ))
                })))
                .child(div().text_size(px(12.)).font_weight(gpui::FontWeight::LIGHT).text_color(theme::text()).child(text))
        };
        let sep = || {
            div()
                .mx(px(6.))
                .my(px(4.))
                .child(div().h(px(1.)).bg(theme::rule_dark()))
                .child(div().h(px(1.)).bg(theme::rule_light()))
        };
        let system_label = if m.for_video { "Select all" } else { "System audio (all windows)" };
        let mut list = div().flex().flex_col().child(row("src-system".into(), Some(m.system), system_label.into()).on_click(
            cx.listener(|this, _, _, cx| {
                if let Some(m) = &mut this.source_menu {
                    // Toggling it (re)selects or deselects every window.
                    m.system = !m.system;
                    let v = m.system;
                    m.windows.iter_mut().for_each(|(_, on)| *on = v);
                }
                cx.notify();
            }),
        ));
        if !m.for_video {
            list = list.child(row("src-mic".into(), Some(m.mic), "Microphone".into()).on_click(cx.listener(|this, _, _, cx| {
                if let Some(m) = &mut this.source_menu {
                    m.mic = !m.mic;
                }
                cx.notify();
            })));
        }
        list = list.child(sep());
        for (i, (w, on)) in m.windows.iter().enumerate() {
            let title: String = if w.title.chars().count() > 42 {
                w.title.chars().take(42).chain(std::iter::once('…')).collect()
            } else {
                w.title.clone()
            };
            list = list.child(row(("src-win", i).into(), Some(*on), format!("{}: {title}", w.process_name)).on_click(
                cx.listener(move |this, _, _, cx| {
                    if let Some(m) = &mut this.source_menu {
                        m.windows[i].1 = !m.windows[i].1;
                        m.system = !m.windows.is_empty() && m.windows.iter().all(|(_, on)| *on);
                    }
                    cx.notify();
                }),
            ));
        }
        let start = if m.for_video { "●  Start video recording" } else { "●  Start recording" };
        list = list.child(sep()).child(
            row("src-start".into(), None, start.into())
                .on_click(cx.listener(|this, _, window, cx| this.start_from_menu(window, cx))),
        );
        let (sl, st, sr, sb) = screen;
        let (mw, mh) = (320.0, (m.windows.len() as f32 + 5.0) * 24.0 + 16.0);
        let x = super::panel::clamp_to(m.x, sl, sr - mw);
        let y = super::panel::clamp_to(m.y - mh.min(520.0) - 4.0, st, sb - mh.min(520.0));
        Some(
            div()
                .id("source-menu")
                .occlude()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(mw))
                .max_h(px(520.))
                .overflow_y_scroll()
                .p(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme::bevel_dark())
                .bg(theme::panel_bg())
                .font_family(theme::FONT)
                .child(list)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .into_any_element(),
        )
    }
}

/// One flyout button: id, icon path and size, tooltip, whether it is the active one, and
/// what clicking it selects.
type FlyoutEntry = (ElementId, &'static str, f32, f32, &'static str, bool, FlyoutItem);

#[derive(Clone, Copy)]
enum FlyoutItem {
    Shape(ShapeKind),
    Tool(DrawTool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_matches_the_xaml_metrics() {
        assert_eq!(TOOLBAR_W, 484.0);
        assert_eq!(TOOLBAR_H, 36.0);
        assert_eq!(Tab::Select.x(), 5.0);
        assert_eq!(Tab::Copy.x(), PAD + 10.0 * SLOT + DIVIDER + 1.0);
    }
}
