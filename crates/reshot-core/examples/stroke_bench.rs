//! Times what the overlay does per mouse move while a region tool is dragged: the editor
//! takes the point, and every 256 px tile under the damaged area is repainted. The tile loop
//! mirrors `crates/reshot/src/overlay/surface.rs`, minus the GPU upload.
//!
//! cargo run -p reshot-core --release --example stroke_bench -- [eraser|blur|pixelize] [size] [strength] [step]

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
    let mut args = std::env::args().skip(2).map(|a| a.parse::<f32>().expect("a number"));
    if let Some(size) = args.next() {
        editor.settings_mut(tool).thickness = size;
    }
    if let Some(strength) = args.next() {
        editor.settings_mut(tool).opacity = strength;
    }
    // Distance between pointer events: 10 px is a brisk hand at 125 Hz, 2 px the same hand
    // on a 1000 Hz mouse.
    let step = args.next().unwrap_or(10.0);

    // Something to erase: a few long brush strokes across the frame.
    for y in [300.0, 500.0, 700.0] {
        editor.begin_brush(Point::new(100.0, y), None);
        editor.extend(Point::new(3000.0, y));
        editor.end_gesture();
    }
    let _ = editor.take_dirty();

    // A zig-zag stroke 3000 px long, `step` px between moves, the way a hand scrubs with an
    // eraser.
    let count = (3000.0 / step) as usize;
    let moves: Vec<Point> = (0..count)
        .map(|i| {
            let t = i as f32 * step / 10.0;
            let phase = t % 20.0;
            Point::new(200.0 + t * 10.0, 400.0 + if phase < 10.0 { phase * 30.0 } else { (20.0 - phase) * 30.0 })
        })
        .collect();
    editor.begin_region(tool, moves[0], false, None);
    let mut times = Vec::new();
    let mut tiles = 0;
    let mut extend_ms = 0.0;
    for p in &moves[1..] {
        let started = Instant::now();
        let before = editor.live_bounds();
        editor.extend(*p);
        extend_ms += started.elapsed().as_secs_f64() * 1000.0;
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
    let total: f64 = times.iter().sum();
    println!(
        "{tool:?} size {} step {step}: total {total:.0} ms (extend {extend_ms:.0} ms), {n} moves, {tiles} tiles; first {:.1} ms, max {:.1} ms; per move mean {:.1} ms, median {:.1} ms, p95 {:.1} ms, last 50 mean {:.1} ms; commit {:.1} ms",
        editor.settings(tool).thickness,
        times[0],
        sorted[n - 1],
        times.iter().sum::<f64>() / n as f64,
        sorted[n / 2],
        sorted[n * 95 / 100],
        last50,
        commit,
    );
}
