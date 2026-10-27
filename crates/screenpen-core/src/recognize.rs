//! Straightens a hand-drawn stroke into the shape it looks like: a line, a rectangle
//! or an ellipse. Used when the pen holds still at the end of a stroke.

use crate::annotation::Geometry;
use crate::geom::{Point, Rect};

/// Shortest line and smallest closed shape recognized, in points.
const MIN_SIZE: f32 = 16.0;
/// A line strays from its chord by at most this share of its length.
const LINE_STRAY: f32 = 0.08;
/// A closed shape ends within this share of its path length from where it began.
const CLOSED: f32 = 0.25;
/// A rectangle passes this close to each corner of its box (share of the shorter
/// side); a circle stays about 0.2 away.
const CORNER: f32 = 0.14;
/// Mean distance from the outline, as a share of the box, still taken as that shape.
const RECT_ERROR: f32 = 0.07;
const ELLIPSE_ERROR: f32 = 0.16;

pub fn recognize(points: &[Point]) -> Option<Geometry> {
    let (&first, &last) = (points.first()?, points.last()?);
    if points.len() < 4 {
        return None;
    }
    let length: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    let chord = first.distance(last);
    if chord >= MIN_SIZE && max_stray(points, first, last) <= LINE_STRAY * chord {
        return Some(Geometry::Line {
            from: first,
            to: last,
        });
    }
    let b = bounding_box(points);
    if chord > CLOSED * length || b.width.min(b.height) < MIN_SIZE {
        return None;
    }
    if near_all_corners(points, b) {
        (rect_error(points, b) <= RECT_ERROR).then_some(Geometry::Rectangle {
            rect: b,
            corner_radius: 6.0,
        })
    } else {
        (ellipse_error(points, b) <= ELLIPSE_ERROR).then_some(Geometry::Ellipse { rect: b })
    }
}

/// The farthest any point gets from the line through `a` and `b`.
fn max_stray(points: &[Point], a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    points
        .iter()
        .map(|p| ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / len)
        .fold(0.0, f32::max)
}

fn bounding_box(points: &[Point]) -> Rect {
    let (mut min, mut max) = (points[0], points[0]);
    for p in points {
        min = Point::new(min.x.min(p.x), min.y.min(p.y));
        max = Point::new(max.x.max(p.x), max.y.max(p.y));
    }
    Rect::from_points(min, max)
}

fn near_all_corners(points: &[Point], b: Rect) -> bool {
    let reach = CORNER * b.width.min(b.height);
    [
        Point::new(b.left(), b.top()),
        Point::new(b.right(), b.top()),
        Point::new(b.right(), b.bottom()),
        Point::new(b.left(), b.bottom()),
    ]
    .iter()
    .all(|c| points.iter().any(|p| p.distance(*c) <= reach))
}

/// Mean distance to the nearest side of the box, as a share of the box.
fn rect_error(points: &[Point], b: Rect) -> f32 {
    let sum: f32 = points
        .iter()
        .map(|p| {
            let dx = (p.x - b.left()).abs().min((p.x - b.right()).abs()) / b.width;
            let dy = (p.y - b.top()).abs().min((p.y - b.bottom()).abs()) / b.height;
            dx.min(dy)
        })
        .sum();
    sum / points.len() as f32
}

/// Mean distance from the ellipse inscribed in the box, in radii.
fn ellipse_error(points: &[Point], b: Rect) -> f32 {
    let c = b.center();
    let (rx, ry) = (b.width / 2.0, b.height / 2.0);
    let sum: f32 = points
        .iter()
        .map(|p| (((p.x - c.x) / rx).hypot((p.y - c.y) / ry) - 1.0).abs())
        .sum();
    sum / points.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wobble of up to `amp` points, so the tests see hand-drawn input.
    fn wobble(i: usize, amp: f32) -> f32 {
        ((i * 7919) % 13) as f32 / 12.0 * 2.0 * amp - amp
    }

    fn circle(cx: f32, cy: f32, rx: f32, ry: f32, amp: f32) -> Vec<Point> {
        (0..=60)
            .map(|i| {
                let a = i as f32 / 60.0 * std::f32::consts::TAU;
                Point::new(
                    cx + (rx + wobble(i, amp)) * a.cos(),
                    cy + (ry + wobble(i + 3, amp)) * a.sin(),
                )
            })
            .collect()
    }

    fn polyline(corners: &[(f32, f32)], amp: f32) -> Vec<Point> {
        let mut out = Vec::new();
        for w in corners.windows(2) {
            for s in 0..15 {
                let t = s as f32 / 15.0;
                let i = out.len();
                out.push(Point::new(
                    w[0].0 + (w[1].0 - w[0].0) * t + wobble(i, amp),
                    w[0].1 + (w[1].1 - w[0].1) * t + wobble(i + 5, amp),
                ));
            }
        }
        let &(x, y) = corners.last().unwrap();
        out.push(Point::new(x, y));
        out
    }

    #[test]
    fn a_wobbly_line_becomes_straight() {
        let pts = polyline(&[(10.0, 10.0), (200.0, 60.0)], 3.0);
        assert!(matches!(recognize(&pts), Some(Geometry::Line { .. })));
    }

    #[test]
    fn a_wobbly_circle_becomes_an_ellipse() {
        let g = recognize(&circle(100.0, 100.0, 60.0, 40.0, 4.0));
        let Some(Geometry::Ellipse { rect }) = g else {
            panic!("{g:?}")
        };
        assert!((rect.width - 120.0).abs() < 10.0 && (rect.height - 80.0).abs() < 10.0);
    }

    #[test]
    fn a_wobbly_box_becomes_a_rectangle() {
        let corners = [
            (20.0, 20.0),
            (220.0, 22.0),
            (218.0, 120.0),
            (22.0, 118.0),
            (24.0, 26.0),
        ];
        let g = recognize(&polyline(&corners, 3.0));
        assert!(matches!(g, Some(Geometry::Rectangle { .. })), "{g:?}");
    }

    #[test]
    fn a_scribble_stays_a_stroke() {
        // A zigzag: neither straight nor closed.
        let pts = polyline(&[(0.0, 0.0), (40.0, 80.0), (80.0, 0.0), (120.0, 80.0)], 1.0);
        assert_eq!(recognize(&pts), None);
        // An open arc.
        let arc: Vec<Point> = circle(100.0, 100.0, 60.0, 60.0, 0.0)[..30].to_vec();
        assert_eq!(recognize(&arc), None);
        // A dot.
        assert_eq!(recognize(&[Point::new(1.0, 1.0); 6]), None);
    }
}
