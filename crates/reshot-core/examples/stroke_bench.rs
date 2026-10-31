//! Times what the overlay does per mouse move while a region tool is dragged: the editor
//! takes the point, and every 256 px tile under the damaged area is repainted. The tile loop
//! mirrors `crates/reshot/src/overlay/surface.rs`, minus the GPU upload.
//!
//! cargo run -p reshot-core --release --example stroke_bench -- [eraser|blur|pixelize]

use std::time::Instant;

use reshot_core::document;
use reshot_core::editor::{DrawTool, Editor};
use reshot_core::skia::{Color, IRect, Point, Rect};

const W: i32 = 4480;
const H: i32 = 1440;
const TILE: i32 = 256;

fn repaint(editor: &mut Editor, damage: IRect) -> usize {
    let (tx0, ty0) = (damage.left.max(0) / TILE, damage.top.max(0) / TILE);
    let (tx1, ty1) = ((damage.right.min(W) - 1).max(0) / TILE, (damage.bottom.min(H) - 1).max(0) / TILE);
    let mut tiles = 0;
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let r = IRect::from_xywh(tx * TILE, ty * TILE, TILE.min(W - tx * TILE), TILE.min(H - ty * TILE));
            let mut bmp = document::new_bitmap(r.width(), r.height());
            document::with_canvas(&mut bmp, |c| {
                c.translate((-r.left as f32, -r.top as f32));
                c.clip_rect(Rect::from_irect(r), None, false);
                editor.paint(c);
            });
            tiles += 1;
        }
    }
    tiles
}

fn main() {
    let tool = match std::env::args().nth(1).as_deref() {
        Some("blur") => DrawTool::Blur,
        Some("pixelize") => DrawTool::Pixelize,
        _ => DrawTool::Eraser,
    };
    let base = document::new_bitmap(W, H);
    base.erase_color(Color::from_rgb(40, 90, 140));
    let mut editor = Editor::new(base);

    // Something to erase: a few long brush strokes across the frame.
    for y in [300.0, 500.0, 700.0] {
        editor.begin_brush(Point::new(100.0, y), None);
        editor.extend(Point::new(3000.0, y));
        editor.end_gesture();
    }
    let _ = editor.take_dirty();

    // A zig-zag stroke of 300 moves, 10 px apart, the way a hand scrubs with an eraser.
    let moves: Vec<Point> = (0..300)
        .map(|i| Point::new(200.0 + i as f32 * 10.0, 400.0 + if i % 20 < 10 { (i % 10) as f32 * 30.0 } else { (10 - i % 10) as f32 * 30.0 }))
        .collect();
    editor.begin_region(tool, moves[0], false, None);
    let mut times = Vec::new();
    let mut tiles = 0;
    for p in &moves[1..] {
        let started = Instant::now();
        let before = editor.live_bounds();
        editor.extend(*p);
        let after = editor.live_bounds();
        let damage = [before, after].into_iter().flatten().reduce(|a, b| IRect::join(&a, &b));
        if let Some(d) = damage {
            tiles += repaint(&mut editor, d);
        }
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let started = Instant::now();
    editor.end_gesture();
    let commit = started.elapsed().as_secs_f64() * 1000.0;

    let n = times.len();
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    let last50: f64 = times[n - 50..].iter().sum::<f64>() / 50.0;
    println!(
        "{tool:?}: {n} moves, {tiles} tiles; per move mean {:.1} ms, median {:.1} ms, p95 {:.1} ms, last 50 mean {:.1} ms; commit {:.1} ms",
        times.iter().sum::<f64>() / n as f64,
        sorted[n / 2],
        sorted[n * 95 / 100],
        last50,
        commit,
    );
}
