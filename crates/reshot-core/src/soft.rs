//! Brush hardness and soft strokes, ported from hive (`app/src/drawing/brush.ts`).
//!
//! A disc of radius `r` has a solid core of `r * hardness` (always leaving at least a one-pixel
//! anti-aliased rim) and fades from the core to the rim along a smoothstep. A stroke is a chain
//! of capsules whose alpha depends on the distance to their segment.
//!
//! Two things keep a soft stroke free of creases, both as hive does them:
//!
//! - **Corners are curves.** A soft polyline shows every corner as a diagonal crease on its
//!   inner side, so the pointer's points are joined by a centripetal Catmull-Rom curve cut into
//!   pieces that turn by at most 3°. The curve needs the point after the one it ends at, so the
//!   last piece up to the pointer is provisional: drawn, then undone and redrawn when the next
//!   point arrives.
//! - **Crossings composite.** Within one pass over a pixel (touched again less than
//!   `pass_window` of path ago: neighbouring pieces, a resting pointer) alpha is the max, so
//!   joints never build up. When the stroke comes back from further along its path (crossing
//!   itself, going back over) the new pass is composited over the earlier ones like paint,
//!   `a + b * (1 - a)`; between one and three windows the two rules blend smoothly, so a tight V
//!   shows no seam. Pure max over the whole path left dark diagonal creases at crossings.
//!
//! The coverage is an opaque greyscale bitmap whose luminance is the strength, placed somewhere
//! in the frame and grown as the stroke needs.

use skia_safe::{Bitmap, Contains, IRect, Point, Rect};

use crate::document;

/// Input points closer than this to the previous one are ignored: sub-pixel jitter would only
/// bend the curve's tangents.
const MIN_INPUT_STEP: f32 = 0.75;
/// Max direction change between neighbouring straight pieces of a smoothed stroke.
const MAX_TURN_PER_PIECE: f32 = 3.0 * std::f32::consts::PI / 180.0;
/// Extra room the coverage grows by, so a stroke reallocates a handful of times, not per move.
const MARGIN: i32 = 128;

/// The solid core of a disc: `radius * hardness`, but never closer than one pixel to the rim,
/// so even a fully hard brush keeps an anti-aliased edge.
pub fn core_radius(radius: f32, hardness: f32) -> f32 {
    (radius * hardness.clamp(0.0, 1.0)).min(radius - 1.0).max(0.0)
}

/// Strength (0..1) at `distance` from the stroke's centre line.
pub fn edge_alpha(distance: f32, radius: f32, hardness: f32) -> f32 {
    if !radius.is_finite() || radius <= 0.0 || distance > radius {
        return 0.0;
    }
    let core = core_radius(radius, hardness);
    if distance <= core {
        return 1.0;
    }
    let edge = (radius - core).max(1e-6);
    let t = ((distance - core) / edge).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Gaussian sigma that softens a shape's outline: the outline's width times its softness,
/// scaled by 0.35, as hive rasterises shapes. A fully hard shape stays crisp.
pub fn shape_blur_sigma(thickness: f32, hardness: f32) -> f32 {
    thickness.max(0.0) * (1.0 - hardness.clamp(0.0, 1.0)) * 0.35
}

/// Samples (after `p1`, ending exactly at `p2`) of the centripetal Catmull-Rom curve through
/// `p0..p3`, dense enough that neighbouring pieces turn by at most 3°. Centripetal
/// parametrisation never loops or overshoots on sharp turns.
pub fn smooth_samples(p0: Point, p1: Point, p2: Point, p3: Point) -> Vec<Point> {
    let chord = (p2 - p1).length();
    if chord == 0.0 {
        return vec![p2];
    }
    let (inv, outv) = (p2 - p0, p3 - p1);
    let turn = (inv.x * outv.y - inv.y * outv.x).atan2(inv.x * outv.x + inv.y * outv.y).abs();
    // The tangent can swing past the chord on both ends, hence the factor 2.
    let count = ((2.0 * turn / MAX_TURN_PER_PIECE).ceil()).max((chord / 64.0).ceil()).clamp(1.0, 256.0) as usize;
    if count == 1 || turn.is_nan() || turn <= 1e-4 {
        return vec![p2];
    }
    let knot = |a: Point, b: Point| (b - a).length().sqrt().max(1e-3);
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let lerp = |a: Point, b: Point, ta: f32, tb: f32, t: f32| {
        let span = tb - ta;
        let u = if span > 0.0 { (t - ta) / span } else { 0.0 };
        a + (b - a) * u
    };
    let mut out = Vec::with_capacity(count);
    for i in 1..count {
        let t = t1 + (t2 - t1) * (i as f32 / count as f32);
        let a1 = lerp(p0, p1, t0, t1, t);
        let a2 = lerp(p1, p2, t1, t2, t);
        let a3 = lerp(p2, p3, t2, t3, t);
        let b1 = lerp(a1, a2, t0, t2, t);
        let b2 = lerp(a2, a3, t1, t3, t);
        let p = lerp(b1, b2, t1, t2, t);
        out.push(if p.x.is_finite() && p.y.is_finite() { p } else { a2 });
    }
    out.push(p2);
    out
}

/// Per-pixel stroke state over `at`, row-major.
#[derive(Default)]
struct Planes {
    /// Shown alpha (0..255), before the stroke's strength.
    value: Vec<u8>,
    /// Alpha of the earlier passes over the pixel.
    base: Vec<u8>,
    /// Max alpha of the current pass over the pixel.
    pass: Vec<u8>,
    /// Path length at which the stroke last touched the pixel; -inf = never.
    position: Vec<f32>,
    /// 0 = the current pass merges with the base by max, 255 = composites over it.
    blend: Vec<u8>,
}

impl Planes {
    fn new(len: usize) -> Self {
        Self {
            value: vec![0; len],
            base: vec![0; len],
            pass: vec![0; len],
            position: vec![f32::NEG_INFINITY; len],
            blend: vec![0; len],
        }
    }

    /// Copies `src[i..i + n]` of every plane into `self[j..j + n]`.
    fn copy_from(&mut self, src: &Planes, i: usize, j: usize, n: usize) {
        self.value[j..j + n].copy_from_slice(&src.value[i..i + n]);
        self.base[j..j + n].copy_from_slice(&src.base[i..i + n]);
        self.pass[j..j + n].copy_from_slice(&src.pass[i..i + n]);
        self.position[j..j + n].copy_from_slice(&src.position[i..i + n]);
        self.blend[j..j + n].copy_from_slice(&src.blend[i..i + n]);
    }
}

/// The pixels under the provisional tail, as they were before it was drawn.
struct Saved {
    rect: IRect,
    planes: Planes,
    length: f32,
}

/// One soft stroke being drawn: brush, blur, pixelize or any eraser.
pub struct SoftStroke {
    frame: IRect,
    radius: f32,
    hardness: f32,
    pass_window: f32,
    /// Strength (0..1) the shown value is scaled by in the bitmap, as a lookup table.
    scale: [u8; 256],
    /// Where the coverage sits in the frame; empty until the first piece.
    at: IRect,
    planes: Planes,
    bitmap: Bitmap,
    /// The last two or three pointer points the curve still needs.
    inputs: Vec<Point>,
    /// Path length drawn so far.
    length: f32,
    tail: Option<Saved>,
    drawn: Option<IRect>,
}

impl SoftStroke {
    /// Starts a stroke at `p`; a press alone leaves a dot. `strength` (0..1) scales the whole
    /// coverage once, so the stroke never builds up past it.
    pub fn new(frame: (i32, i32), p: Point, radius: f32, hardness: f32, strength: f32) -> Self {
        let radius = if radius.is_finite() { radius.max(0.5) } else { 0.5 };
        let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 1.0 };
        let mut s = Self {
            frame: IRect::from_wh(frame.0, frame.1),
            radius,
            hardness: if hardness.is_finite() { hardness.clamp(0.0, 1.0) } else { 1.0 },
            // Neighbouring pieces touch a pixel again within about a radius of path; a return
            // from further away is another pass (fully composited after about 4.5 radii).
            pass_window: (radius * 1.5).max(4.0),
            scale: std::array::from_fn(|v| (v as f32 * strength).round() as u8),
            at: IRect::new_empty(),
            planes: Planes::default(),
            bitmap: Bitmap::new(),
            inputs: vec![p],
            length: 0.0,
            tail: None,
            drawn: None,
        };
        if p.x.is_finite() && p.y.is_finite() {
            s.drawn = s.segment(p, p, 0.0, 0.0);
        } else {
            s.inputs.clear();
        }
        s
    }

    /// Adds a pointer point. Returns the frame area whose coverage changed: the redrawn tail,
    /// the piece made final and the new tail.
    pub fn add(&mut self, p: Point) -> Option<IRect> {
        if !p.x.is_finite() || !p.y.is_finite() {
            return None;
        }
        let Some(&last) = self.inputs.last() else {
            self.inputs.push(p);
            let d = self.segment(p, p, self.length, self.length);
            self.drawn = join(self.drawn, d);
            return d;
        };
        if (p - last).length() < MIN_INPUT_STEP {
            return None;
        }
        let mut damage = self.restore_tail();
        self.inputs.push(p);
        let n = self.inputs.len();
        if n >= 3 {
            // The piece up to the second-last point is final now that its next point is known.
            let i = &self.inputs;
            let p0 = if n >= 4 { i[n - 4] } else { i[n - 3] };
            let (from, samples) = (i[n - 3], smooth_samples(p0, i[n - 3], i[n - 2], i[n - 1]));
            damage = join(damage, self.draw_through(from, &samples));
            self.inputs.drain(..n - 3);
        }
        join(damage, self.draw_tail())
    }

    /// Where the coverage sits and its pixels: an opaque greyscale bitmap, luminance =
    /// strength.
    pub fn coverage(&self) -> Option<(IRect, &Bitmap)> {
        (!self.at.is_empty()).then_some((self.at, &self.bitmap))
    }

    /// Frame area the stroke has touched so far.
    pub fn drawn(&self) -> Option<IRect> {
        self.drawn
    }

    /// Draws the last piece, up to the pointer, keeping what it covers so the next point can
    /// take it back.
    fn draw_tail(&mut self) -> Option<IRect> {
        let n = self.inputs.len();
        if n < 2 {
            return None;
        }
        let (p1, p2) = (self.inputs[n - 2], self.inputs[n - 1]);
        let p0 = if n >= 3 { self.inputs[n - 3] } else { p1 };
        let samples = smooth_samples(p0, p1, p2, p2);
        // Skia's join skips empty rects, so the bounds of the points are taken by hand.
        let mut bounds = Rect::from_ltrb(p1.x, p1.y, p1.x, p1.y);
        for s in &samples {
            bounds = Rect::from_ltrb(bounds.left.min(s.x), bounds.top.min(s.y), bounds.right.max(s.x), bounds.bottom.max(s.y));
        }
        let pad = self.radius + 1.0;
        let rect = IRect::from_ltrb(
            (bounds.left - pad).floor() as i32,
            (bounds.top - pad).floor() as i32,
            (bounds.right + pad).ceil() as i32,
            (bounds.bottom + pad).ceil() as i32,
        );
        let rect = IRect::intersect(&rect, &self.frame)?;
        self.reserve(rect);
        let mut planes = Planes::new((rect.width() * rect.height()) as usize);
        let w = rect.width() as usize;
        for y in rect.top..rect.bottom {
            let i = self.index(rect.left, y);
            planes.copy_from(&self.planes, i, (y - rect.top) as usize * w, w);
        }
        let length = self.length;
        let damage = self.draw_through(p1, &samples);
        self.length = length;
        self.tail = Some(Saved { rect, planes, length });
        damage
    }

    /// Puts back the pixels under the provisional tail.
    fn restore_tail(&mut self) -> Option<IRect> {
        let saved = self.tail.take()?;
        let rect = saved.rect;
        let w = rect.width() as usize;
        for y in rect.top..rect.bottom {
            let i = self.index(rect.left, y);
            self.planes.copy_from(&saved.planes, (y - rect.top) as usize * w, i, w);
        }
        self.length = saved.length;
        self.refresh_bitmap(rect);
        Some(rect)
    }

    /// Lays capsules from `from` through every sample, advancing the path length.
    fn draw_through(&mut self, from: Point, samples: &[Point]) -> Option<IRect> {
        let mut a = from;
        let mut damage = None;
        for &b in samples {
            let len = (b - a).length();
            let d = self.segment(a, b, self.length, self.length + len);
            damage = join(damage, d);
            self.length += len;
            a = b;
        }
        self.drawn = join(self.drawn, damage);
        damage
    }

    /// Row-major index of a frame pixel inside the coverage.
    fn index(&self, x: i32, y: i32) -> usize {
        (y - self.at.top) as usize * self.at.width() as usize + (x - self.at.left) as usize
    }

    /// Grows the coverage, with a margin, until it holds `need`.
    fn reserve(&mut self, need: IRect) {
        if !self.at.is_empty() && self.at.contains(need) {
            return;
        }
        let grown = if self.at.is_empty() { need } else { IRect::join(&self.at, &need) };
        let grown = IRect::from_ltrb(grown.left - MARGIN, grown.top - MARGIN, grown.right + MARGIN, grown.bottom + MARGIN);
        let grown = IRect::intersect(&grown, &self.frame).unwrap_or(need);
        let w = grown.width() as usize;
        let mut planes = Planes::new(w * grown.height() as usize);
        let mut bitmap = document::new_bitmap(grown.width(), grown.height());
        bitmap.erase_color(skia_safe::Color::BLACK);
        if !self.at.is_empty() {
            let (old_w, old_rb, new_rb) = (self.at.width() as usize, self.bitmap.row_bytes(), bitmap.row_bytes());
            let dx = (self.at.left - grown.left) as usize;
            let old_px = document::pixels(&self.bitmap);
            let new_px = document::pixels_mut(&mut bitmap);
            for row in 0..self.at.height() as usize {
                let y = row + (self.at.top - grown.top) as usize;
                planes.copy_from(&self.planes, row * old_w, y * w + dx, old_w);
                new_px[y * new_rb + dx * 4..][..old_w * 4].copy_from_slice(&old_px[row * old_rb..][..old_w * 4]);
            }
        }
        self.at = grown;
        self.planes = planes;
        self.bitmap = bitmap;
    }

    /// Rewrites the bitmap from the shown values over `rect`.
    fn refresh_bitmap(&mut self, rect: IRect) {
        let (w, rb) = (self.at.width() as usize, self.bitmap.row_bytes());
        let (left, top) = (self.at.left, self.at.top);
        let px = document::pixels_mut(&mut self.bitmap);
        for y in rect.top..rect.bottom {
            let row = (y - top) as usize;
            for x in rect.left..rect.right {
                let col = (x - left) as usize;
                let v = self.scale[self.planes.value[row * w + col] as usize];
                px[row * rb + col * 4..][..3].fill(v);
            }
        }
    }

    /// Adds the capsule from `a` to `b`, path length `l0..l1`.
    fn segment(&mut self, a: Point, b: Point, l0: f32, l1: f32) -> Option<IRect> {
        let r = self.radius;
        let need = IRect::from_ltrb(
            (a.x.min(b.x) - r - 1.0).floor() as i32,
            (a.y.min(b.y) - r - 1.0).floor() as i32,
            (a.x.max(b.x) + r + 1.0).ceil() as i32,
            (a.y.max(b.y) + r + 1.0).ceil() as i32,
        );
        let need = IRect::intersect(&need, &self.frame)?;
        self.reserve(need);

        let core = core_radius(r, self.hardness);
        let edge = (r - core).max(1e-6);
        let (radius_sq, core_sq) = (r * r, core * core);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length_sq = dx * dx + dy * dy;
        let window = self.pass_window;
        let (w, rb) = (self.at.width() as usize, self.bitmap.row_bytes());
        let (left, top) = (self.at.left, self.at.top);
        let scale = self.scale;
        let Planes { value, base, pass, position, blend } = &mut self.planes;
        let px = document::pixels_mut(&mut self.bitmap);
        for y in need.top..need.bottom {
            let py = y as f32 + 0.5 - a.y;
            let row = (y - top) as usize;
            for x in need.left..need.right {
                let qx = x as f32 + 0.5 - a.x;
                // Distance to the segment: project onto it and clamp to its end points.
                let t = if length_sq > 0.0 { ((qx * dx + py * dy) / length_sq).clamp(0.0, 1.0) } else { 0.0 };
                let (ox, oy) = (qx - t * dx, py - t * dy);
                let d_sq = ox * ox + oy * oy;
                if d_sq > radius_sq {
                    continue;
                }
                let alpha = if d_sq <= core_sq {
                    255u8
                } else {
                    let e = ((d_sq.sqrt() - core) / edge).clamp(0.0, 1.0);
                    let v = (255.0 * (1.0 - e * e * (3.0 - 2.0 * e))).round() as u8;
                    if v == 0 {
                        continue;
                    }
                    v
                };
                let col = (x - left) as usize;
                let i = row * w + col;
                let at = l0 + t * (l1 - l0);
                let gap = at - position[i];
                if gap > window {
                    // A new pass over this pixel: what is shown becomes the base.
                    let ramp = ((gap - window) / (window * 2.0)).min(1.0);
                    base[i] = value[i];
                    pass[i] = alpha;
                    blend[i] = (255.0 * ramp * ramp * (3.0 - 2.0 * ramp)).round() as u8;
                } else if alpha > pass[i] {
                    pass[i] = alpha;
                }
                position[i] = at;
                let (below, current) = (base[i] as f32, pass[i] as f32);
                let over = below + current * (255.0 - below) / 255.0;
                let weight = blend[i] as f32 / 255.0;
                let shown = (below.max(current) * (1.0 - weight) + over * weight).round() as u8;
                if shown > value[i] {
                    value[i] = shown;
                    // Greyscale: blue, green and red carry the strength; alpha stays opaque.
                    px[row * rb + col * 4..][..3].fill(scale[shown as usize]);
                }
            }
        }
        Some(need)
    }
}

/// Union of two optional areas.
pub fn join(a: Option<IRect>, b: Option<IRect>) -> Option<IRect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(IRect::join(&a, &b)),
        (a, b) => a.or(b),
    }
}

/// The frame area a soft stroke of `radius` around `bounds` can touch.
pub fn padded(bounds: Rect, pad: f32) -> Rect {
    Rect::from_ltrb(bounds.left - pad, bounds.top - pad, bounds.right + pad, bounds.bottom + pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(s: &SoftStroke, x: i32, y: i32) -> u8 {
        let (at, b) = s.coverage().unwrap();
        document::pixels(b)[(y - at.top) as usize * b.row_bytes() + (x - at.left) as usize * 4 + 2]
    }

    fn line(frame: (i32, i32), points: &[(f32, f32)], radius: f32, hardness: f32, strength: f32) -> SoftStroke {
        let mut s = SoftStroke::new(frame, Point::new(points[0].0, points[0].1), radius, hardness, strength);
        for &(x, y) in &points[1..] {
            s.add(Point::new(x, y));
        }
        s
    }

    #[test]
    fn the_profile_matches_hive() {
        // r = 10, hardness 0.5: solid to 5, half way down the smoothstep at 7.5, gone at 10.
        assert_eq!(edge_alpha(4.0, 10.0, 0.5), 1.0);
        assert!((edge_alpha(7.5, 10.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(edge_alpha(10.5, 10.0, 0.5), 0.0);
        // A fully hard brush still keeps a one-pixel rim.
        assert_eq!(core_radius(10.0, 1.0), 9.0);
        assert_eq!(core_radius(0.5, 1.0), 0.0);
        // Hardness 0 fades from the very centre.
        assert!(edge_alpha(5.0, 10.0, 0.0) < 0.6);
    }

    #[test]
    fn a_soft_stroke_fades_and_a_hard_one_does_not() {
        let pts = [(20.0, 50.0), (50.0, 50.0), (80.0, 50.0)];
        let hard = line((100, 100), &pts, 10.0, 1.0, 1.0);
        let soft = line((100, 100), &pts, 10.0, 0.0, 1.0);
        assert_eq!(value(&hard, 50, 50), 255);
        assert_eq!(value(&hard, 50, 57), 255, "inside the hard core");
        assert!(value(&soft, 50, 57) < 128, "a soft edge has faded by then");
        assert_eq!(value(&hard, 50, 62), 0, "past the radius");
    }

    #[test]
    fn strength_scales_once_and_a_pass_back_does_not_build_up() {
        // Out and straight back over: the hard core is full in both passes, so it stays at
        // the strength rather than compositing past it.
        let s = line((100, 100), &[(10.0, 50.0), (90.0, 50.0), (10.0, 50.0)], 10.0, 1.0, 0.5);
        assert_eq!(value(&s, 50, 50), 128);
    }

    #[test]
    fn a_soft_crossing_composites_instead_of_creasing() {
        // A horizontal stroke, then far along the path a vertical one across it. On the
        // diagonal near the crossing both strokes give the same partial alpha; max would show
        // exactly that (the crease), compositing shows more.
        let (r, h) = (20.0, 0.0);
        let s = line(
            (200, 200),
            &[
                (10.0, 100.0),
                (60.0, 100.0),
                (100.0, 100.0),
                (140.0, 100.0),
                (190.0, 100.0),
                (190.0, 190.0),
                (100.0, 190.0),
                (100.0, 140.0),
                (100.0, 100.0),
                (100.0, 60.0),
                (100.0, 10.0),
            ],
            r,
            h,
            1.0,
        );
        let one = (255.0 * edge_alpha(10.5, r, h)).round();
        let shown = value(&s, 110, 110) as f32;
        let over = one + one * (255.0 - one) / 255.0;
        assert!((shown - over).abs() <= 3.0, "crossing composites ({shown} vs {over})");
    }

    #[test]
    fn a_right_angle_is_drawn_as_a_curve() {
        let samples = smooth_samples(
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 50.0),
            Point::new(50.0, 100.0),
        );
        assert!(samples.len() > 10, "{} pieces", samples.len());
        assert_eq!(*samples.last().unwrap(), Point::new(50.0, 50.0));
        // A straight run needs no extra pieces.
        let straight = smooth_samples(Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(20.0, 0.0), Point::new(30.0, 0.0));
        assert_eq!(straight, vec![Point::new(20.0, 0.0)]);
    }

    #[test]
    fn the_tail_is_provisional() {
        // With two points there is no final piece yet: the line to the pointer is the tail,
        // and taking it back leaves only the press dot.
        let mut s = SoftStroke::new((200, 200), Point::new(20.0, 100.0), 6.0, 1.0, 1.0);
        s.add(Point::new(120.0, 100.0));
        assert_eq!(value(&s, 70, 100), 255, "the tail reaches the pointer");
        assert!(s.restore_tail().is_some());
        assert_eq!(value(&s, 70, 100), 0, "and is taken back whole");
        assert_eq!(value(&s, 20, 100), 255, "the dot stays");
        // Redrawing gives the same pixels: nothing stacks.
        s.draw_tail();
        assert_eq!(value(&s, 70, 100), 255);
        assert_eq!(value(&s, 70, 107), 0);
    }

    #[test]
    fn coverage_grows_and_keeps_what_was_laid_before() {
        let s = line((2000, 100), &[(10.0, 50.0), (20.0, 50.0), (1500.0, 50.0), (1900.0, 50.0)], 5.0, 1.0, 1.0);
        assert_eq!(value(&s, 15, 50), 255);
        assert_eq!(value(&s, 1700, 50), 255);
        assert_eq!(value(&s, 800, 20), 0);
    }

    #[test]
    fn shapes_blur_only_when_soft() {
        assert_eq!(shape_blur_sigma(10.0, 1.0), 0.0);
        assert!((shape_blur_sigma(10.0, 0.0) - 3.5).abs() < 1e-6);
    }
}
