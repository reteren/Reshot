//! The dialog's layout, rule for rule the webview's `index.html` + `styles.css`: the same
//! boxes, paddings and line boxes, so the window measures as it did. The one thing gpui
//! cannot do is `text-align: justify`; paragraphs are left-aligned.

use super::vgui::{self, LINE, PARA, button, checkbox, hint, kbd, para, raised, sunken, text};
use super::{Field, PALETTE, Popover, Select, SettingsWindow, Slider, Tab, Toggle};
use crate::theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, canvas, deferred, div, px,
    relative, rgb, rgba, svg,
};

/// `.column`: a flex column, children 16 px apart, sharing the panel width equally.
fn column() -> Div {
    div().flex_1().min_w_0().flex().flex_col().gap(px(16.))
}

/// `.inputGroup`: a label that takes the spare width, then its control.
fn input_group(label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(para(label).flex_grow())
}

fn separator() -> Div {
    theme::separator()
}

fn colour(s: &str) -> Option<gpui::Rgba> {
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    u32::from_str_radix(hex, 16).ok().map(rgb)
}

/// Clicks inside a popover or on what opens it must not reach the dialog, which closes
/// popovers on any other click.
fn keep_popover<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl SettingsWindow {
    fn check(&self, key: Toggle, label: &'static str, enabled: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        checkbox(format!("check-{}", key as usize), self.checked(key), label, enabled).on_click(
            cx.listener(move |this, _, _, cx| {
                if enabled {
                    this.toggle(key);
                    cx.notify();
                }
            }),
        )
    }

    /// `.textInput` / `.numberInput`: a 24 px sunken field. The caret is drawn between two
    /// runs of text so it takes no width, as a real caret does.
    fn field(&self, key: Field, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.field == Some(key);
        let value = self.text(key).to_string();
        let hotkey = matches!(key, Field::Hotkey | Field::AudioHotkey);
        let content: AnyElement = if value.is_empty() && key == Field::AudioHotkey && !active {
            text(12., LINE).text_color(vgui::placeholder()).child("Not set").into_any_element()
        } else if active && (self.select_all || hotkey) && !value.is_empty() {
            // A hotkey field being listened to shows its binding selected: the next
            // combination replaces it.
            text(12., LINE).bg(theme::accent()).child(value).into_any_element()
        } else if active {
            let caret = self.caret.min(value.len());
            let caret = if value.is_char_boundary(caret) { caret } else { value.len() };
            div()
                .flex()
                .child(text(12., LINE).child(value[..caret].to_string()))
                .child(
                    div().w(px(0.)).h(px(LINE)).relative().child(
                        div().absolute().left_0().top(px(0.5)).w(px(1.)).h(px(14.)).bg(theme::text()),
                    ),
                )
                .child(text(12., LINE).child(value[caret..].to_string()))
                .into_any_element()
        } else {
            text(12., LINE).child(value).into_any_element()
        };
        let id: SharedString = format!("field-{}", key as usize).into();
        let el = div()
            .h(px(24.))
            .min_w_0()
            .px(px(5.))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .bg(theme::input_bg())
            .child(content);
        sunken(el, &id, theme::input_dark(), theme::input_light())
            .id(gpui::ElementId::Name(id))
            .cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_field(Some(key), cx);
                this.popover = None;
                this.select_all = false;
                this.caret = this.text(key).len();
                window.focus(&this.focus);
                cx.notify();
            }))
    }

    /// `.slider`: an inset 6 px track with a raised 10×18 thumb, the native range input's
    /// geometry (the thumb travels the track's inside less its own width).
    fn slider_row(&self, key: Slider, name: &'static str, cx: &mut Context<Self>) -> Div {
        let v = self.slider_value(key);
        let (min, max) = Self::slider_range(key);
        let f = (v - min) as f32 / (max - min) as f32;
        let display = if key == Slider::Quality { v.to_string() } else { format!("{v}%") };
        let thumb_id: SharedString = format!("thumb-{}", key as usize).into();
        let thumb = div().absolute().top(px(-3.)).left(relative(f)).ml(px(1. - 12. * f)).w(px(10.)).h(px(18.)).bg(theme::btn_bg());
        let thumb = if self.slider == Some(key) { vgui::pressed(thumb, &thumb_id) } else { raised(thumb, &thumb_id, false) };
        let track = div()
            .id(("slider", key as usize))
            .flex_grow()
            .flex_basis(px(129.))
            .min_w_0()
            .h(px(12.))
            .relative()
            .cursor_pointer()
            .child(
                canvas(
                    {
                        let cell = self.slider_bounds[key as usize].clone();
                        move |bounds, _, _| cell.set(bounds)
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(sunken(
                div().absolute().left_0().right_0().top(px(3.)).h(px(6.)).bg(theme::input_bg()),
                &format!("track-{}", key as usize).into(),
                theme::input_dark(),
                theme::input_light(),
            ))
            .child(thumb)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    this.slider = Some(key);
                    this.slider_from_point(key, e.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, window, cx| {
                let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                if dy != 0.0 {
                    this.set_slider(key, this.slider_value(key) + dy.signum() as i32);
                    cx.notify();
                }
            }));
        input_group(name).child(track).child(
            text(12., PARA).w(px(34.)).flex_none().text_right().child(display),
        )
    }

    /// Hex field, the colour well, and the 16-swatch palette the well opens.
    fn color_row(&self, key: Field, cx: &mut Context<Self>) -> Div {
        let value = self.text(key).to_string();
        let open = self.popover == Some(Popover::Palette(key));
        let well_id: SharedString = format!("well-{}", key as usize).into();
        let well = sunken(
            div().size(px(24.)).flex_none().when_some(colour(&value), |d, c| d.bg(c)),
            &well_id,
            theme::input_dark(),
            theme::input_light(),
        )
        .id(gpui::ElementId::Name(well_id))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.popover = if this.popover == Some(Popover::Palette(key)) { None } else { Some(Popover::Palette(key)) };
            this.set_field(None, cx);
            cx.notify();
        }));
        let mut row = input_group("Colour")
            .relative()
            .child(self.field(key, cx).w(px(66.)).flex_none())
            .child(keep_popover(well));
        if open {
            let mut grid = div().w(px(175.)).p(px(5.)).flex().flex_wrap().gap(px(3.)).bg(theme::input_bg());
            for (i, hex) in PALETTE.iter().copied().enumerate() {
                let id: SharedString = format!("swatch-{}-{i}", key as usize).into();
                let selected = value.eq_ignore_ascii_case(hex);
                let ring = div()
                    .absolute()
                    .top(px(-2.))
                    .left(px(-2.))
                    .right(px(-2.))
                    .bottom(px(-2.))
                    .border_1()
                    .border_color(if selected { theme::text() } else { rgba(0) })
                    .when(!selected, |d| d.group_hover(id.clone(), |s| s.border_color(theme::accent())));
                let swatch = sunken(
                    div().w(px(18.)).h(px(16.)).bg(colour(hex).unwrap_or(rgb(0))).child(ring),
                    &id,
                    theme::input_dark(),
                    theme::input_light(),
                )
                .id(gpui::ElementId::Name(id.clone()))
                .group(id)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_text(key, hex.into());
                    this.popover = None;
                    cx.notify();
                }));
                grid = grid.child(swatch);
            }
            let palette = sunken(grid, &format!("palette-{}", key as usize).into(), theme::input_dark(), theme::input_light())
                .absolute()
                .top(px(28.))
                .right_0()
                .id(("palette", key as usize));
            row = row.child(deferred(keep_popover(palette)).with_priority(1));
        }
        row
    }

    /// `.select`: a raised value box with a solid triangle, opening an inset list whose
    /// current and hovered rows take the accent.
    fn select(&self, key: Select, width: Option<f32>, cx: &mut Context<Self>) -> Div {
        let selected = self.selected(key);
        let options = self.options(key);
        let current = options
            .iter()
            .find(|(v, _)| *v == selected)
            .or(options.first())
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let id: SharedString = format!("select-{}", key as usize).into();
        let value = raised(
            div()
                .size_full()
                .pl(px(5.))
                .pr(px(21.))
                .flex()
                .items_center()
                .bg(theme::btn_bg())
                .child(text(12., LINE).whitespace_nowrap().overflow_hidden().text_ellipsis().child(current))
                .child(svg().path("icons/select-arrow.svg").absolute().right(px(6.)).top(px(10.)).w(px(8.)).h(px(5.)).text_color(theme::text())),
            &id,
            false,
        )
        .id(gpui::ElementId::Name(id))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.popover = if this.popover == Some(Popover::Select(key)) { None } else { Some(Popover::Select(key)) };
            this.set_field(None, cx);
            cx.notify();
        }));
        let mut root = div().relative().h(px(24.)).min_w_0().child(keep_popover(value));
        root = match width {
            Some(w) => root.w(px(w)).flex_none(),
            None => root.w_full(),
        };
        if self.popover == Some(Popover::Select(key)) {
            let mut list = div().id(("select-list", key as usize)).max_h(px(178.)).overflow_y_scroll();
            for (i, (value, name)) in options.into_iter().enumerate() {
                let is_selected = value == selected;
                list = list.child(
                    text(12., LINE)
                        .id(("select-option", key as usize * 256 + i))
                        .px(px(4.))
                        .py(px(3.))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(theme::accent()))
                        .hover(|d| d.bg(theme::accent()))
                        .child(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_selected(key, value.clone());
                            cx.notify();
                        })),
                );
            }
            let popup = sunken(
                div().absolute().top(px(24.)).left_0().right_0().p(px(1.)).bg(theme::input_bg()).child(list),
                &format!("select-popup-{}", key as usize).into(),
                theme::input_dark(),
                theme::input_light(),
            )
            .id(("select-popup", key as usize));
            root = root.child(deferred(keep_popover(popup)).with_priority(1));
        }
        root
    }

    fn path_rows(&self, name: &'static str, key: Field, cx: &mut Context<Self>) -> [Div; 2] {
        [
            input_group(name),
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(self.field(key, cx).flex_1())
                .child(
                    button(format!("browse-{}", key as usize), "…", true, false)
                        .on_click(cx.listener(move |this, _, _, cx| this.browse(key, cx))),
                ),
        ]
    }

    fn general(&self, cx: &mut Context<Self>) -> Div {
        let left = column()
            .child(para("Global hotkeys:"))
            .child(input_group("Capture").child(self.field(Field::Hotkey, cx).w(px(130.)).flex_none()))
            .child(input_group("Quick audio record").child(self.field(Field::AudioHotkey, cx).w(px(130.)).flex_none()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(hint("Click a field, then press a key combination. Print Screen (PrtScn / Prnt) is supported.").flex_1().min_w_0())
                    .child(button("clear-audio-hotkey", "Clear", true, false).on_click(cx.listener(|this, _, _, cx| {
                        this.set_text(Field::AudioHotkey, String::new());
                        cx.notify();
                    }))),
            )
            .child(separator())
            .child(para("Radial menu:"))
            .child(self.check(Toggle::RadialGesture, "Choose by releasing the key", true, cx))
            .child(hint("Holding the capture hotkey opens the quick menu. Normally the wheel stays up after you let go and waits for a left click; the centre, or anywhere outside the wheel, cancels. With this on the menu lives only while the key is held: point at a slice and release to run it, and a flick past the rim counts as pointing that way.").flex_grow());
        // The webview's hint here carried `.inputLabel` (flex-grow) inside a column, so it
        // absorbs the spare height and the update checkbox sits at the bottom. Kept as it was.
        let right = column()
            .child(para("System:"))
            .child(self.check(Toggle::Autostart, "Start with Windows", true, cx))
            .child(self.check(Toggle::Elevated, "…as administrator", self.draft.autostart, cx))
            .child(hint("Needed only to capture over applications that run as administrator. Windows asks for permission once, when this window closes; after that it starts elevated silently, through a scheduled task named “Reshot”.").flex_grow())
            .child(self.check(Toggle::Updates, "Check for updates automatically", true, cx))
            .child(separator())
            .child(hint(self.settings_path.clone()));
        panel().child(left).child(right)
    }

    fn overlay(&self, cx: &mut Context<Self>) -> Div {
        panel()
            .child(
                column()
                    .child(para("Dimming of the non-selected area:"))
                    .child(self.slider_row(Slider::Dim, "Opacity", cx))
                    .child(self.color_row(Field::DimColor, cx)),
            )
            .child(
                column()
                    .child(hint("The overlay dims everything outside the current selection. Opacity 0 disables dimming entirely; the colour accepts any #RRGGBB value."))
                    .child(separator())
                    .child(para("Screen sharing:"))
                    .child(self.check(Toggle::HideOverlay, "Hide the overlay from other applications", true, cx))
                    .child(hint("Off, so a screen share, a call or someone else’s recorder shows the frozen frame, the selection and everything you draw on it. Turn it on and the overlay is visible on your display alone — everyone else keeps seeing the live desktop. Reshot’s own recordings never contain the overlay either way.")),
            )
    }

    fn output(&self, cx: &mut Context<Self>) -> Div {
        let mut left = column().child(para("Folders:"));
        for (name, key) in [("Screenshots", Field::Screenshots), ("Videos", Field::Videos), ("Audio records", Field::Records)] {
            left = left.children(self.path_rows(name, key, cx));
        }
        panel().child(left).child(
            column()
                .child(para("Image format:"))
                .child(input_group("Format").child(self.select(Select::Format, Some(90.), cx)))
                .child(self.slider_row(Slider::Quality, "Quality", cx))
                .child(hint("Quality applies to JPG and WebP only; PNG is always lossless."))
                .child(separator())
                .child(input_group("Filename"))
                .child(self.field(Field::Filename, cx))
                .child(hint("Placeholders: {date} and {time}.")),
        )
    }

    fn video(&self, cx: &mut Context<Self>) -> Div {
        panel()
            .child(
                column()
                    .child(para("Recording:"))
                    .child(input_group("Frame rate").child(self.select(Select::Fps, Some(90.), cx)))
                    .child(separator())
                    .child(para("Recording indicator:"))
                    .child(self.check(Toggle::Corners, "Show corner brackets", true, cx))
                    .child(self.color_row(Field::CornersColor, cx))
                    .child(self.slider_row(Slider::Corners, "Opacity", cx)),
            )
            .child(
                column()
                    .child(para("Audio recorded with video:"))
                    .child(self.check(Toggle::VideoSystem, "System audio", true, cx))
                    .child(self.check(Toggle::VideoMic, "Microphone", true, cx))
                    .child(self.check(Toggle::AskTracks, "Ask which tracks to keep when saving", true, cx))
                    .child(hint("Video is encoded as H.264 / MP4, which has no alpha channel, anything outside a non-rectangular selection is filled black.")),
            )
    }

    fn audio(&self, cx: &mut Context<Self>) -> Div {
        panel()
            .child(
                column()
                    .child(para("Standalone audio recorder:"))
                    .child(self.check(Toggle::AudioSystem, "System audio", true, cx))
                    .child(self.check(Toggle::AudioMic, "Microphone", true, cx)),
            )
            .child(
                column()
                    .child(para("Microphone device:"))
                    .child(self.select(Select::Mic, None, cx))
                    .child(hint("Shared with the video recorder. Individual windows can still be picked at capture time by right-clicking the Audio button in the overlay.")),
            )
    }

    /// A column that scrolls on its own, with the Source scrollbar: an inset trough and a
    /// raised thumb, 10 px wide, present only while the content overflows.
    fn scroll_column(&self, i: usize, content: Div, cx: &mut Context<Self>) -> Div {
        let handle = self.scroll[i].clone();
        let flag = self.scroll_overflow[i].clone();
        let overflow = flag.get();
        let mut wrap = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .child(
                div()
                    .id(("info-scroll", i))
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .pr(px(if overflow { 16. } else { 6. }))
                    .child(content),
            )
            .child(
                canvas(
                    {
                        let handle = handle.clone();
                        move |_, window, cx| {
                            let over = f32::from(handle.max_offset().height) > 0.5;
                            if over != flag.get() {
                                flag.set(over);
                                // After the frame: a refresh asked for mid-draw can be lost.
                                window.defer(cx, |window, _| window.refresh());
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            );
        if overflow {
            let view = f32::from(handle.bounds().size.height).max(1.);
            let max = f32::from(handle.max_offset().height).max(1.);
            let offset = (-f32::from(handle.offset().y)).clamp(0., max);
            let thumb_h = (view * view / (view + max)).max(18.).min(view);
            let thumb_y = offset / max * (view - thumb_h);
            let thumb_id: SharedString = format!("info-thumb-{i}").into();
            let thumb = raised(
                div().absolute().right_0().top(px(thumb_y)).w(px(10.)).h(px(thumb_h)).bg(theme::btn_bg()),
                &thumb_id,
                false,
            )
            .id(gpui::ElementId::Name(thumb_id))
            .hover(|s| s.bg(theme::accent()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    this.scroll_drag = Some((i, f32::from(e.position.y), offset, (view - thumb_h).max(1.) / max));
                    cx.stop_propagation();
                }),
            );
            let track = div()
                .id(("info-track", i))
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(10.))
                .bg(theme::input_bg())
                .border_t_1()
                .border_l_1()
                .border_color(theme::input_dark())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        // A click in the trough pages towards it, as a native scrollbar does.
                        let top = f32::from(this.scroll[i].bounds().origin.y) + thumb_y;
                        let page = if f32::from(e.position.y) < top { -view } else { view };
                        let next = (offset + page).clamp(0., max);
                        this.scroll[i].set_offset(gpui::point(px(0.), px(-next)));
                        cx.notify();
                    }),
                );
            wrap = wrap.child(track).child(thumb);
        }
        wrap
    }

    fn info(&self, cx: &mut Context<Self>) -> Div {
        let hotkey = self.draft.hotkey.clone();
        let audio = if self.draft.audio_hotkey.is_empty() { "not set".to_string() } else { self.draft.audio_hotkey.clone() };
        let mut n = 0usize;
        let mut bind = |keys: &[&str], what: &str| -> Div {
            let mut row = div().flex().items_start().gap(px(8.));
            for key in keys {
                n += 1;
                row = row.child(kbd(format!("kbd-{n}"), key.to_string()));
            }
            row.child(vgui::wrapped(what.to_string(), 12., PARA).mt(px(1.)).opacity(0.85).flex_1())
        };
        fn group(name: &'static str, first: bool) -> Div {
            text(12., PARA)
                .when(!first, |d| d.mt(px(10.)))
                .pb(px(4.))
                .border_b_1()
                .border_color(theme::tab_dark())
                .text_color(theme::accent())
                .child(name)
        }

        let mut binds = div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(group("Global (anywhere in Windows)", true))
            .child(bind(&[&hotkey], "Tap: freeze the screen and start a capture"))
            .child(bind(&[&hotkey], "Hold: open the radial quick menu"))
            .child(bind(&[&hotkey], "While recording: stop and save"))
            .child(bind(&[&audio], "Optional second hotkey: start / stop the audio recorder"))
            .child(group("Radial menu (hold the main hotkey)", false));
        // The two radial modes have different bindings; the list follows the checkbox.
        if self.draft.radial.click_to_choose {
            binds = binds
                .child(bind(&["LMB"], "The wheel stays up after you let go: click a slice for quick screen recording, quick audio, or settings"))
                .child(bind(&["LMB"], "Click the centre, or anywhere outside the wheel: cancel"));
        } else {
            binds = binds
                .child(bind(&[&hotkey], "Point at a slice and release: quick screen recording, quick audio, or settings"))
                .child(bind(&[&hotkey], "Release with the cursor still in the middle: cancel"));
        }
        binds = binds.child(bind(&["Esc", "RMB"], "Cancel, either way round"));
        for (heading, rows) in BINDS {
            binds = binds.child(group(heading, false));
            for (keys, what) in *rows {
                binds = binds.child(bind(keys, what));
            }
            if *heading == "Tools, in toolbar order" {
                binds = binds.child(hint("Number-pad digits work too. A category resumes whichever sub-tool you used last."));
            }
        }

        let about = column().child(para("About:")).child(
            div().flex().flex_col().gap(px(8.))
                .child(para("Reshot is a resident screenshot and screen-recording tool for Windows. It sits in the tray on a global hotkey and does no work at all until you call it."))
                .child(para("Pressing the hotkey freezes every monitor at once. Select an area of any shape: rectangle, ellipse, triangle, lasso or polygon. Annotate it with brushes, shapes, text, blur and pixelation, then copy it, save it, or record it as video. Anything outside a non-rectangular selection is exported as transparency."))
                .child(para("Video is written as H.264 / MP4. System audio and the microphone are captured separately, so you can pick which tracks to keep after the recording ends. Right-click the record tool to record only certain windows' sound. There is also a standalone audio recorder that can capture individual windows.")),
        );
        let keys = column().child(para("Keys:")).child(binds);
        panel()
            .overflow_hidden()
            .child(self.scroll_column(0, about, cx))
            .child(self.scroll_column(1, keys, cx))
    }
}

/// `.panel`: the two columns of a tab, 16 px apart, filling the content box.
fn panel() -> Div {
    div().size_full().min_h_0().flex().gap(px(16.))
}

/// The Info reference past the two groups that follow the settings, in toolbar order.
/// A shortcut line: its key chips and what they do.
type Bind = (&'static [&'static str], &'static str);

const BINDS: &[(&str, &[Bind])] = &[
    (
        "Tools, in toolbar order",
        &[
            (&["1"], "Selection"),
            (&["2"], "Brush"),
            (&["3"], "Shapes (square, circle, triangle)"),
            (&["4"], "Lines (line, arrow)"),
            (&["5"], "Erasers (normal, absolute, filter)"),
            (&["6"], "Effects (blur, pixelate)"),
            (&["7"], "Copy text from the image (OCR)"),
            (&["8"], "Text"),
            (&["9"], "Record video"),
            (&["0"], "Record audio"),
        ],
    ),
    (
        "Selecting an area",
        &[
            (&["LMB"], "Drag: new selection"),
            (&["Ctrl", "LMB"], "Drag: add another selection alongside the first"),
            (&["Shift"], "Held before dragging: lock to a square / circle"),
            (&["Shift"], "Pressed mid-drag: keep the ratio it has now"),
            (&["Ctrl", "A"], "Primary monitor; press again for every monitor"),
            (&["LMB"], "On a handle: resize · inside the frame: move"),
            (&["LMB"], "Polygon: place a point; click the first point to close"),
            (&["RMB"], "Polygon: close the outline by the shortest line"),
            (&["Esc"], "Polygon: abandon the outline"),
        ],
    ),
    (
        "Drawing",
        &[
            (&["LMB"], "Draw with the current tool (inside the selection)"),
            (&["Shift"], "Open / close the tool settings panel"),
            (&["LMB"], "On the dotted strip: the same panel"),
            (&["RMB"], "Hold on the image: colour picker, release to take the colour"),
            (&["LMB"], "Eyedropper button: stays armed until you right-click a pixel"),
            (&["Ctrl", "LMB"], "Effects: drag a rectangle instead of brushing"),
        ],
    ),
    (
        "Toolbar (mouse)",
        &[
            (&["LMB"], "On a tab: activate it"),
            (&["RMB"], "On a tab: sub-tool flyout (hovering 0.5 s opens it too)"),
            (&["RMB"], "On record video / record audio: pick the sound sources"),
            (&["RMB"], "On the OCR tab: switch AUTO / RU / EN"),
        ],
    ),
    (
        "Text tool",
        &[
            (&["LMB"], "Place the caret and start typing"),
            (&["Enter"], "New line"),
            (&["Backspace"], "Delete the last character"),
            (&["Esc"], "Finish the text and keep it (clicking away does the same)"),
        ],
    ),
    (
        "Text recognition (OCR)",
        &[
            (&["LMB"], "Drag across words: selects them and copies on release"),
            (&["Ctrl", "C"], "Copy the selected words"),
            (&["Ctrl", "A"], "Select and copy all recognised text"),
            (&["Esc"], "Leave OCR mode"),
        ],
    ),
    (
        "History",
        &[
            (&["Ctrl", "Z"], "Undo, 32 steps deep"),
            (&["Ctrl", "Shift", "Z"], "Redo"),
            (&["Ctrl", "Y"], "Redo, the same action as Ctrl+Shift+Z"),
        ],
    ),
    (
        "Finishing the session",
        &[
            (&["Enter"], "Copy to the clipboard and close"),
            (&["Ctrl", "C"], "Copy to the clipboard and close"),
            (&["✕"], "Toolbar close button: cancel the session (same as Esc when nothing else is open)"),
            (&["Esc"], "Backs out one layer at a time: armed eyedropper, then the settings panel, then an unfinished polygon, then the session itself"),
        ],
    ),
];

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `.tab`: grows from its own width (flex-basis auto), so the strip is uneven exactly
        // as the webview's was. The current tab is 2 px taller and open at the bottom.
        let mut tabs = div().flex().items_end().gap(px(1.));
        for tab in Tab::ALL {
            let active = self.tab == tab;
            let id: SharedString = format!("tab-{}", tab as usize).into();
            let label = text(12., LINE)
                .opacity(if active { 1.0 } else { 0.5 })
                .when(!active, |d| d.group_hover(id.clone(), |s| s.opacity(0.8)))
                .child(tab.label());
            let el = div().flex_grow().px(px(17.)).pt(px(if active { 7. } else { 5. })).pb(px(5.)).child(label);
            let (light, dark) = (theme::tab_light(), theme::tab_dark());
            let el = vgui::edges(el, &id, [Some(light), Some(dark), (!active).then_some(light), Some(light)], None);
            tabs = tabs.child(
                el.id(gpui::ElementId::Name(id.clone()))
                    .group(id)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        this.popover = None;
                        this.set_field(None, cx);
                        cx.notify();
                    })),
            );
        }
        let content: Div = match self.tab {
            Tab::General => self.general(cx),
            Tab::Overlay => self.overlay(cx),
            Tab::Output => self.output(cx),
            Tab::Video => self.video(cx),
            Tab::Audio => self.audio(cx),
            Tab::Info => self.info(cx),
        };
        // `.content`: no top edge of its own, pulled up 1 px under the tabs' bottom edges.
        let body = vgui::edges(
            div().flex_1().min_h(px(300.)).mt(px(-1.)).pl(px(17.)).pr(px(17.)).pt(px(16.)).pb(px(17.)).child(content),
            &"content".into(),
            [None, Some(theme::tab_dark()), Some(theme::tab_dark()), Some(theme::tab_light())],
            None,
        );

        let dirty = self.dirty();
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
            .line_height(px(LINE))
            .text_color(theme::text())
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.popover = None;
                    this.set_field(None, cx);
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.key_down(event, window, cx)))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some(key) = this.slider {
                    this.slider_from_point(key, event.position);
                    cx.notify();
                }
                if let Some((i, start_y, start_offset, ratio)) = this.scroll_drag {
                    let max = f32::from(this.scroll[i].max_offset().height);
                    let next = (start_offset + (f32::from(event.position.y) - start_y) / ratio).clamp(0., max);
                    this.scroll[i].set_offset(gpui::point(px(0.), px(-next)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.slider = None;
                    this.scroll_drag = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        // Chrome's half-leading sets the caps 1 px lower than gpui's. The
                        // reference's `letter-spacing: 0.04em` has no gpui equivalent; spacing
                        // glyph by glyph loses kerning and snaps each one to a pixel, so it is
                        // left out (0.4 px a letter).
                        div()
                            .flex_grow()
                            .mt(px(2.))
                            .text_size(px(10.))
                            .line_height(px(12.5))
                            .font_weight(FontWeight::BOLD)
                            .window_control_area(WindowControlArea::Drag)
                            .child("RESHOT · SETTINGS"),
                    )
                    .child(
                        div()
                            .id("settings-close")
                            .size(px(16.))
                            .flex_none()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0x00000026)))
                            .child(svg().path("icons/dialog-close.svg").size(px(16.)).text_color(rgb(0xC1C1C1)))
                            .on_click(cx.listener(|this, _, window, _| this.close(window))),
                    ),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(tabs).child(body))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_grow()
                            .min_h(px(14.))
                            .text_size(px(11.))
                            .line_height(px(13.75))
                            .opacity(0.75)
                            .child(self.status.clone()),
                    )
                    .child(button("settings-ok", "OK", false, false).on_click(cx.listener(|this, _, window, cx| {
                        this.save(cx);
                        this.close(window);
                    })))
                    .child(button("settings-cancel", "Cancel", false, false).on_click(cx.listener(|this, _, window, _| this.close(window))))
                    .child(button("settings-apply", "Apply", false, !dirty).on_click(cx.listener(|this, _, _, cx| {
                        if this.dirty() {
                            this.save(cx);
                        }
                    }))),
            )
    }
}
