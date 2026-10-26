use screenpen_core::render::stroke_outline;
use screenpen_core::{BezierSegment, Point, PressureFilter, StrokeBuilder};

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

#[test]
fn emits_first_segment_on_fifth_point() {
    let mut builder = StrokeBuilder::new();
    for q in [p(0., 0.), p(10., 0.), p(20., 10.), p(30., 10.)] {
        assert_eq!(builder.add(q, 2.0), None);
    }
    // p3 becomes the midpoint of p2 and p4.
    assert_eq!(
        builder.add(p(40., 20.), 3.0),
        Some(BezierSegment {
            start: p(0., 0.),
            control1: p(10., 0.),
            control2: p(20., 10.),
            end: p(30., 15.),
            width: 3.0
        })
    );
}

#[test]
fn consecutive_segments_share_joint_and_tangent() {
    let points: Vec<Point> = (0..20)
        .map(|i| p(i as f32 * 7.0, (i as f32 / 2.0).sin() * 30.0))
        .collect();
    let segments = StrokeBuilder::segments(&points, &vec![2.0; points.len()]);
    assert!(segments.len() >= 3);
    for w in segments.windows(2) {
        let (a, b) = (w[0], w[1]);
        assert_eq!(a.end, b.start);
        // C1 continuity: incoming and outgoing control points are collinear with the joint.
        let (ix, iy) = (a.end.x - a.control2.x, a.end.y - a.control2.y);
        let (ox, oy) = (b.control1.x - b.start.x, b.control1.y - b.start.y);
        assert!((ix * oy - iy * ox).abs() < 1e-3);
    }
}

#[test]
fn covers_whole_stroke_from_first_to_last_point() {
    for count in 2..=12 {
        let points: Vec<Point> = (0..count)
            .map(|i| p(i as f32 * 5.0, (i % 3) as f32 * 4.0))
            .collect();
        let segments = StrokeBuilder::segments(&points, &[1.0]);
        assert_eq!(
            segments.first().map(|s| s.start),
            points.first().copied(),
            "count {count}"
        );
        assert_eq!(
            segments.last().map(|s| s.end),
            points.last().copied(),
            "count {count}"
        );
        for w in segments.windows(2) {
            assert_eq!(w[0].end, w[1].start, "count {count}");
        }
    }
}

#[test]
fn ignores_repeated_samples() {
    let mut builder = StrokeBuilder::new();
    builder.add(p(1., 1.), 1.0);
    builder.add(p(1., 1.), 1.0);
    assert_eq!(builder.points().len(), 1);
}

#[test]
fn replay_matches_live_segments() {
    let points: Vec<Point> = (0..17)
        .map(|i| p(i as f32 * 3.0, (i * i) as f32 / 10.0))
        .collect();
    let widths: Vec<f32> = (0..17).map(|i| 1.0 + (i % 4) as f32).collect();
    let mut builder = StrokeBuilder::new();
    let mut live = Vec::new();
    for (&q, &w) in points.iter().zip(&widths) {
        live.extend(builder.add(q, w));
    }
    live.extend(builder.finish());
    assert_eq!(
        live,
        StrokeBuilder::segments(builder.points(), builder.widths())
    );
}

#[test]
fn single_point_is_a_dot() {
    let path = stroke_outline(&[p(50., 50.)], &[10.0]).unwrap();
    let b = path.bounds();
    assert!((b.left() - 45.0).abs() < 0.01 && (b.width() - 10.0).abs() < 0.01);
}

#[test]
fn width_controls_thickness() {
    let points = [p(0., 0.), p(100., 0.)];
    let thin = stroke_outline(&points, &[2.0, 2.0]).unwrap().bounds();
    let thick = stroke_outline(&points, &[20.0, 20.0]).unwrap().bounds();
    assert!((thin.height() - 2.0).abs() < 0.05, "{}", thin.height());
    assert!((thick.height() - 20.0).abs() < 0.05, "{}", thick.height());
}

#[test]
fn pressure_first_sample_is_taken_as_is() {
    assert_eq!(PressureFilter::default().filter(0.8), 0.8);
}

#[test]
fn pressure_smooths_toward_new_values() {
    let mut f = PressureFilter::new(0.5, 0.3);
    f.filter(0.0);
    assert_eq!(f.filter(1.0), 0.5);
    assert_eq!(f.filter(1.0), 0.75);
}

#[test]
fn pressure_clamps_and_maps_to_width() {
    let mut f = PressureFilter::new(0.3, 0.25);
    assert_eq!(f.filter(3.0), 1.0);
    assert_eq!(f.width(8.0, 0.0), 2.0);
    assert_eq!(f.width(8.0, 1.0), 8.0);
}
