use screenpen_core::render::{Fill, bounds, hit_test, render_parts};
use screenpen_core::shortcuts::{Action, Chord, Key, Shortcuts, next_width};
use screenpen_core::tools::{shape_geometry, snap_angle};
use screenpen_core::{
    Annotation, BadgeFormat, Geometry, Point, Rect, Rgba, Style, TextStyle, ToolKind,
};

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn annotation(geometry: Geometry) -> Annotation {
    Annotation::new(geometry, Style::new(Rgba::RED, 4.0))
}

#[test]
fn shift_snaps_lines_to_45_degrees() {
    let flat = snap_angle(p(0., 0.), p(100., 8.));
    assert!((flat.x - (100.0f32 * 100.0 + 64.0).sqrt()).abs() < 1e-3 && flat.y == 0.0);
    let diagonal = snap_angle(p(0., 0.), p(50., 60.));
    assert!((diagonal.x - diagonal.y).abs() < 1e-4);
}

#[test]
fn shift_makes_squares_toward_the_drag() {
    let g = shape_geometry(
        ToolKind::Rectangle,
        p(100., 100.),
        p(40., 130.),
        true,
        false,
        0.0,
    );
    assert_eq!(
        g,
        Some(Geometry::Rectangle {
            rect: Rect::new(40., 100., 60., 60.),
            corner_radius: 0.0
        })
    );
}

#[test]
fn tiny_drags_make_nothing() {
    assert_eq!(
        shape_geometry(ToolKind::Line, p(0., 0.), p(1., 1.), false, false, 0.0),
        None
    );
    assert_eq!(
        shape_geometry(ToolKind::Ellipse, p(0., 0.), p(2., 2.), false, false, 0.0),
        None
    );
    assert_eq!(
        shape_geometry(ToolKind::Pen, p(0., 0.), p(50., 50.), false, false, 0.0),
        None
    );
}

#[test]
fn curved_arrow_has_a_control_point() {
    let g = shape_geometry(ToolKind::Arrow, p(0., 0.), p(100., 0.), false, true, 0.0);
    let Some(Geometry::Arrow { control, .. }) = g else {
        panic!("expected an arrow")
    };
    assert_eq!(control, Some(p(50., -25.)));
}

#[test]
fn rectangle_outline_is_hollow() {
    let a = annotation(Geometry::Rectangle {
        rect: Rect::new(0., 0., 100., 60.),
        corner_radius: 0.0,
    });
    assert!(hit_test(&a, p(0., 30.), 0.0));
    assert!(!hit_test(&a, p(50., 30.), 4.0));
    let b = bounds(&a).unwrap();
    assert!(
        (b.x + 2.0).abs() < 0.01 && (b.width - 104.0).abs() < 0.01,
        "{b:?}"
    );
}

#[test]
fn arrow_head_points_at_the_tip() {
    let a = annotation(Geometry::Arrow {
        from: p(0., 0.),
        to: p(200., 0.),
        control: None,
    });
    assert_eq!(render_parts(&a).len(), 2);
    assert!(hit_test(&a, p(199., 0.), 0.0));
    assert!(bounds(&a).unwrap().right() <= 200.001);
}

#[test]
fn short_arrow_is_just_a_head() {
    let a = annotation(Geometry::Arrow {
        from: p(0., 0.),
        to: p(5., 0.),
        control: None,
    });
    assert_eq!(render_parts(&a).len(), 1);
}

#[test]
fn text_box_starts_at_origin() {
    let a = annotation(Geometry::Text {
        origin: p(10., 20.),
        string: "Hello\nWorld".into(),
        font_size: 20.0,
        style: TextStyle::default(),
    });
    let b = bounds(&a).expect("system fonts are needed for text");
    assert!(b.x >= 10.0 && b.x < 14.0, "{b:?}");
    assert!(b.y >= 20.0 && b.y < 30.0, "{b:?}");
    // Two lines: taller than one line height.
    assert!(b.height > screenpen_core::text::line_height(20.0), "{b:?}");
    assert!(hit_test(&a, b.center(), 6.0));
}

#[test]
fn badge_number_is_centered_in_contrast_color() {
    let a = annotation(Geometry::Badge {
        center: p(50., 50.),
        number: 7,
        format: BadgeFormat::Decimal,
        radius: 16.0,
    });
    let parts = render_parts(&a);
    assert_eq!(
        parts.iter().map(|p| p.fill).collect::<Vec<_>>(),
        [Fill::Style, Fill::Contrast]
    );
    let g = parts[1].path.bounds();
    assert!(((g.left() + g.right()) / 2.0 - 50.0).abs() < 0.01);
    assert!(((g.top() + g.bottom()) / 2.0 - 50.0).abs() < 0.01);
    assert_eq!(Rgba::YELLOW.contrasting(), Rgba::BLACK);
    assert_eq!(Rgba::BLUE.contrasting(), Rgba::WHITE);
}

#[test]
fn translation_moves_every_kind() {
    let geometries = [
        Geometry::Stroke {
            points: vec![p(0., 0.), p(10., 10.)],
            widths: vec![2., 2.],
        },
        Geometry::Line {
            from: p(0., 0.),
            to: p(10., 0.),
        },
        Geometry::Arrow {
            from: p(0., 0.),
            to: p(50., 0.),
            control: Some(p(25., 10.)),
        },
        Geometry::Rectangle {
            rect: Rect::new(0., 0., 10., 10.),
            corner_radius: 2.0,
        },
        Geometry::Ellipse {
            rect: Rect::new(0., 0., 10., 10.),
        },
        Geometry::Text {
            origin: p(0., 0.),
            string: "Hi".into(),
            font_size: 14.0,
            style: TextStyle::default(),
        },
        Geometry::Badge {
            center: p(0., 0.),
            number: 1,
            format: BadgeFormat::Decimal,
            radius: 10.0,
        },
    ];
    for g in geometries {
        let before = bounds(&annotation(g.clone())).unwrap();
        let after = bounds(&annotation(g.translated(30.0, -5.0))).unwrap();
        assert!((after.x - before.x - 30.0).abs() < 0.05, "{:?}", g.kind());
        assert!((after.y - before.y + 5.0).abs() < 0.05, "{:?}", g.kind());
    }
}

#[test]
fn badge_labels() {
    for (n, s) in [(1, "A"), (26, "Z"), (27, "AA"), (52, "AZ"), (53, "BA")] {
        assert_eq!(BadgeFormat::Alphabet.label(n), s);
    }
    for (n, s) in [
        (1, "I"),
        (4, "IV"),
        (9, "IX"),
        (14, "XIV"),
        (40, "XL"),
        (1994, "MCMXCIV"),
    ] {
        assert_eq!(BadgeFormat::Roman.label(n), s);
    }
}

#[test]
fn default_shortcuts_are_unique_and_cover_every_tool() {
    let s = Shortcuts::default();
    for tool in ToolKind::ALL {
        assert!(s.chord(Action::Tool(tool)).is_some(), "{tool:?} has no key");
    }
    assert_eq!(
        s.local_action(Chord::key(Key::Char('P'))),
        Some(Action::Tool(ToolKind::Pen))
    );
    assert_eq!(
        s.local_action(Chord::key(Key::Char('9'))),
        Some(Action::Color(8))
    );
    assert!(s.conflicts().is_empty(), "{:?}", s.conflicts());
}

#[test]
fn chords_round_trip_through_text() {
    for text in [
        "Ctrl+Alt+Shift+A",
        "P",
        "[",
        "Ctrl+]",
        "Shift+/",
        "Delete",
        "Esc",
        "F9",
        "Ctrl++",
    ] {
        let chord: Chord = text.parse().unwrap();
        assert_eq!(chord.to_string(), text);
    }
    assert_eq!(
        "ctrl+shift+z".parse::<Chord>().unwrap(),
        Chord::ctrl_shift(Key::Char('Z'))
    );
    assert!("Hyper+A".parse::<Chord>().is_err());
}

#[test]
fn action_names_round_trip() {
    for b in Shortcuts::default().bindings {
        assert_eq!(Action::from_name(&b.action.name()), Some(b.action));
    }
}

#[test]
fn turning_a_shortcut_off_and_conflicts() {
    let mut s = Shortcuts::default();
    s.set(Action::Tool(ToolKind::Pen), None);
    assert_eq!(s.local_action(Chord::key(Key::Char('P'))), None);
    s.set(Action::Clear, Some(Chord::key(Key::Char('H'))));
    assert_eq!(
        s.conflicts(),
        [(Action::Tool(ToolKind::Highlighter), Action::Clear)]
    );
}

#[test]
fn width_steps_clamp_at_ends() {
    assert_eq!(next_width(4.0, true), 6.0);
    assert_eq!(next_width(5.0, false), 4.0);
    assert_eq!(next_width(32.0, true), 32.0);
    assert_eq!(next_width(1.0, false), 1.0);
}

#[test]
fn styled_text_has_plate_and_outline_and_reads_old_files() {
    let style = TextStyle {
        bold: true,
        background: true,
        outline: true,
        ..TextStyle::default()
    };
    let a = annotation(Geometry::Text {
        origin: p(10., 20.),
        string: "Hi".into(),
        font_size: 30.0,
        style,
    });
    let fills: Vec<_> = render_parts(&a).iter().map(|p| p.fill).collect();
    assert_eq!(fills, [Fill::Plate, Fill::Contrast, Fill::Style]);
    // Text saved before styles existed has no `style` field.
    let old = r#"{"kind":"text","origin":{"x":1.0,"y":2.0},"string":"x","font_size":12.0}"#;
    let g: Geometry = serde_json::from_str(old).unwrap();
    assert!(matches!(g, Geometry::Text { style, .. } if style == TextStyle::default()));
}
