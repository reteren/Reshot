//! Selections (SPEC §4): a shape plus its bounding box, and the geometry the overlay uses to
//! draw, move and resize them. Everything here is in frame pixels; tolerances that the C#
//! build expressed in DIPs are multiplied by the display scale by the caller.

use skia_safe::{Path, PathBuilder, Point, Rect};

/// The selection sub-tools from SPEC §4.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    Ellipse,
    Lasso,
    Polygon,
    Triangle,
}

/// One selection. Bounding-box shapes are fully described by `bounds`; lasso and polygon also
/// keep their outline normalised to [0,1] within the bounds, so resizing the box re-maps them.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub kind: ShapeKind,
    pub bounds: Rect,
    pub normalized: Vec<Point>,
}

impl Selection {
    pub fn new(kind: ShapeKind, bounds: Rect) -> Self {
        Self { kind, bounds, normalized: Vec::new() }
    }

    /// A lasso or polygon from absolute points.
    pub fn from_points(kind: ShapeKind, points: &[Point]) -> Self {
        let bounds = bounds_of(points);
        Self { kind, bounds, normalized: normalize(points, bounds) }
    }

    pub fn is_freeform(&self) -> bool {
        matches!(self.kind, ShapeKind::Lasso | ShapeKind::Polygon)
    }

    pub fn has_area(&self) -> bool {
        self.bounds.width() > 0.0 && self.bounds.height() > 0.0
    }

    /// The outline within the current bounds.
    pub fn path(&self) -> Path {
        self.path_in(self.bounds)
    }

    /// The outline mapped into an arbitrary rect.
    pub fn path_in(&self, b: Rect) -> Path {
        match self.kind {
            ShapeKind::Rectangle => Path::rect(b, None),
            ShapeKind::Ellipse => Path::oval(b, None),
            ShapeKind::Triangle => {
                // Apex at top-centre, base along the bottom edge, inscribed in the box.
                let mut p = PathBuilder::new();
                p.move_to((b.center_x(), b.top)).line_to((b.right, b.bottom)).line_to((b.left, b.bottom)).close();
                p.detach()
            }
            ShapeKind::Lasso | ShapeKind::Polygon => {
                if self.normalized.len() < 2 {
                    return Path::rect(b, None);
                }
                let map = |n: &Point| Point::new(b.left + n.x * b.width(), b.top + n.y * b.height());
                let mut p = PathBuilder::new();
                p.move_to(map(&self.normalized[0]));
                for n in &self.normalized[1..] {
                    p.line_to(map(n));
                }
                p.close();
                p.detach()
            }
        }
    }
}

pub fn bounds_of(points: &[Point]) -> Rect {
    if points.is_empty() {
        return Rect::new_empty();
    }
    let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points {
        l = l.min(p.x);
        t = t.min(p.y);
        r = r.max(p.x);
        b = b.max(p.y);
    }
    Rect::from_ltrb(l, t, r, b)
}

pub fn normalize(points: &[Point], bounds: Rect) -> Vec<Point> {
    let w = if bounds.width() <= 0.0 { 1.0 } else { bounds.width() };
    let h = if bounds.height() <= 0.0 { 1.0 } else { bounds.height() };
    points.iter().map(|p| Point::new((p.x - bounds.left) / w, (p.y - bounds.top) / h)).collect()
}

/// Rect spanned by a fixed anchor and a moving corner. With a ratio (width/height) the
/// longer drag axis wins and the other follows, keeping the corner's quadrant.
pub fn rect_from_anchor(anchor: Point, corner: Point, ratio: Option<f32>) -> Rect {
    let (mut dx, mut dy) = (corner.x - anchor.x, corner.y - anchor.y);
    if let Some(r) = ratio.filter(|r| *r > 0.0) {
        let (mut aw, mut ah) = (dx.abs(), dy.abs());
        if aw >= ah * r {
            ah = aw / r;
        } else {
            aw = ah * r;
        }
        dx = aw * if dx < 0.0 { -1.0 } else { 1.0 };
        dy = ah * if dy < 0.0 { -1.0 } else { 1.0 };
    }
    let x = anchor.x.min(anchor.x + dx);
    let y = anchor.y.min(anchor.y + dy);
    Rect::from_xywh(x, y, dx.abs(), dy.abs())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    pub fn center(self, r: Rect) -> Point {
        let (cx, cy) = (r.center_x(), r.center_y());
        match self {
            Handle::TopLeft => Point::new(r.left, r.top),
            Handle::Top => Point::new(cx, r.top),
            Handle::TopRight => Point::new(r.right, r.top),
            Handle::Right => Point::new(r.right, cy),
            Handle::BottomRight => Point::new(r.right, r.bottom),
            Handle::Bottom => Point::new(cx, r.bottom),
            Handle::BottomLeft => Point::new(r.left, r.bottom),
            Handle::Left => Point::new(r.left, cy),
        }
    }

    pub fn is_corner(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::TopRight | Handle::BottomLeft | Handle::BottomRight)
    }
}

/// The handle under `p`, if any. `tolerance` is half the hit box.
pub fn hit_test_handle(p: Point, sel: Rect, tolerance: f32) -> Option<Handle> {
    Handle::ALL.into_iter().find(|h| {
        let c = h.center(sel);
        (p.x - c.x).abs() <= tolerance && (p.y - c.y).abs() <= tolerance
    })
}

/// Resizes `r` by dragging handle `h` to `p`. Corners keep the opposite corner fixed and
/// honour the ratio; edges move one side only.
pub fn resize_rect(h: Handle, r: Rect, p: Point, ratio: Option<f32>) -> Rect {
    match h {
        Handle::TopLeft => return rect_from_anchor(Point::new(r.right, r.bottom), p, ratio),
        Handle::TopRight => return rect_from_anchor(Point::new(r.left, r.bottom), p, ratio),
        Handle::BottomLeft => return rect_from_anchor(Point::new(r.right, r.top), p, ratio),
        Handle::BottomRight => return rect_from_anchor(Point::new(r.left, r.top), p, ratio),
        _ => {}
    }
    let (mut l, mut t, mut rr, mut b) = (r.left, r.top, r.right, r.bottom);
    match h {
        Handle::Top => t = p.y,
        Handle::Bottom => b = p.y,
        Handle::Left => l = p.x,
        Handle::Right => rr = p.x,
        _ => {}
    }
    Rect::from_ltrb(l.min(rr), t.min(b), l.max(rr), t.max(b))
}

/// True when `p` is on the border band of `b` (within `tol` of an edge): the move zone. The
/// interior is excluded so a new selection can be started inside an existing one.
pub fn is_on_edge(b: Rect, p: Point, tol: f32) -> bool {
    let outer = Rect::from_ltrb(b.left - tol, b.top - tol, b.right + tol, b.bottom + tol);
    if !contains(outer, p) {
        return false;
    }
    let (w, h) = (b.width() - 2.0 * tol, b.height() - 2.0 * tol);
    if w <= 0.0 || h <= 0.0 {
        return true;
    }
    !contains(Rect::from_xywh(b.left + tol, b.top + tol, w, h), p)
}

/// Inclusive on every edge, as WPF's Rect.Contains was.
pub fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.left && p.x <= r.right && p.y >= r.top && p.y <= r.bottom
}

/// Moves `r` by `delta`, keeping it inside `area`.
pub fn clamp_move(r: Rect, dx: f32, dy: f32, area: Rect) -> Rect {
    let x = (r.left + dx).clamp(area.left, (area.right - r.width()).max(area.left));
    let y = (r.top + dy).clamp(area.top, (area.bottom - r.height()).max(area.top));
    Rect::from_xywh(x, y, r.width(), r.height())
}

/// Clips `r` to `area`.
pub fn clamp_rect(r: Rect, area: Rect) -> Rect {
    let l = r.left.clamp(area.left, area.right);
    let t = r.top.clamp(area.top, area.bottom);
    let rr = r.right.clamp(area.left, area.right);
    let b = r.bottom.clamp(area.top, area.bottom);
    Rect::from_ltrb(l, t, rr.max(l), b.max(t))
}

/// Tracks Shift during a drag: held at the start locks 1:1; pressed mid-drag locks the
/// current aspect ratio until released.
#[derive(Clone, Copy, Debug, Default)]
pub struct RatioLock {
    pub shift_at_start: bool,
    locked: Option<f32>,
}

impl RatioLock {
    pub fn start(shift: bool) -> Self {
        Self { shift_at_start: shift, locked: None }
    }

    pub fn ratio(&mut self, shift_now: bool, current: Rect) -> Option<f32> {
        if self.shift_at_start {
            return Some(1.0);
        }
        if shift_now {
            if self.locked.is_none() && current.width() > 1.0 && current.height() > 1.0 {
                self.locked = Some(current.width() / current.height());
            }
            return self.locked;
        }
        self.locked = None;
        None
    }
}

/// The active selection plus the extra ones kept with Ctrl+drag.
#[derive(Clone, Debug, Default)]
pub struct SelectionSet {
    pub active: Option<Selection>,
    pub committed: Vec<Selection>,
}

impl SelectionSet {
    pub fn all(&self) -> impl Iterator<Item = &Selection> {
        self.committed.iter().chain(self.active.iter().filter(|s| s.has_area()))
    }

    pub fn is_empty(&self) -> bool {
        self.all().next().is_none()
    }

    /// Union of every selection's outline: the clip for editing tools and the export mask.
    pub fn clip_path(&self) -> Option<Path> {
        let mut it = self.all();
        let first = it.next()?.path();
        Some(it.fold(first, |acc, s| acc.op(&s.path(), skia_safe::PathOp::Union).unwrap_or(acc)))
    }

    /// Union of the bounding boxes.
    pub fn union_bounds(&self) -> Option<Rect> {
        let mut it = self.all();
        let mut r = it.next()?.bounds;
        for s in it {
            r.join(s.bounds);
        }
        Some(r)
    }

    /// Ctrl held: keep the active selection as another region. Otherwise drop them all.
    pub fn commit_active(&mut self, additive: bool) {
        if additive {
            if let Some(s) = self.active.take().filter(Selection::has_area) {
                self.committed.push(s);
            }
        } else {
            self.committed.clear();
            self.active = None;
        }
    }

    /// Index of the top-most committed selection whose edge band is under `p`.
    pub fn hit_committed(&self, p: Point, tol: f32) -> Option<usize> {
        (0..self.committed.len()).rev().find(|&i| is_on_edge(self.committed[i].bounds, p, tol))
    }

    /// Makes committed selection `i` the active one, demoting the current active.
    pub fn activate(&mut self, i: usize) {
        let picked = self.committed.remove(i);
        if let Some(cur) = self.active.take().filter(Selection::has_area) {
            self.committed.push(cur);
        }
        self.active = Some(picked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: f32, t: f32, rr: f32, b: f32) -> Rect {
        Rect::from_ltrb(l, t, rr, b)
    }

    #[test]
    fn anchor_rect_handles_every_quadrant() {
        assert_eq!(rect_from_anchor(Point::new(10.0, 10.0), Point::new(4.0, 30.0), None), r(4.0, 10.0, 10.0, 30.0));
        assert_eq!(rect_from_anchor(Point::new(10.0, 10.0), Point::new(20.0, 2.0), None), r(10.0, 2.0, 20.0, 10.0));
    }

    #[test]
    fn ratio_follows_the_longer_axis_and_keeps_direction() {
        let s = rect_from_anchor(Point::new(0.0, 0.0), Point::new(-40.0, 10.0), Some(1.0));
        assert_eq!(s, r(-40.0, 0.0, 0.0, 40.0));
        let w = rect_from_anchor(Point::new(0.0, 0.0), Point::new(10.0, 30.0), Some(2.0));
        assert_eq!(w, r(0.0, 0.0, 60.0, 30.0));
    }

    #[test]
    fn corner_resize_pins_the_opposite_corner() {
        let base = r(10.0, 10.0, 50.0, 40.0);
        assert_eq!(resize_rect(Handle::TopLeft, base, Point::new(0.0, 0.0), None), r(0.0, 0.0, 50.0, 40.0));
        assert_eq!(resize_rect(Handle::BottomRight, base, Point::new(60.0, 70.0), None), r(10.0, 10.0, 60.0, 70.0));
    }

    #[test]
    fn edge_resize_moves_one_side_and_can_flip() {
        let base = r(10.0, 10.0, 50.0, 40.0);
        assert_eq!(resize_rect(Handle::Right, base, Point::new(70.0, 999.0), None), r(10.0, 10.0, 70.0, 40.0));
        assert_eq!(resize_rect(Handle::Left, base, Point::new(60.0, 0.0), None), r(50.0, 10.0, 60.0, 40.0));
    }

    #[test]
    fn handles_hit_within_tolerance() {
        let s = r(100.0, 100.0, 200.0, 160.0);
        assert_eq!(hit_test_handle(Point::new(103.0, 98.0), s, 7.5), Some(Handle::TopLeft));
        assert_eq!(hit_test_handle(Point::new(150.0, 161.0), s, 7.5), Some(Handle::Bottom));
        assert_eq!(hit_test_handle(Point::new(150.0, 130.0), s, 7.5), None);
    }

    #[test]
    fn edge_band_excludes_the_interior() {
        let s = r(100.0, 100.0, 200.0, 200.0);
        assert!(is_on_edge(s, Point::new(95.0, 150.0), 8.0));
        assert!(is_on_edge(s, Point::new(105.0, 150.0), 8.0));
        assert!(!is_on_edge(s, Point::new(150.0, 150.0), 8.0));
        assert!(!is_on_edge(s, Point::new(80.0, 150.0), 8.0));
        // Too small for an interior: all of it is edge.
        assert!(is_on_edge(r(0.0, 0.0, 10.0, 10.0), Point::new(5.0, 5.0), 8.0));
    }

    #[test]
    fn moves_and_rects_stay_inside_the_frame() {
        let area = r(0.0, 0.0, 100.0, 100.0);
        assert_eq!(clamp_move(r(10.0, 10.0, 40.0, 40.0), 200.0, -50.0, area), r(70.0, 0.0, 100.0, 30.0));
        assert_eq!(clamp_rect(r(-10.0, 50.0, 150.0, 60.0), area), r(0.0, 50.0, 100.0, 60.0));
    }

    #[test]
    fn ratio_lock_semantics() {
        let mut l = RatioLock::start(true);
        assert_eq!(l.ratio(false, r(0.0, 0.0, 10.0, 5.0)), Some(1.0));
        let mut l = RatioLock::start(false);
        assert_eq!(l.ratio(false, r(0.0, 0.0, 30.0, 10.0)), None);
        assert_eq!(l.ratio(true, r(0.0, 0.0, 30.0, 10.0)), Some(3.0));
        // Stays locked while Shift is held even as the rect changes.
        assert_eq!(l.ratio(true, r(0.0, 0.0, 90.0, 10.0)), Some(3.0));
        assert_eq!(l.ratio(false, r(0.0, 0.0, 90.0, 10.0)), None);
    }

    #[test]
    fn freeform_points_remap_with_the_box() {
        let pts = [Point::new(10.0, 10.0), Point::new(30.0, 10.0), Point::new(20.0, 30.0)];
        let mut s = Selection::from_points(ShapeKind::Polygon, &pts);
        assert_eq!(s.bounds, r(10.0, 10.0, 30.0, 30.0));
        s.bounds = r(0.0, 0.0, 40.0, 40.0);
        let b = s.path().bounds().to_owned();
        assert_eq!(b, r(0.0, 0.0, 40.0, 40.0));
    }

    #[test]
    fn set_union_and_commit() {
        let mut set = SelectionSet {
            active: Some(Selection::new(ShapeKind::Rectangle, r(0.0, 0.0, 10.0, 10.0))),
            ..Default::default()
        };
        set.commit_active(true);
        set.active = Some(Selection::new(ShapeKind::Ellipse, r(20.0, 0.0, 30.0, 10.0)));
        assert_eq!(set.union_bounds(), Some(r(0.0, 0.0, 30.0, 10.0)));
        let clip = set.clip_path().unwrap();
        assert!(clip.contains((5.0, 5.0)));
        assert!(clip.contains((25.0, 5.0)));
        assert!(!clip.contains((15.0, 5.0)));
        set.commit_active(false);
        assert!(set.is_empty());
    }
}
