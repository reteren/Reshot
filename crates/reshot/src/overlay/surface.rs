//! The editing surface above the dimmed frame, as a grid of tiles. Every change (a committed
//! edit, a live gesture moving, an undo) marks a rectangle damaged; only the tiles under it
//! are re-rasterised and re-uploaded. A full-desktop selection on 4K would otherwise push the
//! whole 25 MB frame to the GPU on every mouse move.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Bounds, Pixels, RenderImage, Window, point, px, size};
use reshot_core::document;
use reshot_core::editor::Editor;
use reshot_core::skia::{IRect, Rect};

use super::frame_image;

const TILE: i32 = 256;

#[derive(Default)]
pub struct Surface {
    tiles: HashMap<(i32, i32), Arc<RenderImage>>,
    damaged: Option<IRect>,
    /// Images replaced since the last frame; freed from gpui's atlas on the next paint.
    stale: Vec<Arc<RenderImage>>,
}

impl Surface {
    pub fn damage(&mut self, r: Option<IRect>) {
        let Some(r) = r else { return };
        self.damaged = Some(match self.damaged {
            Some(d) => IRect::join(&d, &r),
            None => r,
        });
    }

    /// Re-rasterises damaged tiles. Tiles that come out fully transparent are dropped.
    pub fn update(&mut self, editor: &mut Editor) {
        let Some(d) = self.damaged.take() else { return };
        let (w, h) = (editor.width(), editor.height());
        let (tx0, ty0) = (d.left.max(0) / TILE, d.top.max(0) / TILE);
        let (tx1, ty1) = ((d.right.min(w) - 1).max(0) / TILE, (d.bottom.min(h) - 1).max(0) / TILE);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let r = IRect::from_xywh(tx * TILE, ty * TILE, TILE.min(w - tx * TILE), TILE.min(h - ty * TILE));
                if r.width() <= 0 || r.height() <= 0 {
                    continue;
                }
                let mut bmp = document::new_bitmap(r.width(), r.height());
                document::with_canvas(&mut bmp, |c| {
                    c.translate((-r.left as f32, -r.top as f32));
                    c.clip_rect(Rect::from_irect(r), None, false);
                    editor.paint(c);
                });
                let old = if is_empty(&bmp) {
                    self.tiles.remove(&(tx, ty))
                } else {
                    let rb = bmp.row_bytes();
                    let img = frame_image::from_premultiplied_bgra(r.width() as u32, r.height() as u32, rb, document::pixels(&bmp));
                    self.tiles.insert((tx, ty), img)
                };
                self.stale.extend(old);
            }
        }
    }

    /// What to paint this frame: images to free from the atlas, and every tile with its
    /// logical bounds relative to the window. Taken out so it can move into a canvas closure.
    pub fn prepare(&mut self, scale: f32) -> SurfaceFrame {
        let tiles = self
            .tiles
            .iter()
            .map(|(&(tx, ty), img)| {
                let s = img.size(0);
                let b: Bounds<Pixels> = Bounds::new(
                    point(px((tx * TILE) as f32 / scale), px((ty * TILE) as f32 / scale)),
                    size(px(s.width.0 as f32 / scale), px(s.height.0 as f32 / scale)),
                );
                (b, img.clone())
            })
            .collect();
        SurfaceFrame { stale: std::mem::take(&mut self.stale), tiles }
    }

    /// Frees every tile from the atlas, when the overlay goes away.
    pub fn release(&mut self, window: &mut Window) {
        for (_, img) in self.tiles.drain() {
            let _ = window.drop_image(img);
        }
        for img in self.stale.drain(..) {
            let _ = window.drop_image(img);
        }
    }
}

pub struct SurfaceFrame {
    stale: Vec<Arc<RenderImage>>,
    tiles: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
}

impl SurfaceFrame {
    pub fn paint(self, origin: gpui::Point<Pixels>, window: &mut Window) {
        for img in self.stale {
            let _ = window.drop_image(img);
        }
        for (b, img) in self.tiles {
            let b = Bounds::new(point(origin.x + b.origin.x, origin.y + b.origin.y), b.size);
            let _ = window.paint_image(b, Default::default(), img, 0, false);
        }
    }
}

fn is_empty(b: &reshot_core::skia::Bitmap) -> bool {
    let (w, rb) = (b.width() as usize * 4, b.row_bytes());
    document::pixels(b).chunks(rb).all(|row| row[..w].iter().skip(3).step_by(4).all(|&a| a == 0))
}
