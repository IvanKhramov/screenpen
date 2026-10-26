use crate::geom::Point;

/// One cubic Bezier piece of a stroke, drawn with a single width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BezierSegment {
    pub start: Point,
    pub control1: Point,
    pub control2: Point,
    pub end: Point,
    pub width: f32,
}

/// Turns raw pointer samples into smooth cubic segments using the 5-point midpoint
/// scheme: once five points are buffered, the fourth is moved to the midpoint of the
/// third and fifth, the curve p0->p3 (controls p1, p2) is emitted, and the buffer
/// restarts from [p3, p4]. Consecutive segments share a tangent at their joint, so
/// the stroke has no corners.
#[derive(Clone, Debug, Default)]
pub struct StrokeBuilder {
    /// Every accepted input sample, kept so the stroke can be stored and replayed.
    points: Vec<Point>,
    widths: Vec<f32>,
    buffer: Vec<Point>,
}

impl StrokeBuilder {
    pub fn new() -> StrokeBuilder {
        StrokeBuilder::default()
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }

    pub fn widths(&self) -> &[f32] {
        &self.widths
    }

    pub fn into_parts(self) -> (Vec<Point>, Vec<f32>) {
        (self.points, self.widths)
    }

    /// Adds a sample. Returns a segment when one is complete.
    pub fn add(&mut self, point: Point, width: f32) -> Option<BezierSegment> {
        if self.points.last() == Some(&point) {
            return None;
        }
        self.points.push(point);
        self.widths.push(width);
        self.buffer.push(point);
        if self.buffer.len() < 5 {
            return None;
        }
        let b = &mut self.buffer;
        b[3] = b[2].midpoint(b[4]);
        let segment = BezierSegment {
            start: b[0],
            control1: b[1],
            control2: b[2],
            end: b[3],
            width,
        };
        let (p3, p4) = (b[3], b[4]);
        b.clear();
        b.extend([p3, p4]);
        Some(segment)
    }

    /// Flushes the 1-3 points still buffered after the last full segment.
    pub fn finish(&mut self) -> Option<BezierSegment> {
        let width = self.widths.last().copied().unwrap_or(1.0);
        let b = std::mem::take(&mut self.buffer);
        match b[..] {
            [a, c] => Some(BezierSegment {
                start: a,
                control1: a.lerp(c, 1.0 / 3.0),
                control2: a.lerp(c, 2.0 / 3.0),
                end: c,
                width,
            }),
            // Quadratic a-q-c raised to a cubic.
            [a, q, c] => Some(BezierSegment {
                start: a,
                control1: a.lerp(q, 2.0 / 3.0),
                control2: c.lerp(q, 2.0 / 3.0),
                end: c,
                width,
            }),
            [a, c1, c2, e] => Some(BezierSegment {
                start: a,
                control1: c1,
                control2: c2,
                end: e,
                width,
            }),
            _ => None,
        }
    }

    /// Replays stored samples into the exact segments produced while drawing.
    pub fn segments(points: &[Point], widths: &[f32]) -> Vec<BezierSegment> {
        let mut builder = StrokeBuilder::new();
        let mut result = Vec::new();
        for (i, &p) in points.iter().enumerate() {
            let width = widths.get(i).or(widths.last()).copied().unwrap_or(1.0);
            result.extend(builder.add(p, width));
        }
        result.extend(builder.finish());
        result
    }
}
