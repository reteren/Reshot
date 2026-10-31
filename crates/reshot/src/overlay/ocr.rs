//! OCR mode (SPEC §9): recognised words positioned over the frozen frame with a click-drag
//! selection model, like selecting text on a PDF. A selection spans words in reading order;
//! copying joins them with spaces and breaks lines with newlines.

use reshot_core::skia::{IRect, Point, Rect};
use reshot_ocr::{OcrLanguage, OcrLine};

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    /// In frame pixels.
    pub rect: Rect,
    pub line: usize,
}

pub struct OcrState {
    pub active: bool,
    pub busy: bool,
    pub selecting: bool,
    pub lang: OcrLanguage,
    pub hint: Option<String>,
    pub words: Vec<Word>,
    anchor: Option<usize>,
    focus: Option<usize>,
}

impl Default for OcrState {
    fn default() -> Self {
        Self {
            active: false,
            busy: false,
            selecting: false,
            lang: OcrLanguage::Auto,
            hint: None,
            words: Vec::new(),
            anchor: None,
            focus: None,
        }
    }
}

impl OcrState {
    pub fn mode_label(&self) -> &'static str {
        match self.lang {
            OcrLanguage::Ru => "RU",
            OcrLanguage::En => "EN",
            OcrLanguage::Auto => "AUTO (RU+EN)",
        }
    }

    /// Leaves OCR mode, keeping the chosen language.
    pub fn reset(&mut self) {
        *self = Self { lang: self.lang, ..Default::default() };
    }

    pub fn cycle_language(&mut self) {
        self.lang = match self.lang {
            OcrLanguage::Auto => OcrLanguage::Ru,
            OcrLanguage::Ru => OcrLanguage::En,
            OcrLanguage::En => OcrLanguage::Auto,
        };
    }

    pub fn begin(&mut self) {
        self.active = true;
        self.busy = true;
        self.selecting = false;
        self.words.clear();
        self.anchor = None;
        self.focus = None;
        self.hint = Some(format!("Recognising ({})…", self.mode_label()));
    }

    /// Takes the engine's lines; their rects are relative to `region`.
    pub fn finish(&mut self, lines: Vec<OcrLine>, region: IRect) {
        self.busy = false;
        if !self.active {
            return;
        }
        let (dx, dy) = (region.left as f32, region.top as f32);
        self.words = lines
            .into_iter()
            .enumerate()
            .flat_map(|(line, l)| {
                l.words.into_iter().map(move |w| Word {
                    text: w.text,
                    rect: Rect::from_ltrb(w.rect.left + dx, w.rect.top + dy, w.rect.right + dx, w.rect.bottom + dy),
                    line,
                })
            })
            .collect();
        self.hint = Some(if self.words.is_empty() {
            format!("No text found ({}). Right-click the tool to switch mode.", self.mode_label())
        } else {
            format!(
                "OCR {} · drag to select · Ctrl+C copy · Ctrl+A all · right-click tool: mode · Esc exit",
                self.mode_label()
            )
        });
    }

    /// Nearest word to a point by distance to its box, for forgiving hit-testing.
    fn nearest(&self, p: Point) -> Option<usize> {
        self.words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let r = w.rect;
                let (cx, cy) = (p.x.clamp(r.left, r.right), p.y.clamp(r.top, r.bottom));
                (i, (p.x - cx).powi(2) + (p.y - cy).powi(2))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    pub fn start_select(&mut self, p: Point) {
        if self.words.is_empty() {
            return;
        }
        self.anchor = self.nearest(p);
        self.focus = self.anchor;
        self.selecting = true;
    }

    pub fn extend_select(&mut self, p: Point) {
        if let Some(i) = self.nearest(p) {
            self.focus = Some(i);
        }
    }

    pub fn select_all(&mut self) {
        if !self.words.is_empty() {
            self.anchor = Some(0);
            self.focus = Some(self.words.len() - 1);
        }
    }

    pub fn selected_range(&self) -> Option<(usize, usize)> {
        let (a, f) = (self.anchor?, self.focus?);
        Some((a.min(f), a.max(f)))
    }

    /// The outlines to draw: one per recognised line rather than one per word, so a line reads
    /// as a single row. The selected part of a line gets its own box spanning the selected
    /// words (`true`), drawn after the line's own; a fully selected line has only that one.
    pub fn line_boxes(&self) -> Vec<(Rect, bool)> {
        let range = self.selected_range();
        let mut plain = Vec::new();
        let mut selected = Vec::new();
        let mut i = 0;
        while i < self.words.len() {
            let line = self.words[i].line;
            let end = self.words[i..].iter().position(|w| w.line != line).map_or(self.words.len(), |n| i + n);
            let union = |a: usize, b: usize| {
                self.words[a..b].iter().map(|w| w.rect).reduce(Rect::join2).expect("non-empty span")
            };
            let picked = range.map(|(a, b)| (a.max(i), (b + 1).min(end))).filter(|(a, b)| a < b);
            if picked != Some((i, end)) {
                plain.push((union(i, end), false));
            }
            if let Some((a, b)) = picked {
                selected.push((union(a, b), true));
            }
            i = end;
        }
        plain.extend(selected);
        plain
    }

    fn join(&self, a: usize, b: usize) -> String {
        let mut out = String::new();
        let mut line = self.words[a].line;
        for (i, w) in self.words[a..=b].iter().enumerate() {
            if w.line != line {
                out.push('\n');
                line = w.line;
            } else if i > 0 {
                out.push(' ');
            }
            out.push_str(&w.text);
        }
        out
    }

    /// The selection if there is one, else everything; with a description of which.
    pub fn text_to_copy(&self) -> Option<(String, &'static str)> {
        if self.words.is_empty() {
            return None;
        }
        let (text, scope) = match self.selected_range() {
            Some((a, b)) => (self.join(a, b), "selection"),
            None => (self.join(0, self.words.len() - 1), "all text"),
        };
        (!text.is_empty()).then_some((text, scope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reshot_ocr::{OcrRect, OcrWord};

    fn state() -> OcrState {
        let w = |t: &str, l: f32| OcrWord { text: t.into(), rect: OcrRect { left: l, top: 0.0, right: l + 10.0, bottom: 10.0 } };
        let mut s = OcrState::default();
        s.begin();
        s.finish(
            vec![OcrLine { words: vec![w("hello", 0.0), w("world", 20.0)] }, OcrLine { words: vec![w("next", 0.0)] }],
            IRect::from_xywh(100, 100, 50, 50),
        );
        s
    }

    #[test]
    fn rects_are_moved_into_frame_space_and_lines_join_with_newlines() {
        let s = state();
        assert_eq!(s.words[1].rect.left, 120.0);
        assert_eq!(s.text_to_copy().unwrap(), ("hello world\nnext".to_string(), "all text"));
    }

    #[test]
    fn drag_selects_in_reading_order_either_way() {
        let mut s = state();
        s.start_select(Point::new(125.0, 105.0));
        s.extend_select(Point::new(101.0, 105.0));
        assert_eq!(s.text_to_copy().unwrap().0, "hello world");
    }

    #[test]
    fn boxes_cover_whole_lines_and_the_selected_span() {
        let mut s = state();
        // Nothing selected: one thin box per line, spanning its words.
        assert_eq!(
            s.line_boxes(),
            vec![(Rect::from_ltrb(100.0, 100.0, 130.0, 110.0), false), (Rect::from_ltrb(100.0, 100.0, 110.0, 110.0), false)]
        );
        // "world" alone: the first line keeps its thin box, the selected word gets a bold one.
        s.start_select(Point::new(125.0, 105.0));
        assert_eq!(s.line_boxes().last(), Some(&(Rect::from_ltrb(120.0, 100.0, 130.0, 110.0), true)));
        assert_eq!(s.line_boxes().len(), 3);
        // Everything: only bold boxes, one per line.
        s.select_all();
        assert!(s.line_boxes().iter().all(|(_, selected)| *selected));
        assert_eq!(s.line_boxes().len(), 2);
    }

    #[test]
    fn language_cycles_auto_ru_en() {
        let mut s = OcrState::default();
        s.cycle_language();
        assert_eq!(s.lang, OcrLanguage::Ru);
        s.cycle_language();
        s.cycle_language();
        assert_eq!(s.lang, OcrLanguage::Auto);
    }
}
