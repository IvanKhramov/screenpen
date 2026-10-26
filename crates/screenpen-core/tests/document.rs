use std::collections::HashSet;

use screenpen_core::render::bounds;
use screenpen_core::{Annotation, BadgeFormat, Document, Geometry, Point, Style};

fn stroke(x: f32) -> Annotation {
    Annotation::new(
        Geometry::Stroke {
            points: vec![Point::new(x, 0.0), Point::new(x + 10.0, 5.0)],
            widths: vec![2.0, 3.0],
        },
        Style::DEFAULT_PEN,
    )
}

fn badge(number: u32) -> Annotation {
    Annotation::new(
        Geometry::Badge {
            center: Point::default(),
            number,
            format: BadgeFormat::Decimal,
            radius: 10.0,
        },
        Style::DEFAULT_PEN,
    )
}

fn snapshot(d: &Document) -> Vec<Annotation> {
    d.annotations().iter().map(|a| (**a).clone()).collect()
}

fn ids(items: &[&Annotation]) -> HashSet<u64> {
    items.iter().map(|a| a.id).collect()
}

#[test]
fn insert_undo_redo() {
    let mut d = Document::new();
    let (a, b) = (stroke(0.0), stroke(20.0));
    d.insert(a.clone());
    d.insert(b.clone());
    assert_eq!(snapshot(&d), [a.clone(), b.clone()]);
    d.undo();
    assert_eq!(snapshot(&d), std::slice::from_ref(&a));
    d.undo();
    assert!(snapshot(&d).is_empty());
    d.redo();
    d.redo();
    assert_eq!(snapshot(&d), [a, b]);
}

#[test]
fn clear_is_undoable() {
    let mut d = Document::new();
    let a = stroke(0.0);
    d.insert(a.clone());
    d.remove_all();
    assert!(snapshot(&d).is_empty());
    d.undo();
    assert_eq!(snapshot(&d), [a]);
}

#[test]
fn remove_keeps_order_of_the_rest() {
    let mut d = Document::new();
    let items: Vec<_> = (0..4).map(|i| stroke(i as f32 * 10.0)).collect();
    items.iter().for_each(|a| d.insert(a.clone()));
    d.remove(&ids(&[&items[1]]));
    assert_eq!(
        snapshot(&d),
        [items[0].clone(), items[2].clone(), items[3].clone()]
    );
    d.undo();
    assert_eq!(snapshot(&d), items);
}

/// Deterministic SplitMix64 so the property test is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Apply N random operations, undo N, end at the start.
#[test]
fn random_operations_undo_to_start() {
    for seed in 0..20 {
        let mut rng = Rng(seed);
        let mut d = Document::new();
        let mut history = vec![snapshot(&d)];
        for step in 0..25 {
            let len = d.annotations().len();
            match rng.next() % 5 {
                0 if len > 0 => d.remove_all(),
                1 if len > 0 => {
                    let id = d.annotations()[(rng.next() % len as u64) as usize].id;
                    d.remove(&HashSet::from([id]));
                }
                _ => d.insert(stroke(step as f32)),
            }
            history.push(snapshot(&d));
        }
        for expected in history.iter().rev().skip(1) {
            d.undo();
            assert_eq!(&snapshot(&d), expected, "seed {seed}");
        }
        assert!(!d.can_undo());
    }
}

#[test]
fn annotation_round_trips_through_json() {
    let original = stroke(3.0);
    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(serde_json::from_str::<Annotation>(&json).unwrap(), original);
}

#[test]
fn badge_numbers_follow_the_highest_and_undo_gives_them_back() {
    let mut d = Document::new();
    assert_eq!(d.next_badge_number(), 1);
    d.insert(badge(d.next_badge_number()));
    d.insert(badge(d.next_badge_number()));
    assert_eq!(d.next_badge_number(), 3);
    d.undo();
    assert_eq!(d.next_badge_number(), 2);
}

#[test]
fn update_is_one_undo_step() {
    let mut d = Document::new();
    let (a, b) = (badge(1), badge(2));
    d.insert(a.clone());
    d.insert(b.clone());
    d.update(&ids(&[&a, &b]), |x| {
        x.geometry = x.geometry.translated(5.0, 5.0)
    });
    for x in d.annotations() {
        assert!((bounds(x).unwrap().center().x - 5.0).abs() < 0.01);
    }
    d.undo();
    assert_eq!(snapshot(&d), [a, b]);
}

#[test]
fn bring_to_front_keeps_relative_order() {
    let mut d = Document::new();
    let items: Vec<_> = (1..=4).map(badge).collect();
    items.iter().for_each(|a| d.insert(a.clone()));
    d.bring_to_front(&ids(&[&items[0], &items[2]]));
    let order: Vec<_> = d.annotations().iter().map(|a| a.id).collect();
    assert_eq!(order, [items[1].id, items[3].id, items[0].id, items[2].id]);
}
