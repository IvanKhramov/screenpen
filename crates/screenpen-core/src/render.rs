//! Every annotation renders as filled paths only, so drawing, hit-testing and
//! (later) export share one geometry.

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, PathSegment, Pixmap, PixmapMut,
    PixmapPaint, Stroke, Transform,
};

use crate::annotation::{Annotation, AnnotationKind, Geometry};
use crate::geom::{Point, Rect};
use crate::stroke::{BezierSegment, StrokeBuilder};
use crate::style::{Blend, Rgba};
use crate::text;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// The annotation's style color.
    Style,
    /// Black or white, whichever reads better on the style color (badge numbers).
    Contrast,
}

#[derive(Clone, Debug)]
pub struct RenderPart {
    pub path: Path,
    pub fill: Fill,
}

/// Byte order of the target pixels. Windows DIBs are BGRA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelOrder {
    Rgba,
    Bgra,
}

pub fn skia_color(c: Rgba, order: ChannelOrder) -> Color {
    let (r, b) = match order {
        ChannelOrder::Rgba => (c.red, c.blue),
        ChannelOrder::Bgra => (c.blue, c.red),
    };
    Color::from_rgba(
        r.clamp(0.0, 1.0),
        c.green.clamp(0.0, 1.0),
        b.clamp(0.0, 1.0),
        c.alpha.clamp(0.0, 1.0),
    )
    .unwrap_or(Color::BLACK)
}

/// Outline of `path` stroked with round caps and joins.
pub fn stroked(path: &Path, width: f32) -> Option<Path> {
    let stroke = Stroke {
        width: width.max(0.5),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    path.stroke(&stroke, 2.0)
}

fn circle(center: Point, radius: f32) -> Option<Path> {
    PathBuilder::from_circle(center.x, center.y, radius.max(0.25))
}

/// Each segment is stroked with its own width and the outlines are appended into one
/// path; a non-zero fill renders the overlaps solid. This gives variable-width strokes
/// without a boolean union.
pub fn stroke_outline(points: &[Point], widths: &[f32]) -> Option<Path> {
    if points.len() < 2 {
        let p = *points.first()?;
        return circle(p, widths.first().copied().unwrap_or(1.0).max(0.5) / 2.0);
    }
    let mut pb = PathBuilder::new();
    for segment in StrokeBuilder::segments(points, widths) {
        if let Some(outline) = segment_outline(&segment) {
            pb.push_path(&outline);
        }
    }
    pb.finish()
}

pub fn segment_outline(s: &BezierSegment) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(s.start.x, s.start.y);
    pb.cubic_to(
        s.control1.x,
        s.control1.y,
        s.control2.x,
        s.control2.y,
        s.end.x,
        s.end.y,
    );
    stroked(&pb.finish()?, s.width)
}

pub fn rounded_rect(rect: Rect, radius: f32) -> Option<Path> {
    let r = rect.standardized();
    let radius = radius.max(0.0).min(r.width.min(r.height) / 2.0);
    if radius <= 0.0 {
        return Some(PathBuilder::from_rect(r.to_skia()?));
    }
    // Quarter circles as cubics.
    let k = radius * 0.552_284_8;
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let mut pb = PathBuilder::new();
    pb.move_to(l + radius, t);
    pb.line_to(rt - radius, t);
    pb.cubic_to(rt - radius + k, t, rt, t + radius - k, rt, t + radius);
    pb.line_to(rt, b - radius);
    pb.cubic_to(rt, b - radius + k, rt - radius + k, b, rt - radius, b);
    pb.line_to(l + radius, b);
    pb.cubic_to(l + radius - k, b, l, b - radius + k, l, b - radius);
    pb.line_to(l, t + radius);
    pb.cubic_to(l, t + radius - k, l + radius - k, t, l + radius, t);
    pb.close();
    pb.finish()
}

/// Shaft centerline plus a filled triangular head. The head size scales with the width.
pub struct ArrowShape {
    pub shaft: Option<Path>,
    pub head: Path,
}

impl ArrowShape {
    pub fn new(from: Point, to: Point, control: Option<Point>, width: f32) -> Option<ArrowShape> {
        let head_length = (width * 4.0).max(14.0);
        let head_half_width = head_length * 0.55;
        // Direction the arrow arrives at its tip.
        let tail = control.unwrap_or(from);
        let (mut dx, mut dy) = (to.x - tail.x, to.y - tail.y);
        let length = dx.hypot(dy);
        if length > 0.0 {
            dx /= length;
            dy /= length;
        } else {
            (dx, dy) = (1.0, 0.0);
        }
        let (nx, ny) = (-dy, dx);
        let base = Point::new(to.x - dx * head_length, to.y - dy * head_length);
        let mut pb = PathBuilder::new();
        pb.move_to(to.x, to.y);
        pb.line_to(base.x + nx * head_half_width, base.y + ny * head_half_width);
        pb.line_to(base.x - nx * head_half_width, base.y - ny * head_half_width);
        pb.close();
        let head = pb.finish()?;

        // End the shaft inside the head so the round cap doesn't poke out past its sides.
        let shaft_end = Point::new(to.x - dx * head_length * 0.6, to.y - dy * head_length * 0.6);
        let shaft = if from.distance(to) > head_length {
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            match control {
                Some(c) => pb.quad_to(c.x, c.y, shaft_end.x, shaft_end.y),
                None => pb.line_to(shaft_end.x, shaft_end.y),
            }
            pb.finish()
        } else {
            None
        };
        Some(ArrowShape { shaft, head })
    }
}

pub fn render_parts(a: &Annotation) -> Vec<RenderPart> {
    let width = a.style.width;
    let style = |path: Option<Path>| {
        path.map(|path| RenderPart {
            path,
            fill: Fill::Style,
        })
    };
    match &a.geometry {
        Geometry::Stroke { points, widths } => {
            style(stroke_outline(points, widths)).into_iter().collect()
        }
        Geometry::Line { from, to } => {
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            pb.line_to(to.x, to.y);
            style(pb.finish().and_then(|p| stroked(&p, width)))
                .into_iter()
                .collect()
        }
        Geometry::Arrow { from, to, control } => {
            let Some(shape) = ArrowShape::new(*from, *to, *control, width) else {
                return Vec::new();
            };
            let shaft = style(shape.shaft.and_then(|p| stroked(&p, width)));
            shaft.into_iter().chain(style(Some(shape.head))).collect()
        }
        Geometry::Rectangle {
            rect,
            corner_radius,
        } => style(rounded_rect(*rect, *corner_radius).and_then(|p| stroked(&p, width)))
            .into_iter()
            .collect(),
        Geometry::Ellipse { rect } => {
            let oval = rect.to_skia().and_then(PathBuilder::from_oval);
            style(oval.and_then(|p| stroked(&p, width)))
                .into_iter()
                .collect()
        }
        Geometry::Text {
            origin,
            string,
            font_size,
        } => style(text::path(string, *font_size, *origin))
            .into_iter()
            .collect(),
        Geometry::Badge {
            center,
            number,
            format,
            radius,
        } => {
            let mut parts: Vec<RenderPart> = style(circle(*center, *radius)).into_iter().collect();
            let label = format.label(*number);
            // Shrink long labels (e.g. roman numerals) to fit inside the circle.
            let count = label.chars().count() as f32;
            let font_size = radius * if count <= 2.0 { 1.1 } else { 2.2 / count };
            if let Some(glyphs) = text::path(&label, font_size, Point::default()) {
                let b = glyphs.bounds();
                let dx = center.x - (b.left() + b.right()) / 2.0;
                let dy = center.y - (b.top() + b.bottom()) / 2.0;
                if let Some(path) = glyphs.transform(Transform::from_translate(dx, dy)) {
                    parts.push(RenderPart {
                        path,
                        fill: Fill::Contrast,
                    });
                }
            }
            parts
        }
    }
}

/// Union of all parts' bounds, in points.
pub fn bounds(a: &Annotation) -> Option<Rect> {
    render_parts(a)
        .iter()
        .map(|p| Rect::from_skia(p.path.bounds()))
        .reduce(Rect::union)
}

/// True when `point` is on the annotation or within `tolerance` points of it.
pub fn hit_test(a: &Annotation, point: Point, tolerance: f32) -> bool {
    match a.kind() {
        // Glyph outlines have gaps; treat the whole box as the target.
        AnnotationKind::Text | AnnotationKind::Badge => {
            bounds(a).is_some_and(|b| b.inset(-tolerance).contains(point))
        }
        _ => render_parts(a).iter().any(|part| {
            let contours = flatten(&part.path);
            winding(&contours, point) != 0 || distance_to_edges(&contours, point) <= tolerance
        }),
    }
}

/// Draws `a` into `pixmap`. `transform` maps points to pixels (usually the screen scale).
pub fn draw(pixmap: &mut PixmapMut, a: &Annotation, transform: Transform, order: ChannelOrder) {
    let parts = render_parts(a);
    let paint = |fill: Fill| {
        let color = match fill {
            Fill::Style => a.style.color,
            Fill::Contrast => a.style.color.contrasting(),
        };
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color(skia_color(color, order));
        paint
    };
    let opacity = a.style.opacity.clamp(0.0, 1.0);
    if opacity >= 1.0 && a.style.blend == Blend::Normal {
        for part in &parts {
            pixmap.fill_path(
                &part.path,
                &paint(part.fill),
                FillRule::Winding,
                transform,
                None,
            );
        }
        return;
    }
    // Partial opacity applies to the annotation as a whole, so it goes through a layer
    // the size of its device bounds.
    let Some(device) = parts
        .iter()
        .filter_map(|p| p.path.clone().transform(transform))
        .map(|p| Rect::from_skia(p.bounds()))
        .reduce(Rect::union)
    else {
        return;
    };
    let (x, y) = (device.left().floor() - 1.0, device.top().floor() - 1.0);
    let w = (device.right().ceil() + 1.0 - x).max(1.0) as u32;
    let h = (device.bottom().ceil() + 1.0 - y).max(1.0) as u32;
    let Some(mut layer) = Pixmap::new(w, h) else {
        return;
    };
    let shifted = transform.post_translate(-x, -y);
    for part in &parts {
        layer.fill_path(
            &part.path,
            &paint(part.fill),
            FillRule::Winding,
            shifted,
            None,
        );
    }
    let blend_mode = match a.style.blend {
        Blend::Normal => tiny_skia::BlendMode::SourceOver,
        Blend::Multiply => tiny_skia::BlendMode::Multiply,
    };
    let layer_paint = PixmapPaint {
        opacity,
        blend_mode,
        ..PixmapPaint::default()
    };
    pixmap.draw_pixmap(
        x as i32,
        y as i32,
        layer.as_ref(),
        &layer_paint,
        Transform::identity(),
        None,
    );
}

// MARK: hit-testing geometry

type Contour = Vec<Point>;

/// Approximates the path with polylines, one per contour.
fn flatten(path: &Path) -> Vec<Contour> {
    const STEPS: usize = 12;
    let mut contours: Vec<Contour> = Vec::new();
    let mut current: Contour = Vec::new();
    let mut last = Point::default();
    let p = |q: tiny_skia::Point| Point::new(q.x, q.y);
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(q) => {
                if current.len() > 1 {
                    contours.push(std::mem::take(&mut current));
                }
                current.clear();
                last = p(q);
                current.push(last);
            }
            PathSegment::LineTo(q) => {
                last = p(q);
                current.push(last);
            }
            PathSegment::QuadTo(c, q) => {
                let (c, e) = (p(c), p(q));
                for i in 1..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    current.push(last.lerp(c, t).lerp(c.lerp(e, t), t));
                }
                last = e;
            }
            PathSegment::CubicTo(c1, c2, q) => {
                let (c1, c2, e) = (p(c1), p(c2), p(q));
                for i in 1..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    let (a, b, c) = (last.lerp(c1, t), c1.lerp(c2, t), c2.lerp(e, t));
                    current.push(a.lerp(b, t).lerp(b.lerp(c, t), t));
                }
                last = e;
            }
            PathSegment::Close => {
                if let Some(&first) = current.first() {
                    current.push(first);
                    last = first;
                }
            }
        }
    }
    if current.len() > 1 {
        contours.push(current);
    }
    contours
}

/// Non-zero winding number of `p` against closed contours.
fn winding(contours: &[Contour], p: Point) -> i32 {
    let mut w = 0;
    for contour in contours {
        let n = contour.len();
        for i in 0..n {
            let a = contour[i];
            let b = contour[(i + 1) % n];
            let cross = (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y);
            if a.y <= p.y {
                if b.y > p.y && cross > 0.0 {
                    w += 1;
                }
            } else if b.y <= p.y && cross < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

fn distance_to_edges(contours: &[Contour], p: Point) -> f32 {
    let mut best = f32::INFINITY;
    for contour in contours {
        for pair in contour.windows(2) {
            best = best.min(distance_to_segment(p, pair[0], pair[1]));
        }
    }
    best
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a.lerp(b, t))
}
