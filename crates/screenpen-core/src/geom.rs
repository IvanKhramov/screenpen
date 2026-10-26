use serde::{Deserialize, Serialize};

/// A point in a screen's top-left, y-down space, in logical points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    pub fn distance(self, other: Point) -> f32 {
        (other.x - self.x).hypot(other.y - self.y)
    }

    pub fn midpoint(self, other: Point) -> Point {
        self.lerp(other, 0.5)
    }

    pub fn lerp(self, other: Point, t: f32) -> Point {
        Point::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }

    pub fn offset(self, dx: f32, dy: f32) -> Point {
        Point::new(self.x + dx, self.y + dy)
    }
}

/// An axis-aligned rectangle. `width` and `height` are never negative once
/// `standardized`, but a drag can produce either sign before that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle spanned by two corners.
    pub fn from_points(a: Point, b: Point) -> Rect {
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (b.x - a.x).abs(),
            (b.y - a.y).abs(),
        )
    }

    pub fn standardized(self) -> Rect {
        Rect::from_points(
            Point::new(self.x, self.y),
            Point::new(self.x + self.width, self.y + self.height),
        )
    }

    pub fn left(self) -> f32 {
        self.x
    }
    pub fn top(self) -> f32 {
        self.y
    }
    pub fn right(self) -> f32 {
        self.x + self.width
    }
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub fn center(self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn offset(self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    pub fn inset(self, d: f32) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            self.width - 2.0 * d,
            self.height - 2.0 * d,
        )
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.left() && p.x <= self.right() && p.y >= self.top() && p.y <= self.bottom()
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.left() <= other.right()
            && other.left() <= self.right()
            && self.top() <= other.bottom()
            && other.top() <= self.bottom()
    }

    pub fn union(self, other: Rect) -> Rect {
        let left = self.left().min(other.left());
        let top = self.top().min(other.top());
        Rect::new(
            left,
            top,
            self.right().max(other.right()) - left,
            self.bottom().max(other.bottom()) - top,
        )
    }

    pub fn to_skia(self) -> Option<tiny_skia::Rect> {
        let r = self.standardized();
        tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height)
    }

    pub fn from_skia(r: tiny_skia::Rect) -> Rect {
        Rect::new(r.x(), r.y(), r.width(), r.height())
    }
}
