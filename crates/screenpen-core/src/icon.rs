//! The app icon, drawn in code so every size is sharp: the packages get it as files
//! (`examples/icon.rs`), the tray draws it with a status dot.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Paint, PathBuilder, Pixmap, Point,
    Shader, SpreadMode, Stroke, Transform,
};

/// The icon on a 1024 grid, drawn at `size` pixels.
pub fn draw(size: u32) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(size, size)?;
    let k = size as f32 / 1024.0;
    let t = Transform::from_scale(k, k);
    let small = size <= 32;

    // A rounded square, blue to indigo, with a margin like other macOS icons.
    let (m, r) = if small { (16.0, 200.0) } else { (100.0, 185.0) };
    let tile = rounded(m, m, 1024.0 - 2.0 * m, 1024.0 - 2.0 * m, r);
    let mut paint = Paint {
        anti_alias: true,
        shader: LinearGradient::new(
            Point::from_xy(m, m),
            Point::from_xy(1024.0 - m, 1024.0 - m),
            vec![
                GradientStop::new(0.0, rgb(0x1e, 0x90, 0xff)),
                GradientStop::new(1.0, rgb(0x5b, 0x4c, 0xe8)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )?,
        ..Paint::default()
    };
    pixmap.fill_path(&tile, &paint, FillRule::Winding, t, None);

    // The ink: a yellow swoosh ending where the pen touches the tile.
    let tip = (575.0, 640.0);
    let mut ink = PathBuilder::new();
    ink.move_to(215.0, 700.0);
    ink.cubic_to(320.0, 545.0, 420.0, 835.0, tip.0, tip.1);
    paint.shader = Shader::SolidColor(rgb(0xff, 0xd6, 0x0a));
    let width = if small { 120.0 } else { 84.0 };
    stroke(&mut pixmap, ink.finish(), &paint, width, t);

    // The pen, drawn along +x from its tip at the origin, then turned up and right.
    let pen = Transform::from_rotate(-45.0)
        .post_translate(tip.0, tip.1)
        .post_concat(t);
    let half = 62.0;
    let mut nib = PathBuilder::new();
    nib.move_to(4.0, -8.0);
    nib.line_to(110.0, -half);
    nib.line_to(110.0, half);
    nib.line_to(4.0, 8.0);
    nib.quad_to(-4.0, 0.0, 4.0, -8.0);
    nib.close();
    paint.shader = Shader::SolidColor(rgb(0x1c, 0x1c, 0x1e));
    pixmap.fill_path(&nib.finish()?, &paint, FillRule::Winding, pen, None);
    paint.shader = Shader::SolidColor(Color::WHITE);
    let barrel = rounded(104.0, -half, 340.0, 2.0 * half, half);
    // Square where it meets the nib: cover the rounded start with a plain block.
    let joint = rounded(104.0, -half, 80.0, 2.0 * half, 0.0);
    pixmap.fill_path(&barrel, &paint, FillRule::Winding, pen, None);
    pixmap.fill_path(&joint, &paint, FillRule::Winding, pen, None);
    if !small {
        // A band near the cap in the pen's ink color.
        paint.shader = Shader::SolidColor(rgb(0xff, 0x3b, 0x30));
        let band = rounded(330.0, -half, 44.0, 2.0 * half, 0.0);
        pixmap.fill_path(&band, &paint, FillRule::Winding, pen, None);
    }
    Some(pixmap)
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let r = r.min(w / 2.0).min(h / 2.0);
    // Cubic corners with the usual circle constant.
    let c = r * 0.5523;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.close();
    pb.finish().unwrap_or_else(|| {
        PathBuilder::from_rect(
            tiny_skia::Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)).expect("rect"),
        )
    })
}

fn stroke(
    pixmap: &mut Pixmap,
    path: Option<tiny_skia::Path>,
    paint: &Paint,
    width: f32,
    t: Transform,
) {
    let Some(path) = path else { return };
    let s = Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, paint, &s, t, None);
}

/// `draw` with a dot in the corner showing a state (drawing, mouse mode); None for none.
pub fn with_dot(size: u32, dot: Option<Color>) -> Option<Pixmap> {
    let mut pixmap = draw(size)?;
    if let Some(color) = dot {
        let s = size as f32;
        let (c, r) = (s * 0.78, s * 0.22);
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color(Color::WHITE);
        let ring = PathBuilder::from_circle(c, c, r + s * 0.05)?;
        pixmap.fill_path(
            &ring,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        paint.set_color(color);
        let dot = PathBuilder::from_circle(c, c, r)?;
        pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
    }
    Some(pixmap)
}
