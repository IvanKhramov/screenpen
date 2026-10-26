use serde::{Deserialize, Serialize};

use crate::annotation::Geometry;
use crate::geom::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Badge,
    Eraser,
    Select,
    /// A fading trail that is never committed.
    Laser,
}

impl ToolKind {
    pub const ALL: [ToolKind; 11] = [
        ToolKind::Pen,
        ToolKind::Highlighter,
        ToolKind::Line,
        ToolKind::Arrow,
        ToolKind::Rectangle,
        ToolKind::Ellipse,
        ToolKind::Text,
        ToolKind::Badge,
        ToolKind::Eraser,
        ToolKind::Select,
        ToolKind::Laser,
    ];

    /// Stable identifier for settings files and translations.
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Pen => "pen",
            ToolKind::Highlighter => "highlighter",
            ToolKind::Line => "line",
            ToolKind::Arrow => "arrow",
            ToolKind::Rectangle => "rectangle",
            ToolKind::Ellipse => "ellipse",
            ToolKind::Text => "text",
            ToolKind::Badge => "badge",
            ToolKind::Eraser => "eraser",
            ToolKind::Select => "select",
            ToolKind::Laser => "laser",
        }
    }

    pub fn from_name(name: &str) -> Option<ToolKind> {
        ToolKind::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// Builds shape geometry from a drag. `constrain` (Shift) snaps lines and arrows to
/// 45 degree steps and makes rectangles and ellipses square.
pub fn shape_geometry(
    tool: ToolKind,
    start: Point,
    raw_end: Point,
    constrain: bool,
    curved: bool,
    corner_radius: f32,
) -> Option<Geometry> {
    match tool {
        ToolKind::Line | ToolKind::Arrow => {
            let end = if constrain {
                snap_angle(start, raw_end)
            } else {
                raw_end
            };
            if start.distance(end) < 3.0 {
                return None;
            }
            if tool == ToolKind::Line {
                return Some(Geometry::Line {
                    from: start,
                    to: end,
                });
            }
            let control = curved.then(|| curve_control(start, end));
            Some(Geometry::Arrow {
                from: start,
                to: end,
                control,
            })
        }
        ToolKind::Rectangle | ToolKind::Ellipse => {
            let rect = if constrain {
                square_rect(start, raw_end)
            } else {
                Rect::from_points(start, raw_end)
            };
            if rect.width < 3.0 && rect.height < 3.0 {
                return None;
            }
            Some(if tool == ToolKind::Rectangle {
                Geometry::Rectangle {
                    rect,
                    corner_radius,
                }
            } else {
                Geometry::Ellipse { rect }
            })
        }
        _ => None,
    }
}

/// Rotates `end` around `start` to the nearest multiple of 45 degrees, keeping the length.
pub fn snap_angle(start: Point, end: Point) -> Point {
    let (dx, dy) = ((end.x - start.x) as f64, (end.y - start.y) as f64);
    let length = dx.hypot(dy);
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    let (mut sx, mut sy) = (length * angle.cos(), length * angle.sin());
    // cos/sin leave tiny residues at 90 degree multiples; make horizontal/vertical exact.
    if sx.abs() < 1e-9 {
        sx = 0.0;
    }
    if sy.abs() < 1e-9 {
        sy = 0.0;
    }
    Point::new(start.x + sx as f32, start.y + sy as f32)
}

/// Square anchored at `start`, growing toward the drag direction.
pub fn square_rect(start: Point, end: Point) -> Rect {
    let side = (end.x - start.x).abs().max((end.y - start.y).abs());
    let corner = Point::new(
        start.x + if end.x >= start.x { side } else { -side },
        start.y + if end.y >= start.y { side } else { -side },
    );
    Rect::from_points(start, corner)
}

/// A gentle bend: the control point sits off the midpoint by a quarter of the length.
pub fn curve_control(start: Point, end: Point) -> Point {
    let mid = start.midpoint(end);
    Point::new(
        mid.x + (end.y - start.y) * 0.25,
        mid.y - (end.x - start.x) * 0.25,
    )
}
