//! Bridge native Windows Unicode/IME input into the current annotation. KeyDown alone
//! misses VK_PACKET/WM_CHAR input, which Windows sends directly to the input handler.

use std::ops::Range;
use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point, px, size};
use super::Overlay;

impl EntityInputHandler for Overlay {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let (text, range) = self.editor.text_for_range_utf16(range)?;
        *actual = Some(range);
        Some(text)
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let end = self.editor.text_editing()?.text.encode_utf16().count();
        Some(UTF16Selection { range: end..end, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.editor.marked_text_range()
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) { self.editor.unmark_text(); }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_typing() && !self.panel.has_field() {
            self.with_live(cx, |this| this.editor.replace_native_text(range, text, false));
        }
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_typing() && !self.panel.has_field() {
            self.with_live(cx, |this| this.editor.replace_native_text(range, text, true));
        }
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let bounds = self.editor.text_editing()?.compute_bounds();
        Some(Bounds::new(point(px(bounds.left / self.scale), px(bounds.top / self.scale)),
            size(px(bounds.width() / self.scale), px(bounds.height() / self.scale))))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.editor.text_editing()?.text.encode_utf16().count())
    }
}
