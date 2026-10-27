//! Renders every annotation kind into a PNG so rendering changes can be checked by eye.
//! Usage: cargo run -p screenpen-core --example selftest -- <out-dir>
//! Exits non-zero if a kind renders nothing.

use screenpen_core::render::{ChannelOrder, draw};
use screenpen_core::{
    Annotation, BadgeFormat, Blend, Geometry, Point, Rect, Rgba, Style, TextStyle,
};
use tiny_skia::{Color, Pixmap, Transform};

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/selftest".into());
    std::fs::create_dir_all(&out).expect("create output dir");

    let wave: Vec<Point> = (0..60)
        .map(|i| p(30.0 + i as f32 * 5.0, 60.0 + (i as f32 / 5.0).sin() * 25.0))
        .collect();
    let widths: Vec<f32> = (0..60).map(|i| 2.0 + 6.0 * (i as f32 / 60.0)).collect();
    let mut highlighter = Style::new(Rgba::YELLOW, 18.0);
    highlighter.opacity = 0.4;
    highlighter.blend = Blend::Multiply;
    let zigzag: Vec<Point> = (0..30)
        .map(|i| {
            p(
                360.0 + (i % 2) as f32 * 6.0 + i as f32,
                40.0 + (i % 3) as f32 * 5.0,
            )
        })
        .collect();

    let items = [
        (
            "stroke",
            Geometry::Stroke {
                points: wave.clone(),
                widths,
            },
            Style::new(Rgba::RED, 4.0),
        ),
        (
            "highlighter",
            Geometry::Stroke {
                points: wave.iter().map(|q| q.offset(0.0, 20.0)).collect(),
                widths: vec![18.0],
            },
            highlighter,
        ),
        (
            "zigzag",
            Geometry::Stroke {
                points: zigzag,
                widths: vec![8.0; 30],
            },
            Style::new(Rgba::PURPLE, 8.0),
        ),
        (
            "line",
            Geometry::Line {
                from: p(30., 150.),
                to: p(330., 170.),
            },
            Style::new(Rgba::GREEN, 4.0),
        ),
        (
            "arrow",
            Geometry::Arrow {
                from: p(30., 200.),
                to: p(330., 200.),
                control: None,
            },
            Style::new(Rgba::BLUE, 4.0),
        ),
        (
            "curved arrow",
            Geometry::Arrow {
                from: p(30., 260.),
                to: p(330., 260.),
                control: Some(p(180., 210.)),
            },
            Style::new(Rgba::ORANGE, 6.0),
        ),
        (
            "rectangle",
            Geometry::Rectangle {
                rect: Rect::new(360., 120., 160., 90.),
                corner_radius: 12.0,
            },
            Style::new(Rgba::RED, 4.0),
        ),
        (
            "ellipse",
            Geometry::Ellipse {
                rect: Rect::new(360., 230., 160., 90.),
            },
            Style::new(Rgba::PINK, 4.0),
        ),
        (
            "text",
            Geometry::Text {
                origin: p(30., 300.),
                string: "Привет, screenpen!\nSecond line".into(),
                font_size: 24.0,
                style: TextStyle::default(),
            },
            Style::new(Rgba::WHITE, 4.0),
        ),
        (
            "styled text",
            Geometry::Text {
                origin: p(360., 330.),
                string: "Bold italic".into(),
                font_size: 22.0,
                style: TextStyle {
                    bold: true,
                    italic: true,
                    background: true,
                    outline: true,
                    family: None,
                },
            },
            Style::new(Rgba::YELLOW, 4.0),
        ),
        (
            "badge",
            Geometry::Badge {
                center: p(570., 60.),
                number: 7,
                format: BadgeFormat::Decimal,
                radius: 18.0,
            },
            Style::new(Rgba::RED, 4.0),
        ),
        (
            "pixelate",
            Geometry::Pixelate {
                rect: Rect::new(520., 300., 60., 40.),
                cols: 3,
                rows: 2,
                colors: vec![
                    [200, 40, 40],
                    [40, 200, 40],
                    [40, 40, 200],
                    [240; 3],
                    [90; 3],
                    [20; 3],
                ],
            },
            Style::new(Rgba::RED, 4.0),
        ),
        (
            "badge roman",
            Geometry::Badge {
                center: p(570., 120.),
                number: 14,
                format: BadgeFormat::Roman,
                radius: 18.0,
            },
            Style::new(Rgba::YELLOW, 4.0),
        ),
        (
            "badge alpha",
            Geometry::Badge {
                center: p(570., 180.),
                number: 27,
                format: BadgeFormat::Alphabet,
                radius: 18.0,
            },
            Style::new(Rgba::WHITE, 4.0),
        ),
    ];

    let scale = 2.0;
    let mut pixmap = Pixmap::new((620.0 * scale) as u32, (380.0 * scale) as u32).unwrap();
    pixmap.fill(Color::from_rgba8(0x3a, 0x3a, 0x3c, 255));
    let mut failed = false;
    for (name, geometry, style) in items {
        let a = Annotation::new(geometry, style);
        let mut single = Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
        draw(
            &mut single.as_mut(),
            &a,
            Transform::from_scale(scale, scale),
            ChannelOrder::Rgba,
        );
        if single.pixels().iter().all(|px| px.alpha() == 0) {
            eprintln!("FAIL: {name} rendered nothing");
            failed = true;
        }
        draw(
            &mut pixmap.as_mut(),
            &a,
            Transform::from_scale(scale, scale),
            ChannelOrder::Rgba,
        );
    }
    let file = format!("{out}/annotations.png");
    pixmap.save_png(&file).expect("write png");
    println!("wrote {file}");
    if failed {
        std::process::exit(1);
    }
}
