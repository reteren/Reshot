//! Source VGUI settings dialog. A draft is applied only on OK or Apply.

mod view;
pub(crate) mod vgui;

use crate::{app::ReshotApp, winutil};
use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, FocusHandle, Global, KeyDownEvent, Pixels, Point,
    ScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use reshot_core::hotkey::HotkeyDefinition;
use reshot_core::settings::AppSettings;
use reshot_platform::win::{self, PxRect};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SNAPSHOT,
};

const WIDTH: f32 = 660.0;
const HEIGHT: f32 = 580.0;
const PALETTE: [&str; 16] = [
    "#FF3B30", "#FF9500", "#FFCC00", "#34C759", "#00C7BE", "#30B0C7", "#007AFF", "#5856D6",
    "#AF52DE", "#FF2D55", "#A2845E", "#8E8E93", "#FFFFFF", "#C7C7CC", "#48484A", "#000000",
];

#[derive(Default)]
struct SettingsState {
    handle: Option<WindowHandle<SettingsWindow>>,
    capturing_hotkey: bool,
}
impl Global for SettingsState {}

pub fn is_capturing_hotkey(cx: &App) -> bool {
    cx.try_global::<SettingsState>()
        .is_some_and(|state| state.capturing_hotkey)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Overlay,
    Output,
    Video,
    Audio,
    Info,
}
impl Tab {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Overlay,
        Self::Output,
        Self::Video,
        Self::Audio,
        Self::Info,
    ];
    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name))
    }
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Overlay => "Overlay",
            Self::Output => "Output",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Info => "Info",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Hotkey,
    AudioHotkey,
    DimColor,
    Screenshots,
    Videos,
    Records,
    Filename,
    CornersColor,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Toggle {
    RadialGesture,
    Autostart,
    Elevated,
    Updates,
    HideOverlay,
    Corners,
    VideoSystem,
    VideoMic,
    AskTracks,
    AudioSystem,
    AudioMic,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    Dim,
    Quality,
    Corners,
    Bitrate,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Select {
    Format,
    Fps,
    Mic,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Popover {
    Palette(Field),
    Select(Select),
}

pub struct SettingsWindow {
    focus: FocusHandle,
    draft: AppSettings,
    saved: AppSettings,
    settings_path: String,
    tab: Tab,
    field: Option<Field>,
    caret: usize,
    select_all: bool,
    popover: Option<Popover>,
    slider: Option<Slider>,
    mics: Vec<(String, String)>,
    status: String,
    last_print_screen: bool,
    slider_bounds: [Rc<Cell<Bounds<Pixels>>>; 4],
    /// The Info tab's two independently scrolling columns.
    scroll: [ScrollHandle; 2],
    /// Whether each column overflowed at the last paint, which decides its scrollbar.
    scroll_overflow: [Rc<Cell<bool>>; 2],
    /// A scrollbar thumb being dragged: column, pointer y and offset at the press, and
    /// thumb pixels per content pixel.
    scroll_drag: Option<(usize, f32, f32, f32)>,
}

/// Opens the window on a tab named like its caption ("info"); an unknown name keeps the default.
pub fn open_tab(cx: &mut App, tab: &str) {
    open(cx);
    let Some(tab) = Tab::from_name(tab) else {
        return;
    };
    if let Some(handle) = cx.try_global::<SettingsState>().and_then(|s| s.handle) {
        let _ = handle.update(cx, |this, _, cx| {
            this.tab = tab;
            cx.notify();
        });
    }
}

pub fn open(cx: &mut App) {
    if !cx.has_global::<SettingsState>() {
        cx.set_global(SettingsState::default());
    }
    if let Some(handle) = cx.global::<SettingsState>().handle {
        if let Ok(Some(hwnd)) = handle.update(cx, |_, window, _| winutil::hwnd(window)) {
            let raw = hwnd.0 as isize;
            winutil::after_update(cx, move || {
                win::force_foreground(win::hwnd(raw), false);
            });
            return;
        }
        cx.global_mut::<SettingsState>().handle = None;
    }
    let primary = win::primary_monitor_bounds();
    let work = win::working_area(primary.left + 1, primary.top + 1);
    let scale = winutil::primary_scale(cx);
    let width = (WIDTH * scale).round() as i32;
    let height = (HEIGHT * scale).round() as i32;
    let rect = PxRect {
        left: work.left + (work.width() - width) / 2,
        top: work.top + (work.height() - height) / 2,
        right: work.left + (work.width() + width) / 2,
        bottom: work.top + (work.height() + height) / 2,
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(winutil::logical(rect, scale))),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        // Acrylic under the translucent panel, as the webview window had.
        window_background: WindowBackgroundAppearance::Blurred,
        ..Default::default()
    };
    let draft = cx.global::<ReshotApp>().settings.current().clone();
    let settings_path = cx
        .global::<ReshotApp>()
        .settings
        .path()
        .display()
        .to_string();
    let mics = reshot_recording::list_microphones();
    let handle = match cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.on_release(|_: &mut SettingsWindow, cx| {
                if cx.has_global::<SettingsState>() {
                    cx.global_mut::<SettingsState>().handle = None;
                    cx.global_mut::<SettingsState>().capturing_hotkey = false;
                }
            })
            .detach();
            SettingsWindow {
                focus,
                saved: draft.clone(),
                draft,
                settings_path,
                tab: Tab::General,
                field: None,
                caret: 0,
                select_all: false,
                popover: None,
                slider: None,
                mics,
                status: String::new(),
                last_print_screen: false,
                slider_bounds: Default::default(),
                scroll: Default::default(),
                scroll_overflow: Default::default(),
                scroll_drag: None,
            }
        })
    }) {
        Ok(handle) => handle,
        Err(error) => {
            log::error!("Settings window: {error}");
            return;
        }
    };
    cx.global_mut::<SettingsState>().handle = Some(handle);
    if let Ok(Some(hwnd)) = handle.update(cx, |_, window, _| winutil::hwnd(window)) {
        let raw = hwnd.0 as isize;
        winutil::after_update(cx, move || {
            win::round_corners(win::hwnd(raw));
            win::force_foreground(win::hwnd(raw), false);
        });
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let alive = cx
                .update(|cx| {
                    handle
                        .update(cx, |this, _, cx| this.poll_print_screen(cx))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if !alive {
                break;
            }
        }
    })
    .detach();
}

impl SettingsWindow {
    fn set_field(&mut self, field: Option<Field>, cx: &mut Context<Self>) {
        self.field = field;
        cx.global_mut::<SettingsState>().capturing_hotkey =
            matches!(field, Some(Field::Hotkey | Field::AudioHotkey));
    }
    fn dirty(&self) -> bool {
        self.draft != self.saved
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        crate::app::apply_settings(cx, self.draft.clone());
        // Re-read: the app fills empty folders with their defaults.
        self.draft = cx.global::<ReshotApp>().settings.current().clone();
        self.saved = self.draft.clone();
        self.status = "Saved.".into();
        cx.notify();
    }
    fn close(&mut self, window: &mut Window) {
        window.remove_window();
    }
    fn poll_print_screen(&mut self, cx: &mut Context<Self>) -> bool {
        let focused = is_capturing_hotkey(cx)
            && matches!(self.field, Some(Field::Hotkey | Field::AudioHotkey));
        let down = focused && win::is_key_down(VK_SNAPSHOT.0 as u32);
        if down && !self.last_print_screen {
            let mut parts = Vec::new();
            if win::is_key_down(VK_CONTROL.0 as u32) {
                parts.push("Ctrl");
            }
            if win::is_key_down(VK_MENU.0 as u32) {
                parts.push("Alt");
            }
            if win::is_key_down(VK_SHIFT.0 as u32) {
                parts.push("Shift");
            }
            if win::is_key_down(VK_LWIN.0 as u32) || win::is_key_down(VK_RWIN.0 as u32) {
                parts.push("Win");
            }
            parts.push("PrtScn");
            self.set_text(self.field.unwrap(), parts.join("+"));
            cx.notify();
        }
        self.last_print_screen = down;
        true
    }
    fn text(&self, field: Field) -> &str {
        match field {
            Field::Hotkey => &self.draft.hotkey,
            Field::AudioHotkey => &self.draft.audio_hotkey,
            Field::DimColor => &self.draft.dim.color,
            Field::Screenshots => &self.draft.paths.screenshots,
            Field::Videos => &self.draft.paths.videos,
            Field::Records => &self.draft.paths.records,
            Field::Filename => &self.draft.filename.template,
            Field::CornersColor => &self.draft.video.corners.color,
        }
    }
    fn set_text(&mut self, field: Field, value: String) {
        match field {
            Field::Hotkey => self.draft.hotkey = value,
            Field::AudioHotkey => self.draft.audio_hotkey = value,
            Field::DimColor => self.draft.dim.color = value,
            Field::Screenshots => self.draft.paths.screenshots = value,
            Field::Videos => self.draft.paths.videos = value,
            Field::Records => self.draft.paths.records = value,
            Field::Filename => self.draft.filename.template = value,
            Field::CornersColor => self.draft.video.corners.color = value,
        }
        self.status.clear();
    }
    fn toggle(&mut self, key: Toggle) {
        let d = &mut self.draft;
        match key {
            Toggle::RadialGesture => d.radial.click_to_choose = !d.radial.click_to_choose,
            Toggle::Autostart => {
                d.autostart = !d.autostart;
                if !d.autostart {
                    d.autostart_elevated = false;
                }
            }
            Toggle::Elevated if d.autostart => d.autostart_elevated = !d.autostart_elevated,
            Toggle::Elevated => {}
            Toggle::Updates => d.update.auto = !d.update.auto,
            Toggle::HideOverlay => d.overlay.hide_from_capture = !d.overlay.hide_from_capture,
            Toggle::Corners => d.video.corners.enabled = !d.video.corners.enabled,
            Toggle::VideoSystem => d.video.audio.system = !d.video.audio.system,
            Toggle::VideoMic => d.video.audio.mic = !d.video.audio.mic,
            Toggle::AskTracks => d.video.audio.ask_on_save = !d.video.audio.ask_on_save,
            Toggle::AudioSystem => d.audio.system = !d.audio.system,
            Toggle::AudioMic => d.audio.mic = !d.audio.mic,
        }
        self.status.clear();
    }
    fn checked(&self, key: Toggle) -> bool {
        let d = &self.draft;
        match key {
            Toggle::RadialGesture => !d.radial.click_to_choose,
            Toggle::Autostart => d.autostart,
            Toggle::Elevated => d.autostart_elevated,
            Toggle::Updates => d.update.auto,
            Toggle::HideOverlay => d.overlay.hide_from_capture,
            Toggle::Corners => d.video.corners.enabled,
            Toggle::VideoSystem => d.video.audio.system,
            Toggle::VideoMic => d.video.audio.mic,
            Toggle::AskTracks => d.video.audio.ask_on_save,
            Toggle::AudioSystem => d.audio.system,
            Toggle::AudioMic => d.audio.mic,
        }
    }
    fn slider_value(&self, key: Slider) -> i32 {
        match key {
            Slider::Dim => (self.draft.dim.opacity * 100.0).round() as i32,
            Slider::Quality => self.draft.format.quality,
            Slider::Corners => (self.draft.video.corners.opacity * 100.0).round() as i32,
            Slider::Bitrate => self.draft.video.min_bitrate_kbps,
        }
    }
    fn slider_range(key: Slider) -> (i32, i32) {
        match key {
            Slider::Quality => (1, 100),
            Slider::Bitrate => (2_000, 20_000),
            _ => (0, 100),
        }
    }
    /// What one wheel notch moves a slider by; dragging snaps to it too.
    fn slider_step(key: Slider) -> i32 {
        if key == Slider::Bitrate { 500 } else { 1 }
    }
    fn set_slider(&mut self, key: Slider, value: i32) {
        let (min, max) = Self::slider_range(key);
        let step = Self::slider_step(key);
        let value = (((value - min) as f32 / step as f32).round() as i32 * step + min).clamp(min, max);
        match key {
            Slider::Dim => self.draft.dim.opacity = value as f64 / 100.0,
            Slider::Quality => self.draft.format.quality = value,
            Slider::Corners => self.draft.video.corners.opacity = value as f64 / 100.0,
            Slider::Bitrate => self.draft.video.min_bitrate_kbps = value,
        }
        self.status.clear();
    }
    fn slider_from_point(&mut self, key: Slider, point: Point<Pixels>) {
        // The thumb's centre follows the pointer; it travels the track's inside (1 px border
        // each side) less its own 10 px.
        let bounds = self.slider_bounds[key as usize].get();
        let travel = (f32::from(bounds.size.width) - 12.0).max(1.0);
        let x = f32::from(point.x) - f32::from(bounds.origin.x) - 6.0;
        let fraction = (x / travel).clamp(0.0, 1.0);
        let (min, max) = Self::slider_range(key);
        self.set_slider(key, min + (fraction * (max - min) as f32).round() as i32);
    }
    fn options(&self, key: Select) -> Vec<(String, String)> {
        match key {
            Select::Format => [("png", "PNG"), ("jpg", "JPG"), ("webp", "WebP")]
                .into_iter()
                .map(|(a, b)| (a.into(), b.into()))
                .collect(),
            Select::Fps => [("60", "60 FPS"), ("30", "30 FPS"), ("25", "25 FPS")]
                .into_iter()
                .map(|(a, b)| (a.into(), b.into()))
                .collect(),
            Select::Mic => std::iter::once(("default".into(), "System default".into()))
                .chain(self.mics.iter().cloned())
                .collect(),
        }
    }
    fn selected(&self, key: Select) -> String {
        match key {
            Select::Format => self.draft.format.image.clone(),
            Select::Fps => self.draft.video.fps.to_string(),
            Select::Mic => self.draft.audio.mic_device.clone(),
        }
    }
    fn set_selected(&mut self, key: Select, value: String) {
        match key {
            Select::Format => self.draft.format.image = value,
            Select::Fps => self.draft.video.fps = value.parse().unwrap_or(60),
            Select::Mic => {
                self.draft.audio.mic_device = value.clone();
                self.draft.video.audio.mic_device = value;
            }
        }
        self.status.clear();
        self.popover = None;
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if let Some(field @ (Field::Hotkey | Field::AudioHotkey)) = self.field {
            if let Some(token) = hotkey_token(event) {
                self.set_text(field, token);
                self.set_field(None, cx);
                cx.notify();
            }
            return;
        }
        if key == "escape" {
            if self.popover.take().is_none() && self.field.take().is_none() {
                self.close(window);
            }
            cx.global_mut::<SettingsState>().capturing_hotkey = false;
            cx.notify();
            return;
        }
        let Some(field) = self.field else {
            return;
        };
        if key == "enter" || key == "tab" {
            self.set_field(None, cx);
            cx.notify();
            return;
        }
        let text = self.text(field).to_string();
        let mut next = text.clone();
        let mut caret = self.caret.min(next.len());
        if event.keystroke.modifiers.control {
            match key {
                "a" => {
                    self.select_all = true;
                    cx.notify();
                }
                "c" => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
                "v" => {
                    if let Some(paste) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let paste = paste.replace(['\r', '\n'], "");
                        if self.select_all {
                            next.clear();
                            caret = 0;
                        }
                        next.insert_str(caret, &paste);
                        caret += paste.len();
                        self.select_all = false;
                        self.set_text(field, next);
                        self.caret = caret;
                        cx.notify();
                    }
                }
                _ => {}
            }
            return;
        }
        match key {
            "left" => {
                caret = next[..caret].char_indices().last().map_or(0, |(i, _)| i);
                self.select_all = false;
            }
            "right" => {
                caret += next[caret..].chars().next().map_or(0, char::len_utf8);
                self.select_all = false;
            }
            "home" => {
                caret = 0;
                self.select_all = false;
            }
            "end" => {
                caret = next.len();
                self.select_all = false;
            }
            "backspace" => {
                if self.select_all {
                    next.clear();
                    caret = 0;
                } else if caret > 0 {
                    let prev = next[..caret].char_indices().last().map_or(0, |(i, _)| i);
                    next.drain(prev..caret);
                    caret = prev;
                }
                self.select_all = false;
            }
            "delete" => {
                if self.select_all {
                    next.clear();
                    caret = 0;
                } else if caret < next.len() {
                    let end = caret + next[caret..].chars().next().map_or(0, char::len_utf8);
                    next.drain(caret..end);
                }
                self.select_all = false;
            }
            _ => {
                let Some(ch) = event.keystroke.key_char.as_deref() else {
                    return;
                };
                if ch.chars().any(char::is_control) {
                    return;
                }
                if self.select_all {
                    next.clear();
                    caret = 0;
                }
                next.insert_str(caret, ch);
                caret += ch.len();
                self.select_all = false;
            }
        }
        if matches!(field, Field::DimColor | Field::CornersColor) && next.len() > 7 {
            return;
        }
        self.set_text(field, next);
        self.caret = caret;
        cx.notify();
    }
    fn browse(&mut self, field: Field, cx: &mut Context<Self>) {
        let current = self.text(field).to_string();
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Choose folder")
            .set_directory(current);
        cx.spawn(async move |view, cx| {
            if let Some(folder) = dialog.pick_folder().await {
                let value = folder.path().display().to_string();
                let _ = view.update(cx, |this, cx| {
                    this.set_text(field, value);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

fn hotkey_token(event: &KeyDownEvent) -> Option<String> {
    let k = &event.keystroke;
    let key = match k.key.to_ascii_lowercase().as_str() {
        "printscreen" | "print_screen" | "print screen" => "PrtScn".to_string(),
        "space" => "Space".to_string(),
        "return" | "enter" => "Enter".to_string(),
        "backspace" => "Backspace".to_string(),
        "tab" => "Tab".to_string(),
        "escape" => "Esc".to_string(),
        "shift" | "control" | "ctrl" | "alt" | "meta" | "win" => return None,
        raw if raw.len() == 1 && raw.chars().all(|c| c.is_ascii_alphanumeric()) => {
            raw.to_ascii_uppercase()
        }
        raw if raw.starts_with('f')
            && raw[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) =>
        {
            raw.to_ascii_uppercase()
        }
        raw => match raw {
            "insert" => "Insert",
            "delete" => "Delete",
            "home" => "Home",
            "end" => "End",
            "pageup" => "PageUp",
            "pagedown" => "PageDown",
            "up" => "Up",
            "down" => "Down",
            "left" => "Left",
            "right" => "Right",
            "pause" => "Pause",
            "scrolllock" => "ScrollLock",
            _ => return None,
        }
        .to_string(),
    };
    let mut parts = Vec::new();
    if k.modifiers.control {
        parts.push("Ctrl");
    }
    if k.modifiers.alt {
        parts.push("Alt");
    }
    if k.modifiers.shift {
        parts.push("Shift");
    }
    if k.modifiers.platform {
        parts.push("Win");
    }
    parts.push(&key);
    let token = parts.join("+");
    HotkeyDefinition::parse(&token).map(|_| token).ok()
}
