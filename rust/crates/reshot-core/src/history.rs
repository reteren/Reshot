//! Undo and redo (ARCHITECTURE §5, SPEC §12): a bounded command stack, 32 steps deep.
//!
//! Commands do not hold references into the state they edit; they receive it on undo and
//! redo. `Ctx` is whatever the command needs: a [`CaptureDocument`] for raster edits, the
//! overlay's whole session for selection changes.

use skia_safe::{Bitmap, IRect};
use std::collections::VecDeque;

use crate::document::{self, CaptureDocument, Layer};

pub const MAX_DEPTH: usize = 32;

pub trait UndoableCommand<Ctx: ?Sized> {
    fn undo(&mut self, ctx: &mut Ctx);
    fn redo(&mut self, ctx: &mut Ctx);
    /// Payload bytes retained by the command, for diagnostics.
    fn retained_bytes(&self) -> usize {
        0
    }
}

pub struct UndoHistory<Ctx: ?Sized> {
    undo: VecDeque<Box<dyn UndoableCommand<Ctx> + Send>>,
    redo: Vec<Box<dyn UndoableCommand<Ctx> + Send>>,
}

impl<Ctx: ?Sized> Default for UndoHistory<Ctx> {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
        }
    }
}

impl<Ctx: ?Sized> UndoHistory<Ctx> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Records an already-applied edit. Clears the redo stack and drops the oldest step past
    /// [`MAX_DEPTH`].
    pub fn push(&mut self, command: impl UndoableCommand<Ctx> + Send + 'static) {
        self.undo.push_back(Box::new(command));
        while self.undo.len() > MAX_DEPTH {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, ctx: &mut Ctx) -> bool {
        let Some(mut c) = self.undo.pop_back() else {
            return false;
        };
        c.undo(ctx);
        self.redo.push(c);
        true
    }

    pub fn redo(&mut self, ctx: &mut Ctx) -> bool {
        let Some(mut c) = self.redo.pop() else {
            return false;
        };
        c.redo(ctx);
        self.undo.push_back(c);
        true
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn retained_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(self.redo.iter())
            .map(|c| c.retained_bytes())
            .sum()
    }
}

/// Anything that owns a document can host layer commands.
pub trait HasDocument {
    fn document_mut(&mut self) -> &mut CaptureDocument;
}

impl HasDocument for CaptureDocument {
    fn document_mut(&mut self) -> &mut CaptureDocument {
        self
    }
}

struct SnapshotRun {
    x: i32,
    y: i32,
    before: Vec<u8>,
    after: Vec<u8>,
}

enum Payload {
    /// Only the pixels that changed, row run by row run.
    Runs(Vec<SnapshotRun>),
    /// Whole before/after snapshots, when the delta would not be smaller.
    Full { before: Bitmap, after: Bitmap },
}

/// Undo for a raster edit to any layer: before and after of just the affected region. When
/// the two snapshots differ in few pixels only the changed runs are kept.
pub struct LayerRegionCommand {
    layer: Layer,
    region: IRect,
    origin_x: i32,
    origin_y: i32,
    payload: Payload,
    bytes: usize,
}

// SAFETY: skia Bitmaps own their pixel memory and are only touched from the thread that holds
// the history; the command never shares them.
unsafe impl Send for LayerRegionCommand {}

impl LayerRegionCommand {
    /// `before` and `after` come from [`document::snapshot_region`] on the same region.
    pub fn new(
        doc: &CaptureDocument,
        layer: Layer,
        region: IRect,
        before: Bitmap,
        after: Bitmap,
    ) -> Self {
        let (w, h) = (doc.width(), doc.height());
        let origin_x = region.left.clamp(0, w);
        let origin_y = region.top.clamp(0, h);
        let full_bytes = before.compute_byte_size() + after.compute_byte_size();
        let (payload, bytes) =
            if before.width() == after.width() && before.height() == after.height() {
                match build_delta(&before, &after, full_bytes) {
                    Some((runs, bytes)) => (Payload::Runs(runs), bytes),
                    None => (Payload::Full { before, after }, full_bytes),
                }
            } else {
                (Payload::Full { before, after }, full_bytes)
            };
        Self {
            layer,
            region,
            origin_x,
            origin_y,
            payload,
            bytes,
        }
    }

    /// Snapshots `region` of `layer`, runs `edit`, snapshots again and returns the command.
    pub fn record(
        doc: &mut CaptureDocument,
        layer: Layer,
        region: IRect,
        edit: impl FnOnce(&mut CaptureDocument),
    ) -> Self {
        let before = document::snapshot_region(doc.layer_mut(layer), region);
        edit(doc);
        let after = document::snapshot_region(doc.layer_mut(layer), region);
        Self::new(doc, layer, region, before, after)
    }

    fn apply(&self, doc: &mut CaptureDocument, use_before: bool) {
        let bitmap = doc.layer_mut(self.layer);
        match &self.payload {
            Payload::Full { before, after } => document::restore_region(
                bitmap,
                self.region,
                if use_before { before } else { after },
            ),
            Payload::Runs(runs) => {
                let rb = bitmap.row_bytes();
                let px = document::pixels_mut(bitmap);
                for run in runs {
                    let x = (self.origin_x + run.x) as usize;
                    let y = (self.origin_y + run.y) as usize;
                    let off = y * rb + x * 4;
                    let src = if use_before { &run.before } else { &run.after };
                    px[off..off + src.len()].copy_from_slice(src);
                }
            }
        }
    }
}

impl<Ctx: HasDocument + ?Sized> UndoableCommand<Ctx> for LayerRegionCommand {
    fn undo(&mut self, ctx: &mut Ctx) {
        self.apply(ctx.document_mut(), true);
    }

    fn redo(&mut self, ctx: &mut Ctx) {
        self.apply(ctx.document_mut(), false);
    }

    fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

fn build_delta(
    before: &Bitmap,
    after: &Bitmap,
    max_payload: usize,
) -> Option<(Vec<SnapshotRun>, usize)> {
    let (w, h) = (before.width() as usize, before.height() as usize);
    let (brb, arb) = (before.row_bytes(), after.row_bytes());
    let (b, a) = (document::pixels(before), document::pixels(after));
    let mut runs = Vec::new();
    let mut bytes = 0usize;
    for y in 0..h {
        let brow = &b[y * brb..y * brb + w * 4];
        let arow = &a[y * arb..y * arb + w * 4];
        let same = |x: usize| brow[x * 4..x * 4 + 4] == arow[x * 4..x * 4 + 4];
        let mut x = 0;
        while x < w {
            while x < w && same(x) {
                x += 1;
            }
            if x == w {
                break;
            }
            let start = x;
            while x < w && !same(x) {
                x += 1;
            }
            let (s, e) = (start * 4, x * 4);
            // A small per-run allowance, so a highly fragmented delta falls back to the raw
            // snapshots instead of retaining more payload plus overhead.
            bytes += (e - s) * 2 + 16;
            if bytes > max_payload {
                return None;
            }
            runs.push(SnapshotRun {
                x: start as i32,
                y: y as i32,
                before: brow[s..e].to_vec(),
                after: arow[s..e].to_vec(),
            });
        }
    }
    Some((runs, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::{Color, Paint, Path, Rect};
    use std::sync::{Arc, Mutex};

    struct Counter {
        undo: Box<dyn FnMut() + Send>,
        redo: Box<dyn FnMut() + Send>,
    }

    impl UndoableCommand<()> for Counter {
        fn undo(&mut self, _: &mut ()) {
            (self.undo)()
        }
        fn redo(&mut self, _: &mut ()) {
            (self.redo)()
        }
    }

    fn noop() -> Counter {
        Counter {
            undo: Box::new(|| {}),
            redo: Box::new(|| {}),
        }
    }

    #[test]
    fn undo_and_redo_roundtrip() {
        let value = Arc::new(Mutex::new(1));
        let (u, r) = (value.clone(), value.clone());
        let mut h = UndoHistory::<()>::new();
        h.push(Counter {
            undo: Box::new(move || *u.lock().unwrap() -= 1),
            redo: Box::new(move || *r.lock().unwrap() += 1),
        });
        assert!(h.can_undo());
        assert!(h.undo(&mut ()));
        assert_eq!(*value.lock().unwrap(), 0);
        assert!(h.can_redo());
        assert!(h.redo(&mut ()));
        assert_eq!(*value.lock().unwrap(), 1);
    }

    #[test]
    fn pushing_clears_redo() {
        let mut h = UndoHistory::<()>::new();
        h.push(noop());
        h.undo(&mut ());
        assert!(h.can_redo());
        h.push(noop());
        assert!(!h.can_redo());
    }

    #[test]
    fn caps_at_32_steps() {
        let undos = Arc::new(Mutex::new(0));
        let mut h = UndoHistory::<()>::new();
        for _ in 0..40 {
            let u = undos.clone();
            h.push(Counter {
                undo: Box::new(move || *u.lock().unwrap() += 1),
                redo: Box::new(|| {}),
            });
        }
        let mut count = 0;
        while h.undo(&mut ()) {
            count += 1;
        }
        assert_eq!(count, 32);
        assert_eq!(*undos.lock().unwrap(), 32);
    }

    #[test]
    fn undo_on_empty_returns_false() {
        let mut h = UndoHistory::<()>::new();
        assert!(!h.undo(&mut ()));
        assert!(!h.redo(&mut ()));
    }

    fn paint_layer_px(d: &CaptureDocument, x: i32, y: i32) -> [u8; 4] {
        let b = d.layer(Layer::Paint).unwrap();
        let i = y as usize * b.row_bytes() + x as usize * 4;
        document::pixels(b)[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn layer_command_undoes_and_redoes_a_stroke() {
        let mut doc = CaptureDocument::new(64, 64);
        let mut red = Paint::default();
        red.set_color(Color::RED);
        let region = IRect::from_ltrb(10, 10, 30, 30);
        let cmd = LayerRegionCommand::record(&mut doc, Layer::Paint, region, |d| {
            d.commit_stroke(
                &Path::rect(Rect::from_ltrb(12.0, 12.0, 20.0, 20.0), None),
                &red,
                None,
            )
        });
        assert!(
            matches!(cmd.payload, Payload::Runs(_)),
            "a small stroke should store a delta"
        );
        let mut h = UndoHistory::<CaptureDocument>::new();
        h.push(cmd);
        assert_eq!(paint_layer_px(&doc, 15, 15), [0, 0, 255, 255]);
        h.undo(&mut doc);
        assert_eq!(paint_layer_px(&doc, 15, 15), [0, 0, 0, 0]);
        h.redo(&mut doc);
        assert_eq!(paint_layer_px(&doc, 15, 15), [0, 0, 255, 255]);
    }

    #[test]
    fn fragmented_delta_falls_back_to_full_snapshots() {
        let before = document::new_bitmap(8, 8);
        let mut after = document::new_bitmap(8, 8);
        before.erase_color(Color::BLACK);
        // A checkerboard: every other pixel changes, so runs cost more than the snapshots.
        {
            let rb = after.row_bytes();
            let p = document::pixels_mut(&mut after);
            for y in 0..8 {
                for x in 0..8 {
                    let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                    p[y * rb + x * 4..y * rb + x * 4 + 4].copy_from_slice(&[0, 0, 0, v]);
                }
            }
        }
        let doc = CaptureDocument::new(8, 8);
        let cmd = LayerRegionCommand::new(
            &doc,
            Layer::Paint,
            IRect::from_ltrb(0, 0, 8, 8),
            before,
            after,
        );
        assert!(matches!(cmd.payload, Payload::Full { .. }));
    }
}
