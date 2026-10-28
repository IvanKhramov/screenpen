//! Model, geometry and rendering for screenpen. No OS dependencies.
//!
//! A port of ScreenInkCore (github.com/volyx/ScreenInk), with the author's permission.

pub mod annotation;
pub mod document;
pub mod geom;
pub mod icon;
pub mod pdf;
pub mod pressure;
pub mod recognize;
pub mod render;
pub mod shortcuts;
pub mod snap;
pub mod stroke;
pub mod style;
pub mod text;
pub mod text_edit;
pub mod tools;

pub use annotation::{Annotation, AnnotationId, AnnotationKind, BadgeFormat, Geometry, TextStyle};
pub use document::Document;
pub use geom::{Point, Rect};
pub use pressure::PressureFilter;
pub use render::ChannelOrder;
pub use shortcuts::{Action, CTRL, Chord, Key, Shortcuts};
pub use stroke::{BezierSegment, StrokeBuilder};
pub use style::{Blend, Rgba, Style};
pub use tools::ToolKind;
