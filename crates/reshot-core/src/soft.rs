//! Brush hardness, with the model hive uses (`app/src/drawing/brush.ts`): a disc of radius `r`
//! has a solid core of `r * hardness` (always leaving at least a one-pixel anti-aliased rim)
//! and fades from the core to the rim along a smoothstep. A stroke is a chain of capsules, one
//! per segment, whose alpha depends on the distance to that segment; overlapping pieces keep
//! the larger value, so a stroke is one continuous shape with a smooth edge, no ripples from
//! discrete dabs, and no build-up where it passes over itself.
//!
//! Coverage is kept the way the soft erasers already did: an opaque greyscale bitmap whose
//! luminance is the strength, placed somewhere in the frame and grown as the stroke needs.

use skia_safe::{Bitmap, Color, Contains, IRect, Point, Rect};

use crate::document;

/// Coverage placed in the frame: where it sits, and its pixels.
pub type Coverage = Option<(IRect, Bitmap)>;

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

/// Lays the capsule from `a` to `b` into `coverage` at `strength` (0..1), keeping the larger
/// value per pixel. The coverage grows with a margin when the capsule reaches past it, so a
/// stroke reallocates a handful of times rather than on every move. Returns the frame area
/// the capsule covers, for repainting and for the commit's undo snapshot.
pub fn accumulate_segment(
    coverage: &mut Coverage,
    frame: (i32, i32),
    a: Point,
    b: Point,
    radius: f32,
    hardness: f32,
    strength: f32,
) -> Option<IRect> {
    const MARGIN: i32 = 128;
    if !radius.is_finite() || radius <= 0.0 || ![a.x, a.y, b.x, b.y, hardness, strength].iter().all(|v| v.is_finite()) {
        return None;
    }
    let frame_rect = IRect::from_wh(frame.0, frame.1);
    let need = IRect::from_ltrb(
        (a.x.min(b.x) - radius - 1.0).floor() as i32,
        (a.y.min(b.y) - radius - 1.0).floor() as i32,
        (a.x.max(b.x) + radius + 1.0).ceil() as i32,
        (a.y.max(b.y) + radius + 1.0).ceil() as i32,
    );
    let need = IRect::intersect(&need, &frame_rect)?;
    if !coverage.as_ref().is_some_and(|(at, _)| at.contains(need)) {
        let grown = match coverage.as_ref() {
            Some((at, _)) => IRect::join(at, &need),
            None => need,
        };
        let grown = IRect::from_ltrb(grown.left - MARGIN, grown.top - MARGIN, grown.right + MARGIN, grown.bottom + MARGIN);
        let grown = IRect::intersect(&grown, &frame_rect).unwrap_or(need);
        let mut bitmap = document::new_bitmap(grown.width(), grown.height());
        bitmap.erase_color(Color::BLACK);
        if let Some((at, old)) = coverage.as_ref() {
            document::with_canvas(&mut bitmap, |c| {
                c.draw_image(old.as_image(), (at.left - grown.left, at.top - grown.top), None);
            });
        }
        *coverage = Some((grown, bitmap));
    }

    let (at, bitmap) = coverage.as_mut().expect("allocated above");
    let at = *at;
    let row_bytes = bitmap.row_bytes();
    let px = document::pixels_mut(bitmap);
    let strength = strength.clamp(0.0, 1.0);
    let core = core_radius(radius, hardness);
    let edge = (radius - core).max(1e-6);
    let (radius_sq, core_sq) = (radius * radius, core * core);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_sq = dx * dx + dy * dy;
    let full = (255.0 * strength).round() as u8;
    for y in need.top..need.bottom {
        let py = y as f32 + 0.5 - a.y;
        let row = (y - at.top) as usize * row_bytes;
        for x in need.left..need.right {
            let qx = x as f32 + 0.5 - a.x;
            // Distance to the segment: project onto it and clamp to its end points.
            let t = if length_sq > 0.0 { ((qx * dx + py * dy) / length_sq).clamp(0.0, 1.0) } else { 0.0 };
            let (ox, oy) = (qx - t * dx, py - t * dy);
            let d_sq = ox * ox + oy * oy;
            if d_sq > radius_sq {
                continue;
            }
            let value = if d_sq <= core_sq {
                full
            } else {
                let e = ((d_sq.sqrt() - core) / edge).clamp(0.0, 1.0);
                (255.0 * strength * (1.0 - e * e * (3.0 - 2.0 * e))).round() as u8
            };
            let i = row + (x - at.left) as usize * 4;
            // Greyscale: blue, green and red carry the strength; alpha stays opaque.
            if value > px[i + 2] {
                px[i..i + 3].fill(value);
            }
        }
    }
    Some(need)
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

    fn value(cov: &Coverage, x: i32, y: i32) -> u8 {
        let (at, b) = cov.as_ref().unwrap();
        document::pixels(b)[(y - at.top) as usize * b.row_bytes() + (x - at.left) as usize * 4 + 2]
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
    fn a_soft_capsule_fades_and_a_hard_one_does_not() {
        let frame = (100, 100);
        let (a, b) = (Point::new(20.0, 50.0), Point::new(80.0, 50.0));
        let mut hard = None;
        accumulate_segment(&mut hard, frame, a, b, 10.0, 1.0, 1.0);
        let mut soft = None;
        accumulate_segment(&mut soft, frame, a, b, 10.0, 0.0, 1.0);
        assert_eq!(value(&hard, 50, 50), 255);
        assert_eq!(value(&hard, 50, 57), 255, "inside the hard core");
        assert!(value(&soft, 50, 57) < 128, "a soft edge has faded by then");
        assert_eq!(value(&hard, 50, 62), 0, "past the radius");
    }

    #[test]
    fn overlaps_keep_the_larger_value_and_strength_scales_it() {
        let frame = (100, 100);
        let p = Point::new(50.0, 50.0);
        let mut cov = None;
        accumulate_segment(&mut cov, frame, p, p, 10.0, 1.0, 0.5);
        accumulate_segment(&mut cov, frame, p, p, 10.0, 1.0, 0.5);
        assert_eq!(value(&cov, 50, 50), 128, "a second pass does not build up");
        accumulate_segment(&mut cov, frame, p, p, 10.0, 1.0, 0.25);
        assert_eq!(value(&cov, 50, 50), 128, "a weaker pass changes nothing");
    }

    #[test]
    fn coverage_grows_and_keeps_what_was_laid_before() {
        let frame = (2000, 100);
        let mut cov = None;
        accumulate_segment(&mut cov, frame, Point::new(10.0, 50.0), Point::new(20.0, 50.0), 5.0, 1.0, 1.0);
        accumulate_segment(&mut cov, frame, Point::new(1500.0, 50.0), Point::new(1900.0, 50.0), 5.0, 1.0, 1.0);
        assert_eq!(value(&cov, 15, 50), 255);
        assert_eq!(value(&cov, 1700, 50), 255);
        assert_eq!(value(&cov, 800, 50), 0);
    }

    #[test]
    fn shapes_blur_only_when_soft() {
        assert_eq!(shape_blur_sigma(10.0, 1.0), 0.0);
        assert!((shape_blur_sigma(10.0, 0.0) - 3.5).abs() < 1e-6);
    }
}
