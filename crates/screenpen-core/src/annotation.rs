use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rect};
use crate::style::Style;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BadgeFormat {
    #[default]
    Decimal,
    Alphabet,
    Roman,
}

impl BadgeFormat {
    pub fn label(self, number: u32) -> String {
        let n = number.max(1);
        match self {
            BadgeFormat::Decimal => n.to_string(),
            BadgeFormat::Alphabet => {
                // 1 -> A, 26 -> Z, 27 -> AA (bijective base 26).
                let mut value = n;
                let mut result = Vec::new();
                while value > 0 {
                    value -= 1;
                    result.push(b'A' + (value % 26) as u8);
                    value /= 26;
                }
                result.reverse();
                String::from_utf8(result).unwrap_or_default()
            }
            BadgeFormat::Roman => {
                const TABLE: [(u32, &str); 13] = [
                    (1000, "M"),
                    (900, "CM"),
                    (500, "D"),
                    (400, "CD"),
                    (100, "C"),
                    (90, "XC"),
                    (50, "L"),
                    (40, "XL"),
                    (10, "X"),
                    (9, "IX"),
                    (5, "V"),
                    (4, "IV"),
                    (1, "I"),
                ];
                let mut value = n.min(3999);
                let mut result = String::new();
                for (amount, symbol) in TABLE {
                    while value >= amount {
                        result.push_str(symbol);
                        value -= amount;
                    }
                }
                result
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
    Stroke,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Badge,
}

/// How a text annotation looks beyond its color and size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    /// A system font family; None is the default sans-serif.
    pub family: Option<String>,
    pub bold: bool,
    pub italic: bool,
    /// A translucent plate behind the text, for busy backgrounds.
    pub background: bool,
    /// A contrasting outline around the glyphs.
    pub outline: bool,
}

/// Vector geometry of one annotation, in the owning screen's top-left point space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Geometry {
    /// Freehand pen or highlighter stroke: raw input points and the width at each.
    Stroke {
        points: Vec<Point>,
        widths: Vec<f32>,
    },
    Line {
        from: Point,
        to: Point,
    },
    /// `control` makes a curved arrow (quadratic Bezier through it).
    Arrow {
        from: Point,
        to: Point,
        control: Option<Point>,
    },
    Rectangle {
        rect: Rect,
        corner_radius: f32,
    },
    Ellipse {
        rect: Rect,
    },
    /// `origin` is the top-left corner of the first line's box.
    Text {
        origin: Point,
        string: String,
        font_size: f32,
        /// Missing in files written before text styles existed.
        #[serde(default)]
        style: TextStyle,
    },
    Badge {
        center: Point,
        number: u32,
        format: BadgeFormat,
        radius: f32,
    },
}

impl Geometry {
    pub fn kind(&self) -> AnnotationKind {
        match self {
            Geometry::Stroke { .. } => AnnotationKind::Stroke,
            Geometry::Line { .. } => AnnotationKind::Line,
            Geometry::Arrow { .. } => AnnotationKind::Arrow,
            Geometry::Rectangle { .. } => AnnotationKind::Rectangle,
            Geometry::Ellipse { .. } => AnnotationKind::Ellipse,
            Geometry::Text { .. } => AnnotationKind::Text,
            Geometry::Badge { .. } => AnnotationKind::Badge,
        }
    }

    pub fn translated(&self, dx: f32, dy: f32) -> Geometry {
        let mv = |p: &Point| p.offset(dx, dy);
        match self {
            Geometry::Stroke { points, widths } => Geometry::Stroke {
                points: points.iter().map(mv).collect(),
                widths: widths.clone(),
            },
            Geometry::Line { from, to } => Geometry::Line {
                from: mv(from),
                to: mv(to),
            },
            Geometry::Arrow { from, to, control } => Geometry::Arrow {
                from: mv(from),
                to: mv(to),
                control: control.as_ref().map(mv),
            },
            Geometry::Rectangle {
                rect,
                corner_radius,
            } => Geometry::Rectangle {
                rect: rect.offset(dx, dy),
                corner_radius: *corner_radius,
            },
            Geometry::Ellipse { rect } => Geometry::Ellipse {
                rect: rect.offset(dx, dy),
            },
            Geometry::Text {
                origin,
                string,
                font_size,
                style,
            } => Geometry::Text {
                origin: mv(origin),
                string: string.clone(),
                font_size: *font_size,
                style: style.clone(),
            },
            Geometry::Badge {
                center,
                number,
                format,
                radius,
            } => Geometry::Badge {
                center: mv(center),
                number: *number,
                format: *format,
                radius: *radius,
            },
        }
    }
}

/// Unique within a process run; restored ids from disk keep their values.
pub type AnnotationId = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> AnnotationId {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Seconds since the Unix epoch.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotationId,
    pub geometry: Geometry,
    pub style: Style,
    /// Seconds since the Unix epoch; drives auto-fade.
    pub created_at: f64,
}

impl Annotation {
    pub fn new(geometry: Geometry, style: Style) -> Annotation {
        Annotation {
            id: next_id(),
            geometry,
            style,
            created_at: now(),
        }
    }

    pub fn kind(&self) -> AnnotationKind {
        self.geometry.kind()
    }
}
