//! The settings dialog's controls, mapped rule by rule from the webview it replaces
//! (`src/reshot-tauri/src/styles.css`). Sizes are the CSS ones; text sits on the same line
//! boxes (`line-height` 125 % for the shell, 150 % for paragraphs).
//!
//! CSS gives every side of a border its own colour, gpui one colour per element, so the
//! four-colour VGUI bevels are 1 px strips laid over a box that keeps a 1 px padding for
//! them. Callers pad by the CSS padding plus that pixel.

use crate::theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    Div, ElementId, FontWeight, InteractiveElement, ParentElement, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, TextRun, Window, canvas, div, px, rgb, svg,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The face the webview bundled through @fontsource; embedded by `assets::register_fonts`.
pub const FONT: &str = "Reshot Roboto";

/// Chromium's placeholder grey, what the empty audio-hotkey field showed.
pub fn placeholder() -> Rgba {
    rgb(0x757575)
}

/// Line box of the shell's `line-height: 125%` at 12 px.
pub const LINE: f32 = 15.;
/// Line box of a `<p>` (`line-height: 150%`) at 12 px.
pub const PARA: f32 = 18.;

/// Pair kerning on, as Chrome laid the webview's text out. gpui hands DirectWrite an
/// explicit (empty) feature set, which leaves it off.
pub fn kerning() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("kern".into(), 1)]))
}

/// Text in the dialog's light weight.
pub fn text(size: f32, line: f32) -> Div {
    let mut el = div()
        .font_family(FONT)
        .font_weight(FontWeight::LIGHT)
        .text_size(px(size))
        .line_height(px(line));
    el.text_style().get_or_insert_with(Default::default).font_features = Some(kerning());
    el
}

/// A paragraph broken into lines the way Chrome breaks them. gpui's own wrapping differs in
/// two ways that move words between lines: it counts the space after a line's last word
/// against the width (browsers let it hang past the edge), and it sums unkerned advances.
/// So the block is broken here, greedily, measuring each candidate line with the shaper
/// (kerned), at spaces and after a hyphen inside a word, and shown with explicit breaks.
///
/// The width is only known after layout: the first frame wraps natively and reports its box
/// width; a changed width re-breaks the text and redraws. Results are cached per text, size
/// and width, so after a tab's first frame this costs a lookup.
pub fn wrapped(s: impl Into<SharedString>, size: f32, line: f32) -> Div {
    let s: SharedString = s.into();
    let shown = BREAKS.with(|b| b.borrow().get(&(s.clone(), size.to_bits())).map(|(_, l)| l.clone()));
    let el = text(size, line).relative().min_w_0();
    let el = match shown {
        Some(lines) => el.whitespace_nowrap().child(lines),
        None => el.child(s.clone()),
    };
    el.child(
        canvas(
            move |bounds, window, cx| {
                let width = f32::from(bounds.size.width);
                let key = (s.clone(), size.to_bits());
                let stale = BREAKS.with(|b| b.borrow().get(&key).is_none_or(|(w, _)| (*w - width).abs() > 0.25));
                if stale && width > 0.0 {
                    let lines = break_lines(&s, width, size, window);
                    BREAKS.with(|b| b.borrow_mut().insert(key, (width, lines)));
                    // Not `refresh()` here: a refresh asked for while the frame is being drawn
                    // can be cleared when that frame finishes, leaving the first layout up.
                    window.defer(cx, |window, _| window.refresh());
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full(),
    )
}

type BreakKey = (SharedString, u32);
thread_local! {
    static BREAKS: RefCell<HashMap<BreakKey, (f32, SharedString)>> = RefCell::new(HashMap::new());
}

/// DirectWrite misses a few of the kerning pairs HarfBuzz applies in Chrome, so a line that
/// carries them measures up to a pixel wider here. Measured against Edge on the dialog's own
/// lines: the tightest line Chrome kept was 0.39 px over by this measure, the loosest it broke
/// 0.68 px over.
const FIT_TOLERANCE: f32 = 0.5;

fn break_lines(s: &str, width: f32, size: f32, window: &Window) -> SharedString {
    let font = gpui::Font { weight: FontWeight::LIGHT, features: kerning(), ..gpui::font(FONT) };
    let measure = |line: &str| -> f32 {
        let run = TextRun {
            len: line.len(),
            font: font.clone(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(window.text_system().shape_line(line.to_string().into(), px(size), &[run], None).width)
    };
    // Break opportunities: after a space, and after a hyphen between two letters.
    let mut tokens = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let end = at + c.len_utf8();
        let next_is_space = chars.get(i + 1).is_some_and(|(_, n)| *n == ' ');
        let hyphen = c == '-'
            && i > 0
            && chars[i - 1].1.is_alphabetic()
            && chars.get(i + 1).is_some_and(|(_, n)| n.is_alphabetic());
        if (c == ' ' && !next_is_space) || hyphen {
            tokens.push(&s[start..end]);
            start = end;
        }
    }
    if start < s.len() {
        tokens.push(&s[start..]);
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for token in tokens {
        let candidate = format!("{current}{token}");
        if current.is_empty() || measure(candidate.trim_end()) <= width + FIT_TOLERANCE {
            current = candidate;
        } else {
            lines.push(current.trim_end().to_string());
            current = token.to_string();
        }
    }
    lines.push(current.trim_end().to_string());
    lines.join("
").into()
}

/// A `<p>`: 12 px on an 18 px line.
pub fn para(s: impl Into<SharedString>) -> Div {
    wrapped(s, 12., PARA)
}

/// `.hint`: 11 px at 55 % opacity, still on a 150 % line.
pub fn hint(s: impl Into<SharedString>) -> Div {
    wrapped(s, 11., 16.5).opacity(0.55)
}

/// Four 1 px edges, each in its own colour (`None` leaves that side bare). Top and left are
/// laid last so they own the corners, as the bevels read best that way.
pub fn edges(
    el: Div,
    id: &SharedString,
    sides: [Option<Rgba>; 4],
    pressed: Option<[Option<Rgba>; 4]>,
) -> Div {
    let [top, right, bottom, left] = sides;
    let strip = |side: usize, colour: Option<Rgba>| {
        let mut s = div()
            .id(ElementId::Name(format!("{id}-edge{side}").into()))
            .absolute();
        s = match side {
            0 => s.top_0().left_0().right_0().h(px(1.)),
            1 => s.top_0().bottom_0().right_0().w(px(1.)),
            2 => s.bottom_0().left_0().right_0().h(px(1.)),
            _ => s.top_0().bottom_0().left_0().w(px(1.)),
        };
        if let Some(c) = colour {
            s = s.bg(c);
        }
        if let Some(p) = pressed.and_then(|p| p[side]) {
            s = s.group_active(id.clone(), move |st| st.bg(p));
        }
        s
    };
    // The strips need a positioned box; an element the caller already made absolute is one.
    let mut el = el;
    if el.style().position.is_none() {
        el = el.relative();
    }
    el.child(strip(1, right))
        .child(strip(2, bottom))
        .child(strip(0, top))
        .child(strip(3, left))
}

/// Light top/left, dark bottom/right: buttons, the slider thumb, kbd chips, select values.
pub fn raised(el: Div, id: &SharedString, presses: bool) -> Div {
    let (l, d) = (theme::bevel_light(), theme::bevel_dark());
    edges(
        el,
        id,
        [Some(l), Some(d), Some(d), Some(l)],
        presses.then_some([Some(d), Some(l), Some(l), Some(d)]),
    )
}

/// Raised, with the bevel shown inverted (a thumb being dragged).
pub fn pressed(el: Div, id: &SharedString) -> Div {
    let (l, d) = (theme::bevel_light(), theme::bevel_dark());
    edges(el, id, [Some(d), Some(l), Some(l), Some(d)], None)
}

/// Dark top/left, light bottom/right: inputs, wells, lists, the slider track.
pub fn sunken(el: Div, id: &SharedString, dark: Rgba, light: Rgba) -> Div {
    edges(el, id, [Some(dark), Some(light), Some(light), Some(dark)], None)
}

/// `.button`: 24 px tall, at least 70 wide, centred light text; no hover, a press inverts
/// the bevel, disabled text turns black.
pub fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    compact: bool,
    disabled: bool,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let el = text(12., LINE)
        .h(px(24.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .bg(theme::btn_bg())
        .text_color(if disabled { rgb(0x000000) } else { theme::text() })
        .when(compact, |d| d.px(px(9.)))
        .when(!compact, |d| d.min_w(px(70.)).px(px(5.)))
        .child(label.into());
    raised(el, &id, !disabled)
        .id(ElementId::Name(id.clone()))
        .group(id)
        .when(!disabled, |d| d.cursor_pointer())
}

/// `.checkmark`: a 12 px sunken box, the tick scaled to 80 % of its inside.
pub fn check_box(id: &SharedString, checked: bool) -> Div {
    let el = div()
        .size(px(12.))
        .flex_none()
        .bg(theme::check_bg())
        .when(checked, |d| {
            d.child(
                svg()
                    .path("icons/check.svg")
                    .absolute()
                    .left(px(2.))
                    .top(px(2.))
                    .size(px(8.))
                    .text_color(theme::text()),
            )
        });
    sunken(el, &format!("{id}-box").into(), theme::check_dark(), theme::check_light())
}

/// `.checkboxWrapper`: box and label 8 px apart; a disabled one fades to 45 %.
pub fn checkbox(
    id: impl Into<SharedString>,
    checked: bool,
    label: &'static str,
    enabled: bool,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(ElementId::Name(id.clone()))
        .flex()
        .items_center()
        .gap(px(8.))
        .when(enabled, |d| d.cursor_pointer())
        .when(!enabled, |d| d.opacity(0.45))
        .child(check_box(&id, checked))
        .child(text(12., LINE).child(label))
}

/// `kbd`: an 11 px raised chip.
pub fn kbd(id: impl Into<SharedString>, key: impl Into<SharedString>) -> Div {
    let el = text(11., 16.5)
        .flex_none()
        .whitespace_nowrap()
        .bg(theme::btn_bg())
        .px(px(6.))
        .py(px(2.))
        .child(key.into());
    raised(el, &id.into(), false)
}
