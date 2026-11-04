//! Three-slice Source-style quick menu. Physical input is polled because the click that
//! cancels can land outside this small window, and a release flick can end beyond its rim.

use std::f32::consts::PI;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, Global, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, PathBuilder, Render,
    Bounds, Styled, TransformationMatrix, Window, WindowHandle, canvas, div, point, px, rgb, size,
};
use reshot_platform::win::{self, PxRect};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

use crate::{app, recording, winutil};

const OUTER: f32 = 132.0;
const INNER: f32 = 46.0;
const EXTEND: f32 = 14.0;
const RADIUS: f32 = OUTER + EXTEND + 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Record,
    Audio,
    Settings,
}

#[derive(Default)]
struct RadialState(Option<WindowHandle<RadialMenu>>);
impl Global for RadialState {}

pub fn open(cx: &mut App, vk: u32) {
    if recording::is_active(cx) {
        return;
    }
    if cx.has_global::<RadialState>() {
        if let Some(handle) = cx.global::<RadialState>().0
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
    } else {
        cx.set_global(RadialState::default());
    }

    let click_mode = cx
        .global::<app::ReshotApp>()
        .settings
        .current()
        .radial
        .click_to_choose;
    let (x, y) = win::cursor_pos();
    let work = win::working_area(x, y);
    let scale = scale_at(x, y).unwrap_or_else(|| winutil::primary_scale(cx));
    let r = (RADIUS * scale).round() as i32;
    let center = (
        clamp(x, work.left + r, work.right - r),
        clamp(y, work.top + r, work.bottom - r),
    );
    let bounds = PxRect {
        left: center.0 - r,
        top: center.1 - r,
        right: center.0 + r,
        bottom: center.1 + r,
    };
    let game = win::foreground_is_fullscreen_foreign_window();
    let handle = match cx.open_window(
        winutil::popup_options(winutil::logical(bounds, scale), true),
        |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus);
                RadialMenu {
                    focus,
                    center,
                    scale,
                    vk: if click_mode { 0 } else { vk },
                    left_down: win::is_key_down(1),
                    right_down: win::is_key_down(2),
                    hover: None,
                    hover_amount: [0.0; 3],
                    opened: Instant::now(),
                    last_tick: Instant::now(),
                    frames: 0,
                    closing: None,
                    chosen: None,
                }
            })
        },
    ) {
        Ok(handle) => handle,
        Err(error) => {
            log::error!("Radial menu: {error}");
            return;
        }
    };
    cx.global_mut::<RadialState>().0 = Some(handle);
    if let Some(hwnd) = handle
        .update(cx, |_, window, _| winutil::hwnd(window))
        .ok()
        .flatten()
    {
        let raw = hwnd.0 as isize;
        winutil::after_update(cx, move || {
            let hwnd = win::hwnd(raw);
            if game {
                win::unlock_input_from_game();
            }
            win::no_frame(hwnd);
            win::place_topmost(hwnd, bounds);
            win::force_foreground(hwnd, game);
        });
        if game {
            winutil::keep_over_game(cx, handle, raw);
        }
    }
}

fn clamp(value: i32, lo: i32, hi: i32) -> i32 {
    if lo > hi {
        lo + (hi - lo) / 2
    } else {
        value.clamp(lo, hi)
    }
}

fn scale_at(x: i32, y: i32) -> Option<f32> {
    // The cursor can be on a monitor with a different DPI from the primary one.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut dpi_x = 0;
        let mut dpi_y = 0;
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).ok()?;
        (dpi_x > 0).then_some(dpi_x as f32 / 96.0)
    }
}

struct RadialMenu {
    focus: FocusHandle,
    center: (i32, i32),
    scale: f32,
    vk: u32,
    left_down: bool,
    right_down: bool,
    hover: Option<usize>,
    hover_amount: [f32; 3],
    opened: Instant,
    last_tick: Instant,
    frames: u32,
    closing: Option<Instant>,
    chosen: Option<Choice>,
}

impl RadialMenu {
    fn commit(&mut self, cancel: bool) {
        if self.closing.is_some() {
            return;
        }
        self.chosen = if cancel {
            None
        } else {
            self.hover
                .map(|i| [Choice::Record, Choice::Audio, Choice::Settings][i])
        };
        self.closing = Some(Instant::now());
    }

    fn tick(
        &mut self,
        cursor: (i32, i32),
        left: bool,
        right: bool,
        key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Option<Choice>> {
        let now = Instant::now();
        self.frames += 1;
        let elapsed = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        if let Some(closing) = self.closing {
            if closing.elapsed() >= Duration::from_millis(160) {
                log::info!("Radial: {} rendered frames in {:.3} s ({:.1} fps).", self.frames,
                    self.opened.elapsed().as_secs_f32(), self.frames as f32 / self.opened.elapsed().as_secs_f32());
                window.remove_window();
                return Some(self.chosen.take());
            }
            cx.notify();
            return None;
        }
        let dx = (cursor.0 - self.center.0) as f32 / self.scale;
        let dy = (cursor.1 - self.center.1) as f32 / self.scale;
        self.hover = slice_at(dx, dy, self.vk == 0);
        for (i, amount) in self.hover_amount.iter_mut().enumerate() {
            let target = if self.hover == Some(i) { 1.0 } else { 0.0 };
            *amount = approach(*amount, target, elapsed);
        }
        let left_pressed = left && !self.left_down;
        let right_pressed = right && !self.right_down;
        self.left_down = left;
        self.right_down = right;
        if right_pressed || (self.vk == 0 && left_pressed) || (self.vk != 0 && !key) {
            self.commit(right_pressed);
        }
        cx.notify();
        None
    }
}

fn slice_at(dx: f32, dy: f32, click_mode: bool) -> Option<usize> {
    let distance = dx.hypot(dy);
    if distance < INNER || (click_mode && distance > OUTER + EXTEND) {
        return None;
    }
    let angle = dy.atan2(dx).to_degrees().rem_euclid(360.0);
    [210.0_f32, 330.0, 90.0]
        .iter()
        .position(|start| (angle - start).rem_euclid(360.0) < 120.0)
}

impl Focusable for RadialMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RadialMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Input and animation advance with the display, rather than a coarse Windows timer
        // followed by another wait for presentation. Polling outside the window still works.
        window.request_animation_frame();
        let menu = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let outcome = menu.update(cx, |menu, cx| {
                menu.tick(win::cursor_pos(), win::is_key_down(1), win::is_key_down(2),
                    menu.vk == 0 || win::is_key_down(menu.vk), window, cx)
            });
            if let Ok(Some(choice)) = outcome {
                cx.global_mut::<RadialState>().0 = None;
                cx.defer(move |cx| match choice {
                    Some(Choice::Record) => recording::quick_record_primary_monitor(cx),
                    Some(Choice::Audio) => recording::quick_audio_all_sources(cx),
                    Some(Choice::Settings) => app::open_settings(cx),
                    None => {},
                });
            }
        });
        let t = (self.opened.elapsed().as_secs_f32() / 0.18).min(1.0);
        let exit = self
            .closing
            .map_or(0.0, |at| (at.elapsed().as_secs_f32() / 0.16).min(1.0));
        let growth = (0.85 + 0.15 * (1.0 - (1.0 - t).powi(3))) * (1.0 - 0.4 * exit * exit);
        let amounts = self.hover_amount;
        let hub_hover = self.hover.is_none();
        let chosen = self.chosen;
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    let (x, y) = win::cursor_pos();
                    let dx = (x - this.center.0) as f32 / this.scale;
                    let dy = (y - this.center.1) as f32 / this.scale;
                    this.hover = slice_at(dx, dy, this.vk == 0);
                    this.commit(false);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.commit(true);
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.closing.is_none() {
                    this.commit(true);
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let center = (
                            f32::from(bounds.origin.x) + RADIUS,
                            f32::from(bounds.origin.y) + RADIUS,
                        );
                        let base = rgb(0x767676);
                        let line = rgb(0x888888);
                        for (i, start) in [210.0_f32, 330.0, 90.0].into_iter().enumerate() {
                            let mid = (start + 60.0).to_radians();
                            let offset = EXTEND * amounts[i];
                            let c = (center.0 + mid.cos() * offset, center.1 + mid.sin() * offset);
                            let pts = sector(c, INNER * growth, OUTER * growth, start);
                        let fill = if chosen == Some([Choice::Record, Choice::Audio, Choice::Settings][i]) {
                            rgb(0x3C9898)
                        } else {
                            blend(base, rgb(0x666666), amounts[i])
                        };
                        paint_polygon(window, &pts, fill);
                            paint_outline(window, &pts, line, 2.0);
                            let icon = (
                                c.0 + mid.cos() * 89.0 * growth,
                                c.1 + mid.sin() * 89.0 * growth,
                            );
                            paint_icon(window, cx, i, icon, growth);
                        }
                        let hub = circle(center, INNER * growth, 48);
                        paint_polygon(window, &hub, base);
                        paint_outline(window, &hub, line, 2.0);
                        let cross = if hub_hover {
                            rgb(0x3C9898)
                        } else {
                            rgb(0xFFFFFF)
                        };
                        let arm = 12.0 * growth;
                        paint_line(
                            window,
                            (center.0 - arm, center.1 - arm),
                            (center.0 + arm, center.1 + arm),
                            cross,
                            4.0 * growth,
                        );
                        paint_line(
                            window,
                            (center.0 + arm, center.1 - arm),
                            (center.0 - arm, center.1 + arm),
                            cross,
                            4.0 * growth,
                        );
                    },
                )
                .size(px(RADIUS * 2.0)),
            )
    }
}

fn approach(current: f32, target: f32, elapsed: f32) -> f32 {
    target + (current - target) * (-27.0 * elapsed).exp()
}

fn blend(a: gpui::Rgba, b: gpui::Rgba, t: f32) -> gpui::Rgba {
    gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}
fn circle(c: (f32, f32), r: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..steps)
        .map(|i| {
            let a = i as f32 * 2.0 * PI / steps as f32;
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}
fn sector(c: (f32, f32), inner: f32, outer: f32, start: f32) -> Vec<(f32, f32)> {
    let mut pts = Vec::with_capacity(66);
    for i in 0..=32 {
        let a = (start + 120.0 * i as f32 / 32.0).to_radians();
        pts.push((c.0 + outer * a.cos(), c.1 + outer * a.sin()));
    }
    for i in (0..=32).rev() {
        let a = (start + 120.0 * i as f32 / 32.0).to_radians();
        pts.push((c.0 + inner * a.cos(), c.1 + inner * a.sin()));
    }
    pts
}
fn paint_polygon(window: &mut Window, pts: &[(f32, f32)], color: gpui::Rgba) {
    let mut path = PathBuilder::fill();
    if let Some(&(x, y)) = pts.first() {
        path.move_to(point(px(x), px(y)));
    }
    for &(x, y) in pts.iter().skip(1) {
        path.line_to(point(px(x), px(y)));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
fn paint_outline(window: &mut Window, pts: &[(f32, f32)], color: gpui::Rgba, width: f32) {
    let mut path = PathBuilder::stroke(px(width));
    if let Some(&(x, y)) = pts.first() {
        path.move_to(point(px(x), px(y)));
    }
    for &(x, y) in pts.iter().skip(1) {
        path.line_to(point(px(x), px(y)));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
fn paint_line(window: &mut Window, a: (f32, f32), b: (f32, f32), color: gpui::Rgba, width: f32) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(point(px(a.0), px(a.1)));
    path.line_to(point(px(b.0), px(b.1)));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
/// The slice icons of the C# build (`RadialMenuWindow.cs`, in git history up to 1.2.4): a
/// screen with a record dot, a microphone, a solid gear. Each is a 40 px wide Viewbox whose
/// top-left sits 20 px left of and above the slice's mid-radius point, as WPF placed it.
fn paint_icon(window: &mut Window, cx: &App, i: usize, c: (f32, f32), scale: f32) {
    let (path, w, h) = match i {
        0 => ("icons/radial-record.svg", 20.0, 19.0),
        1 => ("icons/radial-audio.svg", 18.0, 20.0),
        _ => ("icons/radial-settings.svg", 20.0, 20.0),
    };
    let width = 40.0 * scale;
    let height = width * h / w;
    let half = 20.0 * scale;
    let bounds = Bounds::new(point(px(c.0 - half), px(c.1 - half)), size(px(width), px(height)));
    let _ = window.paint_svg(bounds, path.into(), TransformationMatrix::unit(), rgb(0xFFFFFF).into(), cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_motion_is_independent_of_display_refresh_rate() {
        let mut fast = 0.0;
        for _ in 0..24 { fast = approach(fast, 1.0, 1.0 / 240.0); }
        let mut slow = 0.0;
        for _ in 0..6 { slow = approach(slow, 1.0, 1.0 / 60.0); }
        assert!((fast - slow).abs() < 0.00001);
        assert!(approach(fast, 0.0, 1.0 / 60.0) < fast);
    }
    #[test]
    fn rim_bounds_clicks_but_not_release_flicks() {
        assert_eq!(slice_at(0.0, -200.0, true), None);
        assert_eq!(slice_at(0.0, -200.0, false), Some(0));
        assert_eq!(slice_at(0.0, 0.0, false), None);
        assert_eq!(slice_at(0.0, 100.0, true), Some(2));
    }
}
