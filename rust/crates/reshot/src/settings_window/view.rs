use super::{Field, PALETTE, Popover, Select, SettingsWindow, Slider, Tab, Toggle};
use crate::theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, Window,
    WindowControlArea, canvas, div, px, rgb,
};

fn label(text: impl Into<gpui::SharedString>) -> Div {
    theme::body().text_size(px(12.)).child(text.into())
}
fn hint(text: &'static str) -> Div {
    label(text).opacity(0.7).line_height(gpui::relative(1.5))
}
fn section(text: &'static str) -> Div {
    label(text)
}
fn column() -> Div {
    div().flex_1().min_w_0().flex().flex_col().gap(px(14.))
}
fn row() -> Div {
    div().flex().items_center().gap(px(8.))
}
fn inset(content: impl IntoElement) -> Div {
    div()
        .h(px(24.))
        .bg(theme::input_bg())
        .border_t_1()
        .border_l_1()
        .border_color(theme::input_dark())
        .child(
            div()
                .size_full()
                .border_r_1()
                .border_b_1()
                .border_color(theme::input_light())
                .px(px(4.))
                .flex()
                .items_center()
                .child(content),
        )
}
fn color_value(s: &str) -> gpui::Rgba {
    let v = s
        .strip_prefix('#')
        .and_then(|h| u32::from_str_radix(h, 16).ok());
    v.filter(|_| s.len() == 7).map(rgb).unwrap_or(rgb(0x5A5A5A))
}

impl SettingsWindow {
    fn checkbox(
        &self,
        key: Toggle,
        text: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        theme::check_row(("settings-check", key as usize), self.checked(key), text)
            .opacity(if enabled { 1.0 } else { 0.45 })
            .on_click(cx.listener(move |this, _, _, cx| {
                if enabled {
                    this.toggle(key);
                    cx.notify();
                }
            }))
            .into_any_element()
    }
    fn input(&self, key: Field, width: Option<f32>, cx: &mut Context<Self>) -> AnyElement {
        let active = self.field == Some(key);
        let value = self.text(key);
        let shown = if value.is_empty() && key == Field::AudioHotkey {
            "Not set".to_string()
        } else if active && matches!(key, Field::Hotkey | Field::AudioHotkey) {
            "Press a key…".into()
        } else if active && self.select_all {
            format!("{value} ▌")
        } else if active && self.caret <= value.len() && value.is_char_boundary(self.caret) {
            format!("{}│{}", &value[..self.caret], &value[self.caret..])
        } else {
            value.to_string()
        };
        let mut outer = div()
            .id(("settings-input", key as usize))
            .min_w_0()
            .cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_field(Some(key), cx);
                this.popover = None;
                this.select_all = false;
                this.caret = this.text(key).len();
                window.focus(&this.focus);
                cx.notify();
            }));
        outer = if let Some(width) = width {
            outer.w(px(width))
        } else {
            outer.flex_1()
        };
        outer
            .child(inset(
                label(shown)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .opacity(if value.is_empty() && key == Field::AudioHotkey {
                        0.55
                    } else {
                        1.0
                    }),
            ))
            .into_any_element()
    }
    fn row_input(
        &self,
        name: &'static str,
        key: Field,
        width: Option<f32>,
        cx: &mut Context<Self>,
    ) -> Div {
        row()
            .child(label(name).w(px(115.)))
            .child(self.input(key, width, cx))
    }
    fn path_row(&self, name: &'static str, key: Field, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(label(name))
            .child(
                row().child(self.input(key, None, cx)).child(
                    theme::button(("browse", key as usize), "…")
                        .w(px(26.))
                        .on_click(cx.listener(move |this, _, _, cx| this.browse(key, cx))),
                ),
            )
    }
    fn slider_row(&self, key: Slider, name: &'static str, cx: &mut Context<Self>) -> Div {
        let v = self.slider_value(key);
        let display = if key == Slider::Quality {
            v.to_string()
        } else {
            format!("{v}%")
        };
        let track = div()
            .id(("settings-slider", key as usize))
            .flex_1()
            .h(px(18.))
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
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .right(px(0.))
                    .top(px(6.))
                    .h(px(6.))
                    .bg(theme::input_bg())
                    .border_t_1()
                    .border_l_1()
                    .border_color(theme::input_dark())
                    .child(
                        div()
                            .size_full()
                            .border_b_1()
                            .border_r_1()
                            .border_color(theme::input_light()),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(gpui::relative(v as f32 / 100.0))
                    .ml(px(-5.))
                    .w(px(10.))
                    .h(px(18.))
                    .bg(theme::btn_bg())
                    .border_t_1()
                    .border_l_1()
                    .border_color(theme::bevel_light())
                    .child(
                        div()
                            .size_full()
                            .border_b_1()
                            .border_r_1()
                            .border_color(theme::bevel_dark()),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    this.slider = Some(key);
                    this.set_field(None, cx);
                    // The visual track is 100 px wide at the reference dialog's column size.
                    // Subsequent movement is handled from the window-level listener.
                    this.slider_from_point(key, e.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, e: &gpui::ScrollWheelEvent, window, cx| {
                    let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                    this.set_slider(key, this.slider_value(key) + dy.signum() as i32);
                    cx.notify();
                }),
            );
        row()
            .child(label(name).w(px(72.)))
            .child(track)
            .child(label(display).w(px(40.)).text_align(gpui::TextAlign::Right))
    }
    fn color_row(&self, key: Field, cx: &mut Context<Self>) -> Div {
        let color = self.text(key);
        let open = self.popover == Some(Popover::Palette(key));
        let mut swatches = div()
            .absolute()
            .top(px(28.))
            .right(px(0.))
            .w(px(178.))
            .p(px(4.))
            .bg(theme::input_bg())
            .border_t_1()
            .border_l_1()
            .border_color(theme::input_dark())
            .flex()
            .flex_wrap()
            .gap(px(3.))
            .when(!open, |d| d.hidden());
        for (i, hex) in PALETTE.iter().copied().enumerate() {
            swatches = swatches.child(
                div()
                    .id(("swatch", key as usize * 16 + i))
                    .w(px(18.))
                    .h(px(16.))
                    .bg(color_value(hex))
                    .border_1()
                    .border_color(if color.eq_ignore_ascii_case(hex) {
                        theme::text()
                    } else {
                        theme::input_dark()
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_text(key, hex.into());
                        this.popover = None;
                        cx.notify();
                    })),
            );
        }
        row()
            .relative()
            .child(label("Colour").w(px(72.)))
            .child(self.input(key, Some(70.), cx))
            .child(
                div()
                    .id(("well", key as usize))
                    .w(px(24.))
                    .h(px(24.))
                    .bg(color_value(color))
                    .border_t_1()
                    .border_l_1()
                    .border_color(theme::input_dark())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.popover = if this.popover == Some(Popover::Palette(key)) {
                            None
                        } else {
                            Some(Popover::Palette(key))
                        };
                        this.set_field(None, cx);
                        cx.notify();
                    })),
            )
            .child(swatches)
    }
    fn select_row(
        &self,
        key: Select,
        name: &'static str,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = self.selected(key);
        let options = self.options(key);
        let label_text = options
            .iter()
            .find(|(v, _)| *v == selected)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| selected.clone());
        let open = self.popover == Some(Popover::Select(key));
        let mut list = div()
            .id(("select-list", key as usize))
            .absolute()
            .top(px(24.))
            .left(px(0.))
            .w_full()
            .max_h(px(180.))
            .overflow_y_scroll()
            .bg(theme::input_bg())
            .border_1()
            .border_color(theme::input_dark())
            .when(!open, |d| d.hidden());
        for (i, (value, name)) in options.into_iter().enumerate() {
            let is_selected = selected == value;
            list = list.child(
                div()
                    .id(("select-option", key as usize * 256 + i))
                    .px(px(4.))
                    .py(px(3.))
                    .cursor_pointer()
                    .font_family(theme::FONT)
                    .text_size(px(12.))
                    .bg(if is_selected {
                        theme::accent()
                    } else {
                        theme::input_bg()
                    })
                    .hover(|d| d.bg(theme::accent()))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_selected(key, value.clone());
                        cx.notify();
                    })),
            );
        }
        row().child(label(name).w(px(72.))).child(
            div()
                .relative()
                .w(px(width))
                .child(
                    theme::button(
                        ("settings-select", key as usize),
                        row()
                            .w_full()
                            .justify_between()
                            .child(label(label_text))
                            .child("▾"),
                    )
                    .w_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.popover = if this.popover == Some(Popover::Select(key)) {
                            None
                        } else {
                            Some(Popover::Select(key))
                        };
                        this.set_field(None, cx);
                        cx.notify();
                    })),
                )
                .child(list),
        )
    }
    fn general(&self, cx: &mut Context<Self>) -> Div {
        row().items_start()
            .child(column()
                .child(section("Global hotkeys:"))
                .child(self.row_input("Capture", Field::Hotkey, Some(130.), cx))
                .child(self.row_input("Quick audio record", Field::AudioHotkey, Some(130.), cx))
                .child(row().child(hint("Click a field, then press a key combination. Print Screen (PrtScn / Prnt) is supported.").flex_1())
                    .child(theme::button("clear-audio-hotkey", "Clear").on_click(cx.listener(|this, _, _, cx| { this.set_text(Field::AudioHotkey, String::new()); cx.notify(); }))))
                .child(theme::separator()).child(section("Radial menu:"))
                .child(self.checkbox(Toggle::RadialGesture, "Choose by releasing the key", true, cx))
                .child(hint("Holding the capture hotkey opens the quick menu. Normally the wheel stays up after you let go and waits for a left click; the centre, or anywhere outside the wheel, cancels. With this on the menu lives only while the key is held: point at a slice and release to run it, and a flick past the rim counts as pointing that way.")))
            .child(column()
                .child(section("System:"))
                .child(self.checkbox(Toggle::Autostart, "Start with Windows", true, cx))
                .child(self.checkbox(Toggle::Elevated, "…as administrator", self.draft.autostart, cx))
                .child(hint("Needed only to capture over applications that run as administrator. Windows asks for permission once, when this window closes; after that it starts elevated silently, through a scheduled task named “Reshot”."))
                .child(self.checkbox(Toggle::Updates, "Check for updates automatically", true, cx))
                .child(theme::separator()).child(label(self.settings_path.clone()).opacity(0.7)))
    }
    fn overlay(&self, cx: &mut Context<Self>) -> Div {
        row().items_start()
            .child(column().child(section("Dimming of the non-selected area:"))
                .child(self.slider_row(Slider::Dim, "Opacity", cx))
                .child(self.color_row(Field::DimColor, cx)))
            .child(column().child(hint("The overlay dims everything outside the current selection. Opacity 0 disables dimming entirely; the colour accepts any #RRGGBB value."))
                .child(theme::separator()).child(section("Screen sharing:"))
                .child(self.checkbox(Toggle::HideOverlay, "Hide the overlay from other applications", true, cx))
                .child(hint("Off, so a screen share, a call or someone else’s recorder shows the frozen frame, the selection and everything you draw on it. Turn it on and the overlay is visible on your display alone — everyone else keeps seeing the live desktop. Reshot’s own recordings never contain the overlay either way.")))
    }
    fn output(&self, cx: &mut Context<Self>) -> Div {
        row()
            .items_start()
            .child(
                column()
                    .child(section("Folders:"))
                    .child(self.path_row("Screenshots", Field::Screenshots, cx))
                    .child(self.path_row("Videos", Field::Videos, cx))
                    .child(self.path_row("Audio records", Field::Records, cx)),
            )
            .child(
                column()
                    .child(section("Image format:"))
                    .child(self.select_row(Select::Format, "Format", 90., cx))
                    .child(self.slider_row(Slider::Quality, "Quality", cx))
                    .child(hint(
                        "Quality applies to JPG and WebP only; PNG is always lossless.",
                    ))
                    .child(theme::separator())
                    .child(label("Filename"))
                    .child(self.input(Field::Filename, None, cx))
                    .child(hint("Placeholders: {date} and {time}.")),
            )
    }
    fn video(&self, cx: &mut Context<Self>) -> Div {
        row().items_start().child(column().child(section("Recording:"))
            .child(self.select_row(Select::Fps, "Frame rate", 90., cx))
            .child(theme::separator()).child(section("Recording indicator:"))
            .child(self.checkbox(Toggle::Corners, "Show corner brackets", true, cx))
            .child(self.color_row(Field::CornersColor, cx))
            .child(self.slider_row(Slider::Corners, "Opacity", cx)))
            .child(column().child(section("Audio recorded with video:"))
                .child(self.checkbox(Toggle::VideoSystem, "System audio", true, cx))
                .child(self.checkbox(Toggle::VideoMic, "Microphone", true, cx))
                .child(self.checkbox(Toggle::AskTracks, "Ask which tracks to keep when saving", true, cx))
                .child(hint("Video is encoded as H.264 / MP4, which has no alpha channel, anything outside a non-rectangular selection is filled black.")))
    }
    fn audio(&self, cx: &mut Context<Self>) -> Div {
        row().items_start().child(column().child(section("Standalone audio recorder:"))
            .child(self.checkbox(Toggle::AudioSystem, "System audio", true, cx))
            .child(self.checkbox(Toggle::AudioMic, "Microphone", true, cx)))
            .child(column().child(section("Microphone device:"))
                .child(self.select_row(Select::Mic, "", 255., cx))
                .child(hint("Shared with the video recorder. Individual windows can still be picked at capture time by right-clicking the Audio button in the overlay.")))
    }
    fn info(&self, _cx: &mut Context<Self>) -> Div {
        fn group(name: &'static str) -> Div {
            label(name).font_weight(FontWeight::BOLD).mt(px(10.))
        }
        fn bind(keys: impl Into<gpui::SharedString>, action: &'static str) -> Div {
            row()
                .items_start()
                .child(
                    label(keys)
                        .w(px(92.))
                        .flex_none()
                        .font_weight(FontWeight::BOLD),
                )
                .child(label(action).flex_1())
        }
        let hotkey = self.draft.hotkey.clone();
        let audio = if self.draft.audio_hotkey.is_empty() {
            "not set".to_string()
        } else {
            self.draft.audio_hotkey.clone()
        };
        let mut binds = column()
            .id("settings-info-binds")
            .overflow_y_scroll()
            .gap(px(6.))
            .child(section("Keys:"))
            .child(group("Global (anywhere in Windows)"))
            .child(
                row()
                    .child(
                        label(hotkey.clone())
                            .w(px(92.))
                            .font_weight(FontWeight::BOLD),
                    )
                    .child(label("Tap: freeze the screen and start a capture")),
            )
            .child(
                row()
                    .child(
                        label(hotkey.clone())
                            .w(px(92.))
                            .font_weight(FontWeight::BOLD),
                    )
                    .child(label("Hold: open the radial quick menu")),
            )
            .child(
                row()
                    .child(
                        label(hotkey.clone())
                            .w(px(92.))
                            .font_weight(FontWeight::BOLD),
                    )
                    .child(label("While recording: stop and save")),
            )
            .child(
                row()
                    .child(label(audio).w(px(92.)).font_weight(FontWeight::BOLD))
                    .child(label(
                        "Optional second hotkey: start / stop the audio recorder",
                    )),
            )
            .child(group("Radial menu (hold the main hotkey)"));
        if self.draft.radial.click_to_choose {
            binds = binds.child(bind("LMB", "The wheel stays up after you let go: click a slice for quick screen recording, quick audio, or settings"))
                .child(bind("LMB", "Click the centre, or anywhere outside the wheel: cancel"));
        } else {
            binds = binds.child(bind(hotkey.clone(), "Point at a slice and release: quick screen recording, quick audio, or settings"))
                .child(bind(hotkey, "Release with the cursor still in the middle: cancel"));
        }
        binds = binds.child(bind("Esc  RMB", "Cancel, either way round"));
        const GROUPS: &[(&str, &[(&str, &str)])] = &[
            (
                "Tools, in toolbar order",
                &[
                    ("1", "Selection"),
                    ("2", "Brush"),
                    ("3", "Shapes (square, circle, triangle)"),
                    ("4", "Lines (line, arrow)"),
                    ("5", "Erasers (normal, absolute, filter)"),
                    ("6", "Effects (blur, pixelate)"),
                    ("7", "Copy text from the image (OCR)"),
                    ("8", "Text"),
                    ("9", "Record video"),
                    ("0", "Record audio"),
                ],
            ),
            (
                "Selecting an area",
                &[
                    ("LMB", "Drag: new selection"),
                    (
                        "Ctrl LMB",
                        "Drag: add another selection alongside the first",
                    ),
                    ("Shift", "Held before dragging: lock to a square / circle"),
                    ("Shift", "Pressed mid-drag: keep the ratio it has now"),
                    ("Ctrl A", "Primary monitor; press again for every monitor"),
                    ("LMB", "On a handle: resize · inside the frame: move"),
                    (
                        "LMB",
                        "Polygon: place a point; click the first point to close",
                    ),
                    ("RMB", "Polygon: close the outline by the shortest line"),
                    ("Esc", "Polygon: abandon the outline"),
                ],
            ),
            (
                "Drawing",
                &[
                    ("LMB", "Draw with the current tool (inside the selection)"),
                    ("Shift", "Open / close the tool settings panel"),
                    ("LMB", "On the dotted strip: the same panel"),
                    (
                        "RMB",
                        "Hold on the image: colour picker, release to take the colour",
                    ),
                    (
                        "LMB",
                        "Eyedropper button: stays armed until you right-click a pixel",
                    ),
                    ("Ctrl LMB", "Effects: drag a rectangle instead of brushing"),
                ],
            ),
            (
                "Toolbar (mouse)",
                &[
                    ("LMB", "On a tab: activate it"),
                    (
                        "RMB",
                        "On a tab: sub-tool flyout (hovering 0.5 s opens it too)",
                    ),
                    (
                        "RMB",
                        "On record video / record audio: pick the sound sources",
                    ),
                    ("RMB", "On the OCR tab: switch AUTO / RU / EN"),
                ],
            ),
            (
                "Text tool",
                &[
                    ("LMB", "Place the caret and start typing"),
                    ("Enter", "New line"),
                    ("Backspace", "Delete the last character"),
                    (
                        "Esc",
                        "Finish the text and keep it (clicking away does the same)",
                    ),
                ],
            ),
            (
                "Text recognition (OCR)",
                &[
                    (
                        "LMB",
                        "Drag across words: selects them and copies on release",
                    ),
                    ("Ctrl C", "Copy the selected words"),
                    ("Ctrl A", "Select and copy all recognised text"),
                    ("Esc", "Leave OCR mode"),
                ],
            ),
            (
                "History",
                &[
                    ("Ctrl Z", "Undo, 32 steps deep"),
                    ("Ctrl Shift Z", "Redo"),
                    ("Ctrl Y", "Redo, the same action as Ctrl+Shift+Z"),
                ],
            ),
            (
                "Finishing the session",
                &[
                    ("Enter", "Copy to the clipboard and close"),
                    ("Ctrl C", "Copy to the clipboard and close"),
                    (
                        "✕",
                        "Toolbar close button: cancel the session (same as Esc when nothing else is open)",
                    ),
                    (
                        "Esc",
                        "Backs out one layer at a time: armed eyedropper, then the settings panel, then an unfinished polygon, then the session itself",
                    ),
                ],
            ),
        ];
        for (heading, rows) in GROUPS {
            binds = binds.child(group(heading));
            for (keys, action) in *rows {
                binds = binds.child(bind(*keys, action));
            }
            if *heading == "Tools, in toolbar order" {
                binds = binds.child(hint("Number-pad digits work too. A category resumes whichever sub-tool you used last."));
            }
        }
        row().items_start().h_full()
            .child(column().id("settings-info-about").overflow_y_scroll().child(section("About:"))
                .child(label("Reshot is a resident screenshot and screen-recording tool for Windows. It sits in the tray on a global hotkey and does no work at all until you call it."))
                .child(label("Pressing the hotkey freezes every monitor at once. Select an area of any shape: rectangle, ellipse, triangle, lasso or polygon. Annotate it with brushes, shapes, text, blur and pixelation, then copy it, save it, or record it as video. Anything outside a non-rectangular selection is exported as transparency."))
                .child(label("Video is written as H.264 / MP4. System audio and the microphone are captured separately, so you can pick which tracks to keep after the recording ends. Right-click the record tool to record only certain windows' sound. There is also a standalone audio recorder that can capture individual windows.")))
            .child(binds)
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = row().gap(px(1.));
        for tab in Tab::ALL {
            tabs = tabs.child(
                div()
                    .id(("tab", tab as usize))
                    .flex_1()
                    .cursor_pointer()
                    .px(px(12.))
                    .py(px(if self.tab == tab { 6. } else { 4. }))
                    .border_t_1()
                    .border_l_1()
                    .border_color(theme::tab_light())
                    .border_r_1()
                    .border_color(theme::tab_dark())
                    .text_color(theme::text())
                    .opacity(if self.tab == tab { 1.0 } else { 0.6 })
                    .font_family(theme::FONT)
                    .font_weight(FontWeight::LIGHT)
                    .text_size(px(12.))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        this.popover = None;
                        this.set_field(None, cx);
                        cx.notify();
                    })),
            );
        }
        let content: AnyElement = match self.tab {
            Tab::General => self.general(cx).into_any_element(),
            Tab::Overlay => self.overlay(cx).into_any_element(),
            Tab::Output => self.output(cx).into_any_element(),
            Tab::Video => self.video(cx).into_any_element(),
            Tab::Audio => self.audio(cx).into_any_element(),
            Tab::Info => self.info(cx).into_any_element(),
        };
        div()
            .size_full()
            .rounded(px(8.))
            .bg(theme::dialog_bg())
            .p(px(16.))
            .font_family(theme::FONT)
            .text_color(theme::text())
            .font_weight(FontWeight::LIGHT)
            .text_size(px(12.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.set_field(None, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.key_down(event, window, cx)
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some(key) = this.slider {
                    this.slider_from_point(key, event.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.slider = None),
            )
            .child(
                row()
                    .justify_between()
                    .child(
                        theme::title("RESHOT · SETTINGS")
                            .flex_1()
                            .h(px(20.))
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        theme::button("settings-close", "✕")
                            .w(px(24.))
                            .on_click(cx.listener(|this, _, window, _| this.close(window))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(tabs)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p(px(16.))
                            .border_l_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(theme::tab_light())
                            .child(content),
                    ),
            )
            .child(
                row()
                    .justify_end()
                    .child(label(self.status.clone()).opacity(0.75).flex_1())
                    .child(
                        theme::button("settings-ok", "OK")
                            .w(px(70.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save(cx);
                                this.close(window);
                            })),
                    )
                    .child(
                        theme::button("settings-cancel", "Cancel")
                            .w(px(70.))
                            .on_click(cx.listener(|this, _, window, _| this.close(window))),
                    )
                    .child(
                        theme::button("settings-apply", "Apply")
                            .w(px(70.))
                            .opacity(if self.dirty() { 1.0 } else { 0.45 })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.dirty() {
                                    this.save(cx);
                                }
                            })),
                    ),
            )
    }
}
