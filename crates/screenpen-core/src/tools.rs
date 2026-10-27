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
    /// Coarse blocks over a part of the screen, to hide it.
    Pixelate,
}

impl ToolKind {
    pub const ALL: [ToolKind; 12] = [
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
        ToolKind::Pixelate,
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
            ToolKind::Pixelate => "pixelate",
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

/// Side of a pixelation block, in points: big enough to make text unreadable.
pub const PIXEL_BLOCK: f32 = 12.0;

/// Pixelates `rect` of `screen` (pixels at `scale` per point; BGRA when `bgra`).
/// Without a screen the blocks are a plain grey, which hides as well.
pub fn pixelate(
    rect: Rect,
    screen: Option<&tiny_skia::Pixmap>,
    scale: f32,
    bgra: bool,
) -> Geometry {
    let rect = rect.standardized();
    let cols = (rect.width / PIXEL_BLOCK).round().max(1.0) as u32;
    let rows = (rect.height / PIXEL_BLOCK).round().max(1.0) as u32;
    let (bw, bh) = (rect.width / cols as f32, rect.height / rows as f32);
    let mut colors = Vec::with_capacity((cols * rows) as usize);
    for r in 0..rows {
        for c in 0..cols {
            let block = Rect::new(rect.x + c as f32 * bw, rect.y + r as f32 * bh, bw, bh);
            colors.push(
                screen
                    .and_then(|s| average(s, block, scale, bgra))
                    .unwrap_or([128; 3]),
            );
        }
    }
    Geometry::Pixelate {
        rect,
        cols,
        rows,
        colors,
    }
}

/// The mean RGB of `block` (points) in `screen`, sampling every other pixel.
fn average(screen: &tiny_skia::Pixmap, block: Rect, scale: f32, bgra: bool) -> Option<[u8; 3]> {
    let (w, h) = (screen.width() as i64, screen.height() as i64);
    let x0 = ((block.x * scale) as i64).clamp(0, w);
    let y0 = ((block.y * scale) as i64).clamp(0, h);
    let x1 = (((block.x + block.width) * scale).ceil() as i64).clamp(0, w);
    let y1 = (((block.y + block.height) * scale).ceil() as i64).clamp(0, h);
    let data = screen.data();
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let i = ((y * w + x) * 4) as usize;
            for k in 0..3 {
                sum[k] += data[i + k] as u64;
            }
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let [a, g, b] = sum.map(|v| (v / n) as u8);
    Some(if bgra { [b, g, a] } else { [a, g, b] })
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
