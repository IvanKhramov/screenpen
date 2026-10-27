//! Shows what the magnet sees on a screenshot: every edge in pink and the boxes found
//! around a grid of points in blue, so detection changes can be checked by eye.
//! Usage: cargo run --release -p screenpen-core --example snap -- <in.png> <out.png> [scale]

use std::time::Instant;

use screenpen_core::Point;
use screenpen_core::snap::SnapMap;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output, rest @ ..] = args.as_slice() else {
        eprintln!("usage: snap <in.png> <out.png> [scale]");
        std::process::exit(2);
    };
    let scale: f32 = rest.first().map_or(1.0, |s| s.parse().expect("scale"));
    let mut image = Pixmap::load_png(input).expect("read png");
    let started = Instant::now();
    let map = SnapMap::detect(image.data(), image.width(), image.height(), scale);
    println!("detect: {:?}", started.elapsed());

    let t = Transform::from_scale(scale, scale);
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    let (horizontal, vertical) = map.edges();
    println!(
        "edges: {} horizontal, {} vertical",
        horizontal.len(),
        vertical.len()
    );
    if std::env::var_os("SNAP_DEBUG").is_some() {
        println!("{vertical:?}");
    }
    let mut pb = PathBuilder::new();
    for e in horizontal {
        pb.move_to(e.from, e.pos);
        pb.line_to(e.to, e.pos);
    }
    for e in vertical {
        pb.move_to(e.pos, e.from);
        pb.line_to(e.pos, e.to);
    }
    if let Some(path) = pb.finish() {
        paint.set_color(Color::from_rgba8(255, 46, 158, 255));
        image.stroke_path(&path, &paint, &Stroke::default(), t, None);
    }

    let (w, h) = (image.width() as f32 / scale, image.height() as f32 / scale);
    let dashed = Stroke {
        width: 2.0,
        dash: StrokeDash::new(vec![6.0, 4.0], 0.0),
        ..Stroke::default()
    };
    let mut boxes = 0;
    let (mut y, step) = (60.0, 160.0);
    while y < h {
        let mut x = 80.0;
        while x < w {
            let p = Point::new(x, y);
            paint.set_color(Color::from_rgba8(10, 132, 255, 255));
            if let Some(dot) = PathBuilder::from_circle(x, y, 4.0) {
                image.fill_path(&dot, &paint, tiny_skia::FillRule::Winding, t, None);
            }
            if let Some(b) = map.box_at(p).and_then(|b| b.to_skia()) {
                boxes += 1;
                image.stroke_path(&PathBuilder::from_rect(b), &paint, &dashed, t, None);
            }
            x += step;
        }
        y += step;
    }
    println!("boxes: {boxes}");
    image.save_png(output).expect("write png");
}
