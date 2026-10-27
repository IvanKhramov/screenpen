//! The drawing surface for one screen, independent of the OS.
//!
//! Layers, bottom to top: a cached bitmap of the background and committed annotations,
//! the live stroke (its own bitmap, so the highlighter's opacity applies to the stroke
//! as a whole), then transient overlays: shape preview, moved selection, selection
//! boxes, marquee, eraser ring and the text being typed.
//!
//! Input arrives in pixels; the model is in points (pixels / scale). Each handler
//! returns what to repaint, and `paint` updates only that part of the target.

use std::collections::HashSet;
use std::f32::consts::SQRT_2;

use screenpen_core::annotation::{AnnotationId, now};
use screenpen_core::render::{
    ChannelOrder, bounds, draw, hit_test, rounded_rect, segment_outline, skia_color,
};
use screenpen_core::shortcuts::next_text_size;
use screenpen_core::snap::{Edge, SnapMap, Snapped};
use screenpen_core::text_edit::{Motion, TextBuffer};
use screenpen_core::tools::shape_geometry;
use screenpen_core::{
    Action, Annotation, AnnotationKind, BadgeFormat, Chord, Document, Geometry, Key, Point,
    PressureFilter, Rect, Rgba, StrokeBuilder, Style, TextStyle, ToolKind,
};
use tiny_skia::{
    FillRule, IntRect, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapMut, PixmapPaint,
    Stroke, StrokeDash, Transform,
};

use crate::state::ToolState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// One pointer sample, in pixels relative to the screen's top-left corner.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    /// 0..=1; a mouse reports 1 while the button is down.
    pub pressure: f32,
    pub mods: Mods,
    pub double_click: bool,
}

/// What a handler wants repainted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Repaint {
    None,
    /// A rectangle in pixels.
    Rect(Rect),
    Full,
}

impl Repaint {
    pub fn union(self, other: Repaint) -> Repaint {
        match (self, other) {
            (Repaint::Full, _) | (_, Repaint::Full) => Repaint::Full,
            (Repaint::None, x) | (x, Repaint::None) => x,
            (Repaint::Rect(a), Repaint::Rect(b)) => Repaint::Rect(a.union(b)),
        }
    }
}

/// Something only the app can do (it spans screens or windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    StopDrawing,
    Ghost,
    ToggleWhiteboard,
    Help,
    ToggleSpotlight,
    ToggleFreeze,
    ToggleMagnifier,
    Snapshot,
    /// Tool, color or width changed: every screen and the palette should refresh.
    StateChanged,
}

/// How long the laser trail takes to fade after the button is released.
const LASER_FADE: f64 = 0.8;
const LASER: Rgba = Rgba::rgb(1.0, 0.18, 0.18);
/// The trail keeps only this many latest points.
const LASER_POINTS: usize = 600;
/// Spotlight radius in points, and its limits for the wheel.
const SPOTLIGHT_RADIUS: f32 = 140.0;
const SPOTLIGHT_LIMITS: (f32, f32) = (40.0, 600.0);
/// Room around a lit box, and its corner radius, in points.
const FOCUS_PAD: f32 = 6.0;
const FOCUS_RADIUS: f32 = 10.0;

#[derive(Default)]
struct Laser {
    trail: Vec<Point>,
    /// Where the dot is (the pointer, while the laser tool is active).
    at: Option<Point>,
    /// When the button went up; the trail fades from then.
    released: Option<f64>,
}

enum Gesture {
    Idle,
    Freehand {
        builder: StrokeBuilder,
        style: Style,
        filter: PressureFilter,
        pressure: bool,
        last_width: f32,
    },
    Shape {
        tool: ToolKind,
        /// Where the shape starts: `pressed`, or the edges the magnet moved it to.
        start: Point,
        pressed: Point,
        start_edges: (Option<Edge>, Option<Edge>),
        preview: Option<Annotation>,
    },
    Erase {
        ids: HashSet<AnnotationId>,
        radius: f32,
    },
    Move {
        start: Point,
        offset: (f32, f32),
    },
    /// Drawing with the laser: points go to `Canvas::laser`.
    Laser,
    Marquee {
        start: Point,
        initial: HashSet<AnnotationId>,
        rect: Rect,
    },
    /// Dragging inside the text being edited selects.
    TextSelect,
}

/// What a key did to the text being edited.
#[derive(Debug, PartialEq)]
pub enum TextKey {
    Handled(Repaint),
    /// Put this on the clipboard.
    Copy(String),
    /// Read the clipboard and pass it to `Canvas::paste`.
    Paste,
}

struct TextEdit {
    origin: Point,
    buffer: TextBuffer,
    /// Earlier states for Ctrl+Z, and undone ones for Ctrl+Y.
    undo: Vec<TextBuffer>,
    redo: Vec<TextBuffer>,
    /// The last change was typed chars: the next one joins its undo step.
    typing: bool,
    font_size: f32,
    style: TextStyle,
    color: Rgba,
    /// The annotation being edited, hidden from the cache meanwhile.
    editing: Option<AnnotationId>,
}

/// The shortcuts that still work while typing: text size and style with Ctrl or Alt
/// (letters without them are text).
pub fn typing_action(
    shortcuts: &screenpen_core::Shortcuts,
    chord: screenpen_core::Chord,
) -> Option<Action> {
    if !chord.ctrl && !chord.alt {
        return None;
    }
    shortcuts.local_action(chord).filter(|a| {
        matches!(
            a,
            Action::TextSmaller | Action::TextBigger | Action::Bold | Action::Italic
        )
    })
}

/// Auto-fade: how long the fade-out itself takes.
const FADE_DURATION: f64 = 0.4;
const ACCENT: Rgba = Rgba::rgb(0.04, 0.52, 1.0);
/// The magnet's guides: cyan, unlike any ink color.
const GUIDE: Rgba = Rgba::rgb(0.0, 0.85, 0.9);
/// How far the magnet reaches for an edge, in points.
const MAGNET_RADIUS: f32 = 10.0;
/// How much of an edge a guide shows each side of the pointer, in points.
const GUIDE_REACH: f32 = 120.0;

/// Ctrl turns the magnet on for shapes, the pen's Ctrl shapes included.
fn magnet(state: &ToolState, mods: Mods) -> bool {
    mods.ctrl
        && matches!(
            state.tool,
            ToolKind::Pen
                | ToolKind::Line
                | ToolKind::Arrow
                | ToolKind::Rectangle
                | ToolKind::Ellipse
        )
}

/// The parts of `edges` (vertical, horizontal) near `p`.
fn guide_lines(p: Point, (x, y): (Option<Edge>, Option<Edge>)) -> Vec<(Point, Point)> {
    let near = |e: Edge, along: f32| {
        (
            e.from.max(along - GUIDE_REACH),
            e.to.min(along + GUIDE_REACH),
        )
    };
    let mut lines = Vec::new();
    if let Some(e) = x {
        let (a, b) = near(e, p.y);
        lines.push((Point::new(e.pos, a), Point::new(e.pos, b)));
    }
    if let Some(e) = y {
        let (a, b) = near(e, p.x);
        lines.push((Point::new(a, e.pos), Point::new(b, e.pos)));
    }
    lines
}

pub struct Canvas {
    pub doc: Document,
    scale: f32,
    order: ChannelOrder,
    /// Background and committed annotations; 1x1 while asleep.
    cache: Pixmap,
    width: u32,
    height: u32,
    /// Hidden: the full-size buffers are freed until `wake`.
    asleep: bool,
    /// The stroke in progress; allocated only during a freehand gesture.
    live: Option<Pixmap>,
    live_color: Rgba,
    live_opacity: f32,
    /// Pixel bounds of everything drawn in `live` since the gesture began.
    live_bounds: Option<Rect>,
    background: Rgba,
    gesture: Gesture,
    pub selection: HashSet<AnnotationId>,
    text: Option<TextEdit>,
    eraser_at: Option<(Point, f32)>,
    /// Pixel bounds of the overlays as last painted, so they can be erased.
    painted_overlay: Option<Rect>,
    /// Annotations that fade out (id, fade start time).
    fade_after: Option<f32>,
    laser: Laser,
    /// A frozen screen image (same size and byte order as the cache) under the ink.
    backdrop: Option<Pixmap>,
    /// Center (None until the pointer is seen) and radius, while the spotlight is on.
    spotlight: Option<(Option<Point>, f32)>,
    /// A box the spotlight lights instead of the circle (Ctrl+click on a panel).
    focus: Option<Rect>,
    /// Edges of the screen under the ink, for the magnet; None until first needed.
    snap: Option<SnapMap>,
    /// The box a Ctrl+click would frame, under the pointer.
    snap_hint: Option<Rect>,
    /// Pieces of the edges the magnet holds the pointer to.
    guides: Vec<(Point, Point)>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, scale: f32, order: ChannelOrder) -> Option<Canvas> {
        Some(Canvas {
            doc: Document::new(),
            scale,
            order,
            cache: Pixmap::new(width, height)?,
            width,
            height,
            asleep: false,
            live: None,
            live_color: Rgba::RED,
            live_opacity: 1.0,
            live_bounds: None,
            background: Rgba::BLACK.with_alpha(1.0 / 255.0),
            gesture: Gesture::Idle,
            selection: HashSet::new(),
            text: None,
            eraser_at: None,
            painted_overlay: None,
            fade_after: None,
            laser: Laser::default(),
            backdrop: None,
            spotlight: None,
            focus: None,
            snap: None,
            snap_hint: None,
            guides: Vec::new(),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Frees the full-size buffers while drawing is off; the annotations stay.
    pub fn sleep(&mut self) {
        self.cancel();
        if let Some(tiny) = Pixmap::new(1, 1) {
            self.cache = tiny;
        }
        self.live = None;
        self.asleep = true;
    }

    /// Allocates the buffers again and redraws the annotations into them.
    /// Returns false when the memory isn't there.
    pub fn wake(&mut self) -> bool {
        if self.asleep {
            let Some(cache) = Pixmap::new(self.width, self.height) else {
                return false;
            };
            self.cache = cache;
            self.asleep = false;
            self.rebuild_cache();
        }
        true
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    fn to_points(&self, p: &Pointer) -> Point {
        Point::new(p.x / self.scale, p.y / self.scale)
    }

    fn to_pixels(&self, r: Rect) -> Rect {
        Rect::new(
            r.x * self.scale,
            r.y * self.scale,
            r.width * self.scale,
            r.height * self.scale,
        )
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }

    /// The background under the ink: almost-transparent black keeps the window
    /// hit-testable, a dim level darkens the screen, the whiteboard hides it.
    pub fn set_background(&mut self, background: Rgba) -> Repaint {
        if self.background == background {
            return Repaint::None;
        }
        self.background = background;
        self.rebuild_cache();
        Repaint::Full
    }

    pub fn set_auto_fade(&mut self, seconds: Option<f32>) {
        self.fade_after = seconds;
    }

    // MARK: cache

    /// Annotations not drawn into the cache right now.
    fn hidden(&self) -> HashSet<AnnotationId> {
        let mut hidden = HashSet::new();
        match &self.gesture {
            Gesture::Erase { ids, .. } => hidden.extend(ids),
            Gesture::Move { .. } => hidden.extend(&self.selection),
            _ => {}
        }
        if let Some(id) = self.text.as_ref().and_then(|t| t.editing) {
            hidden.insert(id);
        }
        hidden
    }

    fn fade_alpha(&self, a: &Annotation, at: f64) -> f32 {
        let Some(after) = self.fade_after else {
            return 1.0;
        };
        let t = (at - a.created_at - after as f64) / FADE_DURATION;
        (1.0 - t.clamp(0.0, 1.0)) as f32
    }

    pub fn rebuild_cache(&mut self) {
        if self.asleep {
            return;
        }
        let hidden = self.hidden();
        let at = now();
        let transform = self.transform();
        let alphas: Vec<f32> = self
            .doc
            .annotations()
            .iter()
            .map(|a| self.fade_alpha(a, at))
            .collect();
        let mut cache = self.cache.as_mut();
        match &self.backdrop {
            // A frozen screen: the image, then the dim veil (or the whiteboard) over it.
            Some(image) if self.background.alpha < 1.0 => {
                cache.data_mut().copy_from_slice(image.data());
                if self.background.alpha > 1.5 / 255.0 {
                    let mut veil = Paint::default();
                    veil.set_color(skia_color(self.background, self.order));
                    let r = tiny_skia::Rect::from_xywh(
                        0.0,
                        0.0,
                        cache.width() as f32,
                        cache.height() as f32,
                    );
                    if let Some(r) = r {
                        cache.fill_rect(r, &veil, Transform::identity(), None);
                    }
                }
            }
            _ => cache.fill(skia_color(self.background, self.order)),
        }
        for (a, &alpha) in self.doc.annotations().iter().zip(&alphas) {
            if hidden.contains(&a.id) {
                continue;
            }
            if alpha >= 1.0 {
                draw(&mut cache, a, transform, self.order);
            } else if alpha > 0.0 {
                let mut faded = Annotation::clone(a);
                faded.style.opacity *= alpha;
                draw(&mut cache, &faded, transform, self.order);
            }
        }
    }

    /// Adds a committed annotation on top of the cache without a full rebuild.
    fn commit(&mut self, a: Annotation) -> Repaint {
        let transform = self.transform();
        if !self.asleep {
            draw(&mut self.cache.as_mut(), &a, transform, self.order);
        }
        let repaint = bounds(&a).map_or(Repaint::Full, |b| Repaint::Rect(self.to_pixels(b)));
        self.doc.insert(a);
        repaint
    }

    /// Drops annotations whose fade finished and redraws the ones fading.
    /// Returns whether something is still fading (keep ticking).
    pub fn tick(&mut self) -> (Repaint, bool) {
        // The laser trail fades out after release; overlays repaint themselves.
        if let Some(released) = self.laser.released
            && now() - released >= LASER_FADE
        {
            self.laser.trail.clear();
            self.laser.released = None;
        }
        let laser = self.laser.released.is_some();
        let (repaint, more) = self.tick_fade();
        (repaint, more || laser)
    }

    /// Whether something animates and needs frequent ticks (not auto-fade's slow ones).
    pub fn animating(&self) -> bool {
        self.laser.released.is_some()
    }

    fn tick_fade(&mut self) -> (Repaint, bool) {
        let Some(after) = self.fade_after else {
            return (Repaint::None, false);
        };
        let at = now();
        let mut expired = HashSet::new();
        let mut fading = false;
        // Text being edited or a selection being moved must not vanish mid-gesture.
        let busy = self.hidden();
        for a in self
            .doc
            .annotations()
            .iter()
            .filter(|a| !busy.contains(&a.id))
        {
            let age = at - a.created_at;
            if age >= after as f64 + FADE_DURATION {
                expired.insert(a.id);
            } else if age >= after as f64 {
                fading = true;
            }
        }
        if expired.is_empty() && !fading {
            let waiting = !self.doc.annotations().is_empty();
            return (Repaint::None, waiting);
        }
        self.doc.expire(&expired);
        self.selection.retain(|id| !expired.contains(id));
        self.rebuild_cache();
        (Repaint::Full, !self.doc.annotations().is_empty())
    }

    // MARK: painting

    /// Brings `target` up to date in the given region (or everywhere).
    /// Returns the pixel rectangle that changed, if any.
    pub fn paint(&mut self, target: &mut PixmapMut, repaint: Repaint) -> Option<IntRect> {
        if self.asleep {
            return None;
        }
        let overlay = self.overlay_bounds();
        let region = match repaint.union(self.painted_overlay.map_or(Repaint::None, Repaint::Rect))
        {
            Repaint::None => overlay.map_or(Repaint::None, Repaint::Rect),
            r => r.union(overlay.map_or(Repaint::None, Repaint::Rect)),
        };
        let rect = match region {
            Repaint::None => return None,
            Repaint::Full => IntRect::from_xywh(0, 0, self.width(), self.height()),
            Repaint::Rect(r) => clip(r, self.width(), self.height()),
        }?;

        copy_rect(&self.cache, target, rect);
        if let Some(lb) = self.live_bounds
            && let Some(live_rect) = clip(lb, self.width(), self.height())
            && let Some(both) = rect.intersect(&live_rect)
            && let Some(part) = self.live.as_ref().and_then(|live| live.clone_rect(both))
        {
            let paint = PixmapPaint {
                opacity: self.live_opacity,
                ..PixmapPaint::default()
            };
            target.draw_pixmap(
                both.x(),
                both.y(),
                part.as_ref(),
                &paint,
                Transform::identity(),
                None,
            );
        }
        self.draw_overlays(target, rect);
        self.painted_overlay = overlay;
        Some(rect)
    }

    /// Pixel bounds of the transient overlays, with room for antialiasing.
    fn overlay_bounds(&self) -> Option<Rect> {
        let mut r: Option<Rect> = None;
        let mut add = |b: Rect| r = Some(r.map_or(b, |r| r.union(b)));
        match &self.gesture {
            Gesture::Shape {
                preview: Some(a), ..
            } => {
                if let Some(b) = bounds(a) {
                    add(b);
                }
            }
            Gesture::Marquee { rect, .. } => add(*rect),
            _ => {}
        }
        if let Some(b) = self.snap_hint {
            add(b);
        }
        for (a, b) in &self.guides {
            add(Rect::from_points(*a, *b));
        }
        for b in self.selection_boxes() {
            add(b);
        }
        if let Some((p, radius)) = self.eraser_at {
            add(Rect::new(
                p.x - radius,
                p.y - radius,
                radius * 2.0,
                radius * 2.0,
            ));
        }
        if let Some(a) = self.text_annotation() {
            let b = bounds(&a).unwrap_or(Rect::new(a_origin(&a).x, a_origin(&a).y, 0.0, 0.0));
            add(b.union(self.caret(&a)));
            for r in self.selection_rects() {
                add(r);
            }
        }
        for p in self.laser.trail.iter().chain(&self.laser.at) {
            add(Rect::new(p.x - 14.0, p.y - 14.0, 28.0, 28.0));
        }
        // The veil outside the circle doesn't change as it moves; only the circle does.
        if let Some((Some(c), radius)) = self.spotlight
            && self.focus.is_none()
        {
            add(Rect::new(
                c.x - radius,
                c.y - radius,
                radius * 2.0,
                radius * 2.0,
            ));
        }
        r.map(|r| self.to_pixels(r.inset(-4.0)))
    }

    fn selection_boxes(&self) -> Vec<Rect> {
        let (dx, dy) = match self.gesture {
            Gesture::Move { offset, .. } => offset,
            _ => (0.0, 0.0),
        };
        self.doc
            .annotations()
            .iter()
            .filter(|a| self.selection.contains(&a.id))
            .filter_map(|a| bounds(a))
            .map(|b| b.offset(dx, dy).inset(-4.0))
            .collect()
    }

    fn draw_overlays(&self, target: &mut PixmapMut, region: IntRect) {
        let t = self.transform();
        self.draw_spotlight(target, region);
        self.draw_laser(target);
        match &self.gesture {
            Gesture::Shape {
                preview: Some(a), ..
            } => draw(target, a, t, self.order),
            Gesture::Move {
                offset: (dx, dy), ..
            } => {
                for a in self
                    .doc
                    .annotations()
                    .iter()
                    .filter(|a| self.selection.contains(&a.id))
                {
                    let mut moved = Annotation::clone(a);
                    moved.geometry = moved.geometry.translated(*dx, *dy);
                    draw(target, &moved, t, self.order);
                }
            }
            _ => {}
        }
        let mut accent = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        accent.set_color(skia_color(ACCENT, self.order));
        let dashed = Stroke {
            width: 1.5,
            dash: StrokeDash::new(vec![5.0, 4.0], 0.0),
            ..Stroke::default()
        };
        for b in self.selection_boxes() {
            if let Some(path) = b.to_skia().map(PathBuilder::from_rect) {
                target.stroke_path(&path, &accent, &dashed, t, None);
            }
        }
        if let Some(path) = self
            .snap_hint
            .and_then(|b| b.to_skia())
            .map(PathBuilder::from_rect)
        {
            let mut fill = accent.clone();
            fill.set_color(skia_color(ACCENT.with_alpha(0.1), self.order));
            target.fill_path(&path, &fill, FillRule::Winding, t, None);
            target.stroke_path(&path, &accent, &dashed, t, None);
        }
        if !self.guides.is_empty() {
            let mut pb = PathBuilder::new();
            for (a, b) in &self.guides {
                pb.move_to(a.x, a.y);
                pb.line_to(b.x, b.y);
            }
            if let Some(path) = pb.finish() {
                let mut guide = accent.clone();
                guide.set_color(skia_color(GUIDE, self.order));
                let stroke = Stroke {
                    width: 1.5,
                    ..Stroke::default()
                };
                target.stroke_path(&path, &guide, &stroke, t, None);
            }
        }
        if let Gesture::Marquee { rect, .. } = &self.gesture
            && let Some(path) = rect.to_skia().map(PathBuilder::from_rect)
        {
            let mut fill = accent.clone();
            fill.set_color(skia_color(ACCENT.with_alpha(0.12), self.order));
            target.fill_path(&path, &fill, FillRule::Winding, t, None);
            target.stroke_path(&path, &accent, &dashed, t, None);
        }
        if let Some((p, radius)) = self.eraser_at
            && let Some(ring) = PathBuilder::from_circle(p.x, p.y, radius)
        {
            let mut white = accent.clone();
            white.set_color(skia_color(Rgba::WHITE.with_alpha(0.9), self.order));
            target.stroke_path(
                &ring,
                &white,
                &Stroke {
                    width: 1.5,
                    ..Stroke::default()
                },
                t,
                None,
            );
        }
        if let Some(a) = self.text_annotation() {
            let mut highlight = accent.clone();
            highlight.set_color(skia_color(ACCENT.with_alpha(0.3), self.order));
            for r in self.selection_rects() {
                if let Some(r) = r.to_skia() {
                    target.fill_rect(r, &highlight, t, None);
                }
            }
            draw(target, &a, t, self.order);
            if let Some(path) = self.caret(&a).to_skia().map(PathBuilder::from_rect) {
                let mut caret = accent.clone();
                caret.set_color(skia_color(a.style.color, self.order));
                target.fill_path(&path, &caret, FillRule::Winding, t, None);
            }
        }
    }

    /// Puts a saved snapshot back: its screen image frozen under the ink, its
    /// annotations added as one undo step.
    pub fn restore(&mut self, annotations: Vec<Annotation>, backdrop: Option<Pixmap>) -> Repaint {
        self.cancel();
        self.doc.insert_all(annotations);
        self.set_backdrop(backdrop)
    }

    /// Holds `image` (or nothing) under the ink. It must match the canvas size.
    pub fn set_backdrop(&mut self, image: Option<Pixmap>) -> Repaint {
        self.backdrop = image.filter(|i| i.width() == self.width() && i.height() == self.height());
        self.snap = self.backdrop.as_ref().map(|i| self.edges(i));
        self.rebuild_cache();
        Repaint::Full
    }

    pub fn backdrop(&self) -> Option<&Pixmap> {
        self.backdrop.as_ref()
    }

    /// The screen as viewers see it: `screen` (or the frozen image) with the background
    /// veil and every annotation on top. Same byte order as the canvas.
    pub fn compose(&self, screen: Option<Pixmap>) -> Option<Pixmap> {
        let mut image = match (self.backdrop.clone(), screen) {
            (Some(frozen), _) => frozen,
            (None, Some(screen)) => screen,
            (None, None) => Pixmap::new(self.width(), self.height())?,
        };
        if self.background.alpha > 1.5 / 255.0 {
            let mut veil = Paint::default();
            veil.set_color(skia_color(self.background, self.order));
            let r =
                tiny_skia::Rect::from_xywh(0.0, 0.0, image.width() as f32, image.height() as f32)?;
            image.fill_rect(r, &veil, Transform::identity(), None);
        }
        let t = self.transform();
        for a in self.doc.annotations() {
            draw(&mut image.as_mut(), a, t, self.order);
        }
        Some(image)
    }

    pub fn spotlight_on(&self) -> bool {
        self.spotlight.is_some()
    }

    pub fn set_spotlight(&mut self, on: bool) -> Repaint {
        if on == self.spotlight.is_some() {
            return Repaint::None;
        }
        self.spotlight = on.then_some((None, SPOTLIGHT_RADIUS));
        self.focus = None;
        Repaint::Full
    }

    /// Wheel over the spotlight: bigger or smaller circle.
    pub fn resize_spotlight(&mut self, grow: bool) {
        if let Some((_, radius)) = &mut self.spotlight {
            let factor = if grow { 1.15 } else { 1.0 / 1.15 };
            *radius = (*radius * factor).clamp(SPOTLIGHT_LIMITS.0, SPOTLIGHT_LIMITS.1);
        }
    }

    /// A dark veil over `region`, with a hole around the pointer.
    fn draw_spotlight(&self, target: &mut PixmapMut, region: IntRect) {
        let Some((center, radius)) = self.spotlight else {
            return;
        };
        let mut pb = PathBuilder::new();
        pb.push_rect(region.to_rect());
        if let Some(f) = self.focus {
            let hole = self.to_pixels(f.inset(-FOCUS_PAD));
            if let Some(hole) = rounded_rect(hole, FOCUS_RADIUS * self.scale) {
                pb.push_path(&hole);
            }
        } else if let Some(c) = center {
            let s = self.scale;
            pb.push_circle(c.x * s, c.y * s, radius * s);
        }
        let Some(path) = pb.finish() else { return };
        let mut veil = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        veil.set_color(skia_color(Rgba::BLACK.with_alpha(0.65), self.order));
        target.fill_path(&path, &veil, FillRule::EvenOdd, Transform::identity(), None);
    }

    /// A glowing red trail with a dot at the pointer.
    fn draw_laser(&self, target: &mut PixmapMut) {
        let alpha = match self.laser.released {
            Some(released) => (1.0 - (now() - released) / LASER_FADE).clamp(0.0, 1.0) as f32,
            None => 1.0,
        };
        let t = self.transform();
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        if self.laser.trail.len() > 1 && alpha > 0.0 {
            let mut pb = PathBuilder::new();
            pb.move_to(self.laser.trail[0].x, self.laser.trail[0].y);
            for p in &self.laser.trail[1..] {
                pb.line_to(p.x, p.y);
            }
            if let Some(path) = pb.finish() {
                for (width, a) in [(12.0, 0.25), (4.0, 1.0)] {
                    paint.set_color(skia_color(LASER.with_alpha(a * alpha), self.order));
                    let stroke = Stroke {
                        width,
                        line_cap: LineCap::Round,
                        line_join: LineJoin::Round,
                        ..Stroke::default()
                    };
                    target.stroke_path(&path, &paint, &stroke, t, None);
                }
            }
        }
        if let Some(p) = self.laser.at {
            for (radius, a) in [(11.0, 0.3), (5.0, 1.0)] {
                paint.set_color(skia_color(LASER.with_alpha(a), self.order));
                if let Some(dot) = PathBuilder::from_circle(p.x, p.y, radius) {
                    target.fill_path(&dot, &paint, FillRule::Winding, t, None);
                }
            }
        }
    }

    // MARK: pointer

    pub fn pointer_down(&mut self, state: &ToolState, p: Pointer, button: Button) -> Repaint {
        let at = self.to_points(&p);
        self.snap_hint = None;
        self.guides.clear();
        if button == Button::Left && p.mods.ctrl && self.spotlight.is_some() {
            return self.toggle_focus(at);
        }
        if button == Button::Right {
            // Hold the right button to erase with any tool.
            return self.begin_erase(state, at);
        }
        if self.text.is_some() && state.tool == ToolKind::Text && self.on_text(at) {
            let index = self.text_index(at);
            if let Some(t) = &mut self.text {
                if p.double_click {
                    t.buffer.select_word(index);
                } else {
                    t.buffer.set_caret(index, p.mods.shift);
                    self.gesture = Gesture::TextSelect;
                }
                t.typing = false;
            }
            return Repaint::None;
        }
        if self.text.is_some() {
            let committed = self.end_text();
            if state.tool == ToolKind::Text {
                return committed;
            }
        }
        match state.tool {
            ToolKind::Pen if p.mods.ctrl && p.mods.alt => {
                self.begin_shape(ToolKind::Ellipse, at, true)
            }
            ToolKind::Pen if p.mods.ctrl => self.begin_shape(ToolKind::Rectangle, at, true),
            ToolKind::Pen if p.mods.alt => self.begin_shape(ToolKind::Arrow, at, false),
            ToolKind::Pen | ToolKind::Highlighter => self.begin_freehand(state, state.tool, &p, at),
            tool @ (ToolKind::Line | ToolKind::Arrow | ToolKind::Rectangle | ToolKind::Ellipse) => {
                self.begin_shape(tool, at, p.mods.ctrl)
            }
            ToolKind::Badge => {
                let a = Annotation::new(
                    Geometry::Badge {
                        center: at,
                        number: self.doc.next_badge_number(),
                        format: BadgeFormat::Decimal,
                        radius: state.badge_radius(),
                    },
                    Style::new(state.color(), state.width),
                );
                self.commit(a)
            }
            ToolKind::Eraser => self.begin_erase(state, at),
            ToolKind::Select => self.select_down(at, p.mods.shift, p.double_click),
            ToolKind::Text => self.text_down(state, at),
            ToolKind::Laser => {
                self.laser = Laser {
                    trail: vec![at],
                    at: Some(at),
                    released: None,
                };
                self.gesture = Gesture::Laser;
                Repaint::None
            }
        }
    }

    pub fn pointer_move(&mut self, state: &ToolState, p: Pointer) -> Repaint {
        let at = self.to_points(&p);
        // Shift's angles and squares win over the magnet.
        let end = match self.gesture {
            Gesture::Shape { .. } => self.snapped(at, magnet(state, p.mods) && !p.mods.shift),
            _ => Snapped::free(at),
        };
        if let Some((center, _)) = &mut self.spotlight {
            *center = Some(at);
        }
        match &mut self.gesture {
            Gesture::Laser => {
                self.laser.at = Some(at);
                self.laser.trail.push(at);
                // Holding the button for long would make every move redraw an ever longer path.
                if self.laser.trail.len() > LASER_POINTS {
                    self.laser
                        .trail
                        .drain(..self.laser.trail.len() - LASER_POINTS);
                }
                Repaint::None
            }
            Gesture::Idle => Repaint::None,
            Gesture::TextSelect => {
                let index = self.text_index(at);
                if let Some(t) = &mut self.text {
                    t.buffer.set_caret(index, true);
                }
                Repaint::None
            }
            Gesture::Freehand {
                builder,
                style,
                filter,
                pressure,
                last_width,
            } => {
                let width = if *pressure {
                    let pressure = filter.filter(p.pressure);
                    filter.width(style.width, pressure)
                } else {
                    style.width
                };
                *last_width = width;
                match builder.add(at, width) {
                    Some(segment) => self.draw_live(segment_outline(&segment)),
                    None => Repaint::None,
                }
            }
            Gesture::Shape {
                tool,
                start,
                pressed,
                start_edges,
                preview,
            } => {
                // A click stays a click even when the magnet moves both ends apart.
                if pressed.distance(at) < 3.0 {
                    *preview = None;
                    self.guides.clear();
                    return Repaint::None;
                }
                self.guides = guide_lines(*start, *start_edges)
                    .into_iter()
                    .chain(guide_lines(end.point, (end.x, end.y)))
                    .collect();
                let at = end.point;
                let constrain = p.mods.shift;
                let curved =
                    *tool == ToolKind::Arrow && p.mods.alt && state.tool == ToolKind::Arrow;
                let radius = if *tool == ToolKind::Rectangle {
                    6.0
                } else {
                    0.0
                };
                let style = state.style(*tool);
                *preview = shape_geometry(*tool, *start, at, constrain, curved, radius)
                    .map(|g| Annotation::new(g, style));
                Repaint::None // overlays repaint themselves
            }
            Gesture::Erase { ids, radius } => {
                let radius = *radius;
                let before = ids.len();
                for a in self.doc.annotations() {
                    if !ids.contains(&a.id) && hit_test(a, at, radius) {
                        ids.insert(a.id);
                    }
                }
                let changed = ids.len() != before;
                self.eraser_at = Some((at, radius));
                if changed {
                    self.rebuild_cache();
                    return Repaint::Full;
                }
                Repaint::None
            }
            Gesture::Move { start, offset } => {
                *offset = (at.x - start.x, at.y - start.y);
                Repaint::None
            }
            Gesture::Marquee {
                start,
                initial,
                rect,
            } => {
                *rect = Rect::from_points(*start, at);
                let r = *rect;
                let mut selection = initial.clone();
                for a in self.doc.annotations() {
                    if bounds(a).is_some_and(|b| b.intersects(r)) {
                        selection.insert(a.id);
                    }
                }
                self.selection = selection;
                Repaint::None
            }
        }
    }

    /// Hover without a button: only the eraser ring follows the pointer.
    pub fn hover(&mut self, state: &ToolState, p: Pointer) -> Repaint {
        let at = self.to_points(&p);
        self.laser.at = (state.tool == ToolKind::Laser).then_some(at);
        if let Some((center, _)) = &mut self.spotlight {
            *center = Some(at);
        }
        if state.tool == ToolKind::Eraser && matches!(self.gesture, Gesture::Idle) {
            self.eraser_at = Some((self.to_points(&p), state.eraser_radius()));
        } else {
            self.eraser_at = None;
        }
        self.snap_hint = None;
        self.guides.clear();
        if matches!(self.gesture, Gesture::Idle)
            && self.magnet_on(state, p.mods)
            && let Some(map) = &self.snap
        {
            let frames = self.spotlight.is_some()
                || matches!(
                    state.tool,
                    ToolKind::Pen | ToolKind::Rectangle | ToolKind::Ellipse
                );
            self.snap_hint = frames.then(|| map.box_at(at)).flatten();
            let s = map.snap(at, MAGNET_RADIUS);
            self.guides = guide_lines(s.point, (s.x, s.y));
        }
        // Overlays repaint themselves in `paint`.
        Repaint::None
    }

    pub fn pointer_up(&mut self, state: &ToolState, p: Pointer) -> Repaint {
        let at = self.to_points(&p);
        let gesture = std::mem::replace(&mut self.gesture, Gesture::Idle);
        match gesture {
            Gesture::Idle => Repaint::None,
            Gesture::Laser => {
                self.laser.released = Some(now());
                Repaint::None
            }
            Gesture::Freehand {
                mut builder,
                style,
                last_width,
                ..
            } => {
                // Pointer-up reports zero pressure, so keep the last width.
                if let Some(segment) = builder.add(at, last_width) {
                    self.draw_live(segment_outline(&segment));
                }
                let repaint = self.live_bounds.map_or(Repaint::None, Repaint::Rect);
                self.clear_live();
                let (points, widths) = builder.into_parts();
                repaint
                    .union(self.commit(Annotation::new(Geometry::Stroke { points, widths }, style)))
            }
            Gesture::Shape {
                tool,
                pressed,
                preview,
                ..
            } => {
                self.guides.clear();
                match preview {
                    Some(a) => self.commit(a),
                    None if magnet(state, p.mods) => self.frame_box(state, tool, pressed),
                    None => Repaint::None,
                }
            }
            Gesture::Erase { ids, .. } => {
                self.eraser_at =
                    (state.tool == ToolKind::Eraser).then_some((at, state.eraser_radius()));
                self.doc.remove(&ids);
                self.rebuild_cache();
                Repaint::Full
            }
            Gesture::Move {
                offset: (dx, dy), ..
            } => {
                if dx != 0.0 || dy != 0.0 {
                    let ids = self.selection.clone();
                    self.doc
                        .update(&ids, |a| a.geometry = a.geometry.translated(dx, dy));
                }
                self.rebuild_cache();
                Repaint::Full
            }
            Gesture::Marquee { .. } | Gesture::TextSelect => Repaint::None,
        }
    }

    fn begin_freehand(
        &mut self,
        state: &ToolState,
        tool: ToolKind,
        p: &Pointer,
        at: Point,
    ) -> Repaint {
        let style = state.style(tool);
        let mut builder = StrokeBuilder::new();
        let mut filter = PressureFilter::default();
        let pressure = tool == ToolKind::Pen;
        let width = if pressure {
            let smoothed = filter.filter(p.pressure);
            filter.width(style.width, smoothed)
        } else {
            style.width
        };
        builder.add(at, width);
        self.clear_live();
        self.live_opacity = style.opacity;
        // The stroke is drawn opaque into `live`; its opacity applies when composited.
        self.live_color = style.color;
        self.gesture = Gesture::Freehand {
            builder,
            style,
            filter,
            pressure,
            last_width: width,
        };
        // Immediate feedback for a click; later segments draw over it.
        let dot = PathBuilder::from_circle(at.x, at.y, (width / 2.0).max(0.25));
        self.draw_live(dot)
    }

    fn begin_shape(&mut self, tool: ToolKind, at: Point, magnet: bool) -> Repaint {
        let start = self.snapped(at, magnet);
        self.gesture = Gesture::Shape {
            tool,
            start: start.point,
            pressed: at,
            start_edges: (start.x, start.y),
            preview: None,
        };
        Repaint::None
    }

    // MARK: magnet

    /// Whether the screen under the ink is needed before this pointer event:
    /// call `set_screen` first.
    pub fn needs_screen(&self, state: &ToolState, mods: Mods) -> bool {
        !self.asleep && self.snap.is_none() && self.magnet_on(state, mods)
    }

    /// Ctrl with a shape tool, or with the spotlight on (to light a box).
    fn magnet_on(&self, state: &ToolState, mods: Mods) -> bool {
        magnet(state, mods) || (mods.ctrl && self.spotlight.is_some())
    }

    /// With the spotlight on, Ctrl+click lights the box under the pointer instead of
    /// the circle; again on it, or where there's no box, brings the circle back.
    fn toggle_focus(&mut self, at: Point) -> Repaint {
        let b = self.snap.as_ref().and_then(|m| m.box_at(at));
        self.focus = if b == self.focus { None } else { b };
        Repaint::Full
    }

    /// The screen under the ink for the magnet, or None when it can't be captured.
    pub fn set_screen(&mut self, image: Option<&Pixmap>) {
        self.snap = Some(image.map_or_else(SnapMap::default, |i| self.edges(i)));
    }

    /// The screen may have changed (drawing turned on, back from mouse mode): the
    /// magnet captures it again when next used. A frozen screen stays.
    pub fn forget_screen(&mut self) {
        if self.backdrop.is_none() {
            self.snap = None;
        }
        self.snap_hint = None;
        self.guides.clear();
    }

    fn edges(&self, image: &Pixmap) -> SnapMap {
        SnapMap::detect(image.data(), image.width(), image.height(), self.scale)
    }

    fn snapped(&self, at: Point, magnet: bool) -> Snapped {
        match &self.snap {
            Some(map) if magnet => map.snap(at, MAGNET_RADIUS),
            _ => Snapped::free(at),
        }
    }

    /// Ctrl+click with a rectangle or an ellipse frames the box under the pointer.
    fn frame_box(&mut self, state: &ToolState, tool: ToolKind, at: Point) -> Repaint {
        let style = state.style(tool);
        let Some(b) = self.snap.as_ref().and_then(|m| m.box_at(at)) else {
            return Repaint::None;
        };
        // Just outside the box, so the ink doesn't cover what it points at.
        let r = b.inset(-(style.width / 2.0 + 2.0));
        let geometry = match tool {
            ToolKind::Rectangle => Geometry::Rectangle {
                rect: r,
                corner_radius: 6.0,
            },
            ToolKind::Ellipse => {
                // Through the box's corners.
                let (w, h) = (r.width * SQRT_2, r.height * SQRT_2);
                let c = r.center();
                Geometry::Ellipse {
                    rect: Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h),
                }
            }
            _ => return Repaint::None,
        };
        self.commit(Annotation::new(geometry, style))
    }

    fn begin_erase(&mut self, state: &ToolState, at: Point) -> Repaint {
        self.gesture = Gesture::Erase {
            ids: HashSet::new(),
            radius: state.eraser_radius(),
        };
        let p = Pointer {
            x: at.x * self.scale,
            y: at.y * self.scale,
            pressure: 1.0,
            mods: Mods::default(),
            double_click: false,
        };
        self.pointer_move(state, p)
    }

    fn select_down(&mut self, at: Point, shift: bool, double_click: bool) -> Repaint {
        let hit = self
            .doc
            .annotations()
            .iter()
            .rev()
            .find(|a| hit_test(a, at, 6.0))
            .map(|a| Annotation::clone(a));
        let Some(hit) = hit else {
            let initial = if shift {
                self.selection.clone()
            } else {
                HashSet::new()
            };
            self.selection = initial.clone();
            self.gesture = Gesture::Marquee {
                start: at,
                initial,
                rect: Rect::new(at.x, at.y, 0.0, 0.0),
            };
            return Repaint::None;
        };
        if double_click && hit.kind() == AnnotationKind::Text {
            return self.edit_text(&hit);
        }
        if shift {
            if !self.selection.remove(&hit.id) {
                self.selection.insert(hit.id);
            }
        } else if !self.selection.contains(&hit.id) {
            self.selection = HashSet::from([hit.id]);
        }
        self.gesture = Gesture::Move {
            start: at,
            offset: (0.0, 0.0),
        };
        self.rebuild_cache();
        Repaint::Full
    }

    // MARK: live stroke

    fn draw_live(&mut self, path: Option<tiny_skia::Path>) -> Repaint {
        let Some(path) = path else {
            return Repaint::None;
        };
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color(skia_color(self.live_color, self.order));
        let t = self.transform();
        if self.live.is_none() {
            self.live = Pixmap::new(self.width, self.height);
        }
        let Some(live) = &mut self.live else {
            return Repaint::None;
        };
        live.fill_path(&path, &paint, FillRule::Winding, t, None);
        let b = path.bounds();
        let px = self.to_pixels(Rect::from_skia(b)).inset(-2.0);
        self.live_bounds = Some(self.live_bounds.map_or(px, |r| r.union(px)));
        Repaint::Rect(px)
    }

    /// The live layer is only needed during a stroke; a fresh one is zeroed anyway.
    fn clear_live(&mut self) {
        self.live_bounds = None;
        self.live = None;
    }

    // MARK: text

    fn text_down(&mut self, state: &ToolState, at: Point) -> Repaint {
        let hit = self
            .doc
            .annotations()
            .iter()
            .rev()
            .find(|a| a.kind() == AnnotationKind::Text && hit_test(a, at, 6.0))
            .map(|a| Annotation::clone(a));
        if let Some(hit) = hit {
            let r = self.edit_text(&hit);
            let index = self.text_index(at);
            if let Some(t) = &mut self.text {
                t.buffer.set_caret(index, false);
            }
            self.gesture = Gesture::TextSelect;
            return r;
        }
        // Put the click at the middle of the first line, where the caret appears.
        let font_size = state.font_size();
        let origin = Point::new(
            at.x,
            at.y - screenpen_core::text::line_height(font_size) / 2.0,
        );
        self.text = Some(TextEdit {
            origin,
            buffer: TextBuffer::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            font_size,
            style: state.text_style.clone(),
            color: state.color(),
            editing: None,
        });
        Repaint::None
    }

    fn edit_text(&mut self, a: &Annotation) -> Repaint {
        let Geometry::Text {
            origin,
            string,
            font_size,
            style,
        } = &a.geometry
        else {
            return Repaint::None;
        };
        self.selection.clear();
        self.text = Some(TextEdit {
            origin: *origin,
            buffer: TextBuffer::new(string),
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            font_size: *font_size,
            style: style.clone(),
            color: a.style.color,
            editing: Some(a.id),
        });
        self.rebuild_cache();
        Repaint::Full
    }

    fn text_annotation(&self) -> Option<Annotation> {
        let t = self.text.as_ref()?;
        Some(Annotation::new(
            Geometry::Text {
                origin: t.origin,
                string: t.buffer.text().to_string(),
                font_size: t.font_size,
                style: t.style.clone(),
            },
            Style::new(t.color, 2.0),
        ))
    }

    /// Where a char offset of the text being edited sits: x and the line's top.
    fn text_position(t: &TextEdit, at: usize) -> Point {
        let text = t.buffer.text();
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        let row = text[..line_start].matches('\n').count();
        let stops =
            screenpen_core::text::caret_stops(&text[line_start..line_end], t.font_size, &t.style);
        let x = stops
            .iter()
            .rev()
            .find(|(i, _)| *i <= at - line_start)
            .map_or(0.0, |(_, x)| *x);
        Point::new(
            t.origin.x + x,
            t.origin.y + row as f32 * screenpen_core::text::line_height(t.font_size),
        )
    }

    /// The char offset nearest to a point, in the text being edited.
    fn text_index(&self, at: Point) -> usize {
        let Some(t) = &self.text else {
            return 0;
        };
        let text = t.buffer.text();
        let line_height = screenpen_core::text::line_height(t.font_size);
        let row = ((at.y - t.origin.y) / line_height).floor().max(0.0) as usize;
        let mut line_start = 0;
        for _ in 0..row {
            match text[line_start..].find('\n') {
                Some(i) => line_start += i + 1,
                None => break,
            }
        }
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        let stops =
            screenpen_core::text::caret_stops(&text[line_start..line_end], t.font_size, &t.style);
        let x = at.x - t.origin.x;
        line_start
            + stops
                .iter()
                .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
                .map_or(0, |(i, _)| *i)
    }

    /// Whether a point is on the text being edited (with some slack around it).
    fn on_text(&self, at: Point) -> bool {
        let Some(a) = self.text_annotation() else {
            return false;
        };
        let caret = self.caret(&a);
        let b = bounds(&a).map_or(caret, |b| b.union(caret));
        b.inset(-8.0).contains(at)
    }

    /// A thin bar at the caret.
    fn caret(&self, a: &Annotation) -> Rect {
        let Some(t) = &self.text else {
            return Rect::default();
        };
        let line_height = screenpen_core::text::line_height(a_font_size(a));
        let p = Self::text_position(t, t.buffer.caret());
        Rect::new(p.x, p.y + line_height * 0.1, 2.0, line_height * 0.85)
    }

    /// Highlight boxes of the selection, one per line.
    fn selection_rects(&self) -> Vec<Rect> {
        let Some(t) = &self.text else {
            return Vec::new();
        };
        let r = t.buffer.selection();
        if r.is_empty() {
            return Vec::new();
        }
        let text = t.buffer.text();
        let line_height = screenpen_core::text::line_height(t.font_size);
        let mut rects = Vec::new();
        let mut start = r.start;
        loop {
            let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            let end = line_end.min(r.end);
            let a = Self::text_position(t, start);
            let b = Self::text_position(t, end);
            // A selected line break shows as a little extra width.
            let extra = if end == line_end && end < r.end {
                t.font_size * 0.3
            } else {
                0.0
            };
            rects.push(Rect::new(
                a.x,
                a.y,
                (b.x - a.x + extra).max(1.0),
                line_height,
            ));
            if end >= r.end {
                break;
            }
            start = line_end + 1;
        }
        rects
    }

    /// Typed text while the text tool is active (control chars are keys, see `text_key`).
    pub fn type_char(&mut self, c: char) -> Repaint {
        // macOS reports function and arrow keys as private-use chars.
        if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) {
            return Repaint::None;
        }
        self.text_change(true, |b| b.insert(c.encode_utf8(&mut [0; 4])));
        Repaint::None
    }

    /// Inserts clipboard text at the caret.
    pub fn paste(&mut self, s: &str) -> Repaint {
        self.text_change(false, |b| b.insert(s));
        Repaint::None
    }

    /// Applies an edit, keeping the undo history; typed chars join one step.
    fn text_change(&mut self, typing: bool, edit: impl FnOnce(&mut TextBuffer)) {
        let Some(t) = &mut self.text else {
            return;
        };
        let before = t.buffer.clone();
        edit(&mut t.buffer);
        if t.buffer.text() == before.text() {
            return;
        }
        if !(typing && t.typing) {
            t.undo.push(before);
        }
        t.typing = typing;
        t.redo.clear();
    }

    /// Editing keys while typing: caret movement, selection, deletion, clipboard,
    /// undo. None for keys that aren't about the text.
    pub fn text_key(&mut self, chord: Chord) -> Option<TextKey> {
        let t = self.text.as_mut()?;
        let (word, extend) = (chord.ctrl, chord.shift);
        let m = match chord.key {
            Key::Left => Some(Motion::Left),
            Key::Right => Some(Motion::Right),
            Key::Up => Some(Motion::Up),
            Key::Down => Some(Motion::Down),
            Key::Home => Some(Motion::LineStart),
            Key::End => Some(Motion::LineEnd),
            _ => None,
        };
        if let Some(m) = m {
            if chord.alt {
                return None;
            }
            t.buffer.motion(m, word, extend);
            t.typing = false;
            return Some(TextKey::Handled(Repaint::None));
        }
        match chord.key {
            Key::Backspace if !chord.alt => self.text_change(false, |b| b.backspace(word)),
            Key::Delete if !chord.alt => self.text_change(false, |b| b.delete(word)),
            // Ctrl+Enter finishes; Enter and Shift+Enter break the line.
            Key::Enter if chord.ctrl => return Some(TextKey::Handled(self.end_text())),
            Key::Enter if !chord.alt => self.text_change(false, |b| b.insert("\n")),
            Key::Char(c) if chord.ctrl && !chord.alt => match c {
                'A' => t.buffer.select_all(),
                'C' => return t.buffer.selected().map(|s| TextKey::Copy(s.to_string())),
                'X' => {
                    let s = t.buffer.selected()?.to_string();
                    self.text_change(false, |b| b.insert(""));
                    return Some(TextKey::Copy(s));
                }
                'V' => return Some(TextKey::Paste),
                'Z' if !chord.shift => {
                    let before = t.undo.pop()?;
                    t.redo.push(std::mem::replace(&mut t.buffer, before));
                    t.typing = false;
                }
                'Y' | 'Z' => {
                    let after = t.redo.pop()?;
                    t.undo.push(std::mem::replace(&mut t.buffer, after));
                    t.typing = false;
                }
                _ => return None,
            },
            _ => return None,
        }
        Some(TextKey::Handled(Repaint::None))
    }

    /// Commits the text being edited (or deletes it when emptied).
    pub fn end_text(&mut self) -> Repaint {
        let Some(t) = self.text.take() else {
            return Repaint::None;
        };
        let string = t.buffer.text().trim().to_string();
        let geometry = Geometry::Text {
            origin: t.origin,
            string: string.clone(),
            font_size: t.font_size,
            style: t.style.clone(),
        };
        match t.editing {
            Some(id) if string.is_empty() => self.doc.remove(&HashSet::from([id])),
            Some(id) => self
                .doc
                .update(&HashSet::from([id]), |a| a.geometry = geometry.clone()),
            None if !string.is_empty() => {
                return self.commit(Annotation::new(geometry, Style::new(t.color, 2.0)));
            }
            None => return Repaint::None,
        }
        self.rebuild_cache();
        Repaint::Full
    }

    // MARK: actions

    /// Cancels any gesture in progress (tool switch, hide, ghost).
    pub fn cancel(&mut self) -> Repaint {
        let committed = self.end_text();
        let had_gesture = !matches!(self.gesture, Gesture::Idle);
        self.gesture = Gesture::Idle;
        self.eraser_at = None;
        self.snap_hint = None;
        self.guides.clear();
        self.laser = Laser::default();
        let live = self.live_bounds.map_or(Repaint::None, Repaint::Rect);
        self.clear_live();
        if had_gesture {
            self.rebuild_cache();
            return Repaint::Full;
        }
        committed.union(live)
    }

    /// Runs a drawing-time shortcut. Returns what to repaint and what the app must do.
    pub fn perform(&mut self, state: &mut ToolState, action: Action) -> (Repaint, Option<Request>) {
        let full = |c: &mut Canvas| {
            c.rebuild_cache();
            Repaint::Full
        };
        match action {
            Action::Tool(tool) => {
                state.tool = tool;
                if tool != ToolKind::Select {
                    self.selection.clear();
                }
                (
                    self.cancel().union(Repaint::Full),
                    Some(Request::StateChanged),
                )
            }
            Action::Color(i) => {
                let Some(&color) = state.colors.get(i as usize) else {
                    return (Repaint::None, None);
                };
                if self.selection.is_empty() {
                    state.color_index = i as usize;
                    if let Some(t) = &mut self.text {
                        t.color = color;
                    }
                    (Repaint::None, Some(Request::StateChanged))
                } else {
                    let ids = self.selection.clone();
                    self.doc.update(&ids, |a| a.style.color = color);
                    (full(self), None)
                }
            }
            Action::TextSmaller | Action::TextBigger => {
                let bigger = action == Action::TextBigger;
                self.restyle_text(
                    state,
                    |size, _| *size = next_text_size(*size, bigger),
                    |s| s.step_text_size(bigger),
                )
            }
            Action::Bold => self.restyle_text(
                state,
                |_, style| style.bold = !style.bold,
                |s| s.text_style.bold = !s.text_style.bold,
            ),
            Action::Italic => self.restyle_text(
                state,
                |_, style| style.italic = !style.italic,
                |s| s.text_style.italic = !s.text_style.italic,
            ),
            Action::Thinner | Action::Thicker => {
                state.step_width(action == Action::Thicker);
                (Repaint::None, Some(Request::StateChanged))
            }
            Action::Width(i) => {
                let presets = screenpen_core::shortcuts::WIDTH_PRESETS;
                state.width = presets[(i as usize).min(presets.len() - 1)];
                (Repaint::None, Some(Request::StateChanged))
            }
            Action::Clear => {
                self.cancel();
                self.doc.remove_all();
                self.selection.clear();
                (full(self), None)
            }
            Action::Undo => {
                self.cancel();
                self.doc.undo();
                self.prune_selection();
                (full(self), None)
            }
            Action::Redo => {
                self.cancel();
                self.doc.redo();
                self.prune_selection();
                (full(self), None)
            }
            Action::SelectAll => {
                state.tool = ToolKind::Select;
                self.selection = self.doc.annotations().iter().map(|a| a.id).collect();
                (Repaint::None, Some(Request::StateChanged))
            }
            Action::BringToFront => {
                let ids = self.selection.clone();
                self.doc.bring_to_front(&ids);
                (full(self), None)
            }
            Action::DeleteSelection => {
                let ids = std::mem::take(&mut self.selection);
                self.doc.remove(&ids);
                (full(self), None)
            }
            Action::Escape => {
                if self.text.is_some() {
                    return (self.end_text(), None);
                }
                if !self.selection.is_empty() {
                    self.selection.clear();
                    return (Repaint::None, None);
                }
                (Repaint::None, Some(Request::StopDrawing))
            }
            Action::Ghost => (self.cancel(), Some(Request::Ghost)),
            Action::Whiteboard => (Repaint::None, Some(Request::ToggleWhiteboard)),
            Action::Help => (Repaint::None, Some(Request::Help)),
            Action::Spotlight => (Repaint::None, Some(Request::ToggleSpotlight)),
            Action::Freeze => (Repaint::None, Some(Request::ToggleFreeze)),
            Action::Magnifier => (Repaint::None, Some(Request::ToggleMagnifier)),
            Action::Snapshot => (Repaint::None, Some(Request::Snapshot)),
            Action::ToggleDrawing | Action::ExitGhost | Action::Quit => (Repaint::None, None),
        }
    }

    /// Applies a size or style change to the text being typed, else to the selected
    /// text, else to the defaults for new text.
    fn restyle_text(
        &mut self,
        state: &mut ToolState,
        change: impl Fn(&mut f32, &mut TextStyle),
        default: impl FnOnce(&mut ToolState),
    ) -> (Repaint, Option<Request>) {
        if let Some(t) = &mut self.text {
            change(&mut t.font_size, &mut t.style);
            return (Repaint::None, None);
        }
        let texts: HashSet<AnnotationId> = self
            .doc
            .annotations()
            .iter()
            .filter(|a| self.selection.contains(&a.id) && a.kind() == AnnotationKind::Text)
            .map(|a| a.id)
            .collect();
        if !texts.is_empty() {
            self.doc.update(&texts, |a| {
                if let Geometry::Text {
                    font_size, style, ..
                } = &mut a.geometry
                {
                    change(font_size, style);
                }
            });
            self.rebuild_cache();
            return (Repaint::Full, None);
        }
        default(state);
        (Repaint::None, Some(Request::StateChanged))
    }

    fn prune_selection(&mut self) {
        let ids: HashSet<_> = self.doc.annotations().iter().map(|a| a.id).collect();
        self.selection.retain(|id| ids.contains(id));
    }
}

fn a_font_size(a: &Annotation) -> f32 {
    match &a.geometry {
        Geometry::Text { font_size, .. } => *font_size,
        _ => 0.0,
    }
}

fn a_origin(a: &Annotation) -> Point {
    match &a.geometry {
        Geometry::Text { origin, .. } => *origin,
        _ => Point::default(),
    }
}

/// `r` rounded out to whole pixels and clipped to the surface.
fn clip(r: Rect, width: u32, height: u32) -> Option<IntRect> {
    let left = r.left().floor().max(0.0) as i32;
    let top = r.top().floor().max(0.0) as i32;
    let right = (r.right().ceil() as i32).min(width as i32);
    let bottom = (r.bottom().ceil() as i32).min(height as i32);
    if right <= left || bottom <= top {
        return None;
    }
    IntRect::from_xywh(left, top, (right - left) as u32, (bottom - top) as u32)
}

/// Copies a rectangle of pixels between same-sized surfaces.
fn copy_rect(src: &Pixmap, dst: &mut PixmapMut, rect: IntRect) {
    let stride = src.width() as usize * 4;
    let (x, w) = (rect.x() as usize * 4, rect.width() as usize * 4);
    let src = src.data();
    let dst = dst.data_mut();
    for row in rect.y() as usize..rect.y() as usize + rect.height() as usize {
        let at = row * stride + x;
        dst[at..at + w].copy_from_slice(&src[at..at + w]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new(400, 300, 2.0, ChannelOrder::Rgba).unwrap()
    }

    fn at(x: f32, y: f32) -> Pointer {
        Pointer {
            x,
            y,
            pressure: 1.0,
            mods: Mods::default(),
            double_click: false,
        }
    }

    fn with(mods: Mods, x: f32, y: f32) -> Pointer {
        Pointer { mods, ..at(x, y) }
    }

    /// Drives one left-button drag through the canvas and paints after every event.
    fn drag(c: &mut Canvas, state: &ToolState, points: &[Pointer]) -> Pixmap {
        let mut target = Pixmap::new(c.width(), c.height()).unwrap();
        let r = c.pointer_down(state, points[0], Button::Left);
        c.paint(&mut target.as_mut(), r);
        for &p in &points[1..] {
            let r = c.pointer_move(state, p);
            c.paint(&mut target.as_mut(), r);
        }
        let r = c.pointer_up(state, *points.last().unwrap());
        c.paint(&mut target.as_mut(), r);
        target
    }

    fn line(from: (f32, f32), to: (f32, f32), mods: Mods) -> Vec<Pointer> {
        (0..=20)
            .map(|i| {
                let t = i as f32 / 20.0;
                with(
                    mods,
                    from.0 + (to.0 - from.0) * t,
                    from.1 + (to.1 - from.1) * t,
                )
            })
            .collect()
    }

    fn kinds(c: &Canvas) -> Vec<AnnotationKind> {
        c.doc.annotations().iter().map(|a| a.kind()).collect()
    }

    fn opaque(p: &Pixmap, x: u32, y: u32) -> bool {
        p.pixel(x, y).is_some_and(|px| px.alpha() > 200)
    }

    /// A light screen with a dark panel at (40, 30)-(160, 120) points, at scale 2.
    fn panel_screen() -> Pixmap {
        let mut screen = Pixmap::new(400, 300).unwrap();
        screen.fill(tiny_skia::Color::from_rgba8(235, 235, 235, 255));
        let r = tiny_skia::Rect::from_xywh(80.0, 60.0, 240.0, 180.0).unwrap();
        let mut paint = Paint::default();
        paint.set_color_rgba8(60, 60, 60, 255);
        screen.fill_rect(r, &paint, Transform::identity(), None);
        screen
    }

    fn only_rect(c: &Canvas) -> Rect {
        match c.doc.annotations() {
            [a] => match a.geometry {
                Geometry::Rectangle { rect, .. } => rect,
                ref g => panic!("{g:?}"),
            },
            other => panic!("{} annotations", other.len()),
        }
    }

    const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
    };

    #[test]
    fn magnet_snaps_a_rectangle_to_a_panel() {
        let mut c = canvas();
        let state = ToolState {
            tool: ToolKind::Rectangle,
            ..ToolState::default()
        };
        assert!(!c.needs_screen(&state, Mods::default()));
        assert!(c.needs_screen(&state, CTRL));
        c.set_screen(Some(&panel_screen()));
        assert!(!c.needs_screen(&state, CTRL));

        drag(&mut c, &state, &line((86., 66.), (314., 234.), CTRL));
        assert_eq!(only_rect(&c), Rect::new(40.0, 30.0, 120.0, 90.0));

        // Without Ctrl the rectangle is where the pointer was.
        c.doc.remove_all();
        drag(
            &mut c,
            &state,
            &line((86., 66.), (314., 234.), Mods::default()),
        );
        assert_eq!(only_rect(&c), Rect::new(43.0, 33.0, 114.0, 84.0));
    }

    #[test]
    fn magnet_click_frames_the_box_under_the_pointer() {
        let mut c = canvas();
        let mut state = ToolState::default();
        c.set_screen(Some(&panel_screen()));
        // Hovering shows the box first.
        c.hover(&state, with(CTRL, 200., 150.));
        assert_eq!(c.snap_hint, Some(Rect::new(40.0, 30.0, 120.0, 90.0)));
        c.hover(&state, at(200., 150.));
        assert_eq!(c.snap_hint, None);

        // The pen's Ctrl rectangle frames it just outside.
        drag(&mut c, &state, &[with(CTRL, 200., 150.)]);
        let pad = state.style(ToolKind::Rectangle).width / 2.0 + 2.0;
        assert_eq!(
            only_rect(&c),
            Rect::new(40.0, 30.0, 120.0, 90.0).inset(-pad)
        );

        // Nothing to frame outside the panel.
        c.doc.remove_all();
        state.tool = ToolKind::Rectangle;
        drag(&mut c, &state, &[with(CTRL, 20., 20.)]);
        assert!(c.doc.annotations().is_empty());
    }

    #[test]
    fn spotlight_lights_a_box_on_ctrl_click() {
        let mut c = canvas();
        let state = ToolState::default();
        c.set_spotlight(true);
        assert!(c.needs_screen(&state, CTRL));
        c.set_screen(Some(&panel_screen()));
        let mut target = Pixmap::new(400, 300).unwrap();
        drag(&mut c, &state, &[with(CTRL, 200., 150.)]);
        assert!(c.doc.annotations().is_empty(), "nothing drawn");
        assert_eq!(c.focus, Some(Rect::new(40.0, 30.0, 120.0, 90.0)));
        c.hover(&state, at(20., 20.));
        c.paint(&mut target.as_mut(), Repaint::Full);
        let dark = |x, y| target.pixel(x, y).unwrap().alpha() > 100;
        assert!(!dark(200, 150) && dark(20, 20) && !dark(85, 65));
        // Again on the same box: back to the circle.
        drag(&mut c, &state, &[with(CTRL, 200., 150.)]);
        assert_eq!(c.focus, None);
    }

    #[test]
    fn freeze_feeds_the_magnet() {
        let mut c = canvas();
        let state = ToolState::default();
        c.set_backdrop(Some(panel_screen()));
        assert!(!c.needs_screen(&state, CTRL));
        // A frozen screen doesn't change, so its edges stay.
        c.forget_screen();
        assert!(!c.needs_screen(&state, CTRL));
        c.set_backdrop(None);
        assert!(c.needs_screen(&state, CTRL));
    }

    #[test]
    fn pen_stroke_commits_and_paints() {
        let mut c = canvas();
        let state = ToolState::default();
        let target = drag(
            &mut c,
            &state,
            &line((20., 20.), (200., 100.), Mods::default()),
        );
        assert_eq!(kinds(&c), [AnnotationKind::Stroke]);
        // Model is in points: pixels / scale.
        let Geometry::Stroke { points, .. } = &c.doc.annotations()[0].geometry else {
            panic!()
        };
        assert_eq!(points[0], Point::new(10.0, 10.0));
        assert!(opaque(&target, 110, 60), "the stroke is on screen");
        assert!(!opaque(&target, 300, 250));
    }

    #[test]
    fn pen_modifiers_make_shapes() {
        let mut c = canvas();
        let state = ToolState::default();
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        };
        drag(&mut c, &state, &line((20., 20.), (200., 120.), ctrl));
        drag(&mut c, &state, &line((20., 20.), (200., 120.), ctrl_alt));
        drag(&mut c, &state, &line((20., 200.), (300., 200.), alt));
        assert_eq!(
            kinds(&c),
            [
                AnnotationKind::Rectangle,
                AnnotationKind::Ellipse,
                AnnotationKind::Arrow
            ]
        );
    }

    #[test]
    fn shift_makes_a_square() {
        let mut c = canvas();
        let state = ToolState::default();
        let mods = Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        };
        drag(&mut c, &state, &line((20., 20.), (220., 120.), mods));
        let Geometry::Rectangle { rect, .. } = c.doc.annotations()[0].geometry else {
            panic!()
        };
        assert_eq!(rect.width, rect.height);
    }

    #[test]
    fn right_button_erases_and_undo_restores() {
        let mut c = canvas();
        let state = ToolState::default();
        drag(
            &mut c,
            &state,
            &line((20., 50.), (300., 50.), Mods::default()),
        );
        let mut target = Pixmap::new(c.width(), c.height()).unwrap();
        c.pointer_down(&state, at(150., 50.), Button::Right);
        c.pointer_up(&state, at(150., 50.));
        c.paint(&mut target.as_mut(), Repaint::Full);
        assert!(c.doc.annotations().is_empty());
        assert!(!opaque(&target, 150, 50));
        let mut state = state;
        c.perform(&mut state, Action::Undo);
        assert_eq!(kinds(&c), [AnnotationKind::Stroke]);
    }

    #[test]
    fn select_drag_moves_as_one_undo_step() {
        let mut c = canvas();
        let mut state = ToolState::default();
        drag(
            &mut c,
            &state,
            &line((20., 50.), (100., 50.), Mods::default()),
        );
        state.tool = ToolKind::Select;
        drag(
            &mut c,
            &state,
            &line((60., 50.), (60., 150.), Mods::default()),
        );
        let b = bounds(&c.doc.annotations()[0]).unwrap();
        assert!((b.center().y - 75.0).abs() < 1.0, "{b:?}");
        c.perform(&mut state, Action::Undo);
        let b = bounds(&c.doc.annotations()[0]).unwrap();
        assert!((b.center().y - 25.0).abs() < 1.0, "{b:?}");
    }

    #[test]
    fn marquee_selects_and_color_recolors() {
        let mut c = canvas();
        let mut state = ToolState::default();
        drag(
            &mut c,
            &state,
            &line((20., 50.), (100., 50.), Mods::default()),
        );
        drag(
            &mut c,
            &state,
            &line((20., 250.), (100., 250.), Mods::default()),
        );
        state.tool = ToolKind::Select;
        drag(
            &mut c,
            &state,
            &line((5., 5.), (200., 100.), Mods::default()),
        );
        assert_eq!(c.selection.len(), 1);
        c.perform(&mut state, Action::Color(3));
        let colors: Vec<_> = c.doc.annotations().iter().map(|a| a.style.color).collect();
        assert_eq!(colors, [Rgba::BLUE, Rgba::RED]);
        assert_eq!(
            state.color_index, 0,
            "recoloring a selection keeps the tool color"
        );
    }

    fn key(ctrl: bool, shift: bool, key: Key) -> Chord {
        Chord {
            ctrl,
            alt: false,
            shift,
            key,
        }
    }

    fn typed(c: &Canvas) -> String {
        c.text.as_ref().unwrap().buffer.text().to_string()
    }

    #[test]
    fn text_edits_like_a_text_field() {
        let mut c = canvas();
        let state = ToolState {
            tool: ToolKind::Text,
            ..ToolState::default()
        };
        c.pointer_down(&state, at(50., 50.), Button::Left);
        c.pointer_up(&state, at(50., 50.));
        for ch in "one two".chars() {
            c.type_char(ch);
        }
        // Ctrl+Shift+Left selects the last word, Ctrl+X cuts it.
        c.text_key(key(true, true, Key::Left));
        assert_eq!(
            c.text_key(key(true, false, Key::Char('X'))),
            Some(TextKey::Copy("two".into()))
        );
        assert_eq!(typed(&c), "one ");
        // Home, then paste in front.
        c.text_key(key(false, false, Key::Home));
        c.paste("two ");
        assert_eq!(typed(&c), "two one ");
        // Ctrl+A, Ctrl+C copies all; typing replaces it.
        c.text_key(key(true, false, Key::Char('A')));
        assert_eq!(
            c.text_key(key(true, false, Key::Char('C'))),
            Some(TextKey::Copy("two one ".into()))
        );
        c.type_char('x');
        assert_eq!(typed(&c), "x");
        // Undo steps back through the replace and the paste.
        c.text_key(key(true, false, Key::Char('Z')));
        assert_eq!(typed(&c), "two one ");
        c.text_key(key(true, false, Key::Char('Z')));
        assert_eq!(typed(&c), "one ");
        c.text_key(key(true, false, Key::Char('Y')));
        assert_eq!(typed(&c), "two one ");
        // Keys that aren't about text fall through.
        assert_eq!(c.text_key(key(false, false, Key::F(5))), None);
        // Ctrl+Enter finishes.
        c.text_key(key(true, false, Key::Enter));
        assert!(!c.is_typing());
    }

    #[test]
    fn clicking_in_the_text_moves_the_caret() {
        let mut c = canvas();
        let state = ToolState {
            tool: ToolKind::Text,
            ..ToolState::default()
        };
        c.pointer_down(&state, at(50., 50.), Button::Left);
        c.pointer_up(&state, at(50., 50.));
        for ch in "hello".chars() {
            c.type_char(ch);
        }
        let origin = c.text.as_ref().unwrap().origin;
        // A click left of the first glyph puts the caret at the start and keeps editing
        // (pointers are in pixels, the scale is 2).
        let start = at((origin.x + 1.0) * 2.0, (origin.y + 5.0) * 2.0);
        c.pointer_down(&state, start, Button::Left);
        c.pointer_up(&state, start);
        assert!(c.is_typing());
        assert_eq!(c.text.as_ref().unwrap().buffer.caret(), 0);
        // A click far away commits it.
        c.pointer_down(&state, at(400., 400.), Button::Left);
        assert!(!c.is_typing());
        assert_eq!(c.doc.annotations().len(), 1);
    }

    #[test]
    fn typing_commits_text_on_escape() {
        let mut c = canvas();
        let mut state = ToolState {
            tool: ToolKind::Text,
            ..ToolState::default()
        };
        c.pointer_down(&state, at(50., 50.), Button::Left);
        c.pointer_up(&state, at(50., 50.));
        for ch in "Hi!".chars() {
            c.type_char(ch);
        }
        c.text_key(key(false, false, Key::Backspace));
        assert!(c.is_typing());
        c.perform(&mut state, Action::Escape);
        assert!(!c.is_typing());
        let Geometry::Text { string, .. } = &c.doc.annotations()[0].geometry else {
            panic!()
        };
        assert_eq!(string, "Hi");
    }

    #[test]
    fn badges_count_up() {
        let mut c = canvas();
        let state = ToolState {
            tool: ToolKind::Badge,
            ..ToolState::default()
        };
        for x in [50., 150.] {
            c.pointer_down(&state, at(x, 50.), Button::Left);
            c.pointer_up(&state, at(x, 50.));
        }
        let numbers: Vec<_> = c
            .doc
            .annotations()
            .iter()
            .map(|a| match a.geometry {
                Geometry::Badge { number, .. } => number,
                _ => 0,
            })
            .collect();
        assert_eq!(numbers, [1, 2]);
    }

    #[test]
    fn escape_without_selection_stops_drawing() {
        let mut c = canvas();
        let mut state = ToolState::default();
        assert_eq!(
            c.perform(&mut state, Action::Escape).1,
            Some(Request::StopDrawing)
        );
    }

    #[test]
    fn highlighter_is_translucent_as_a_whole() {
        let mut c = canvas();
        let state = ToolState {
            tool: ToolKind::Highlighter,
            ..ToolState::default()
        };
        // Back and forth over the same spot: overlaps must not get darker.
        let mut points = line((20., 50.), (200., 50.), Mods::default());
        points.extend(line((200., 50.), (20., 50.), Mods::default()));
        let target = drag(&mut c, &state, &points);
        let px = target.pixel(100, 50).unwrap();
        assert!(px.alpha() > 80 && px.alpha() < 130, "alpha {}", px.alpha());
    }

    #[test]
    fn laser_leaves_nothing_behind() {
        let mut c = canvas();
        let state = ToolState {
            tool: ToolKind::Laser,
            ..ToolState::default()
        };
        let target = drag(
            &mut c,
            &state,
            &line((20., 50.), (300., 50.), Mods::default()),
        );
        assert!(c.doc.annotations().is_empty());
        assert!(c.animating(), "the trail fades after release");
        assert!(
            opaque(&target, 150, 50),
            "the trail is visible right after release"
        );
        c.laser.released = Some(now() - LASER_FADE - 0.1);
        c.tick();
        assert!(!c.animating());
        assert!(c.laser.trail.is_empty());
    }

    #[test]
    fn spotlight_dims_all_but_a_circle() {
        let mut c = canvas();
        let state = ToolState::default();
        let mut target = Pixmap::new(c.width(), c.height()).unwrap();
        let r = c.set_spotlight(true);
        for _ in 0..10 {
            c.resize_spotlight(false); // down to the smallest circle, 40 pt
        }
        c.hover(&state, at(200., 150.));
        c.paint(&mut target.as_mut(), r);
        assert!(
            target.pixel(200, 150).unwrap().alpha() < 10,
            "clear around the pointer"
        );
        assert!(target.pixel(5, 5).unwrap().alpha() > 150, "dark elsewhere");
        c.resize_spotlight(true);
        let r = c.set_spotlight(false);
        c.paint(&mut target.as_mut(), r);
        assert!(target.pixel(5, 5).unwrap().alpha() < 10);
    }

    #[test]
    fn compose_puts_ink_over_the_screen() {
        let mut c = canvas();
        let state = ToolState::default();
        drag(
            &mut c,
            &state,
            &line((20., 50.), (300., 50.), Mods::default()),
        );
        let mut screen = Pixmap::new(c.width(), c.height()).unwrap();
        screen.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        let image = c.compose(Some(screen.clone())).unwrap();
        let ink = image.pixel(150, 50).unwrap();
        assert!(ink.red() > 200 && ink.blue() < 100, "red ink over blue");
        assert!(
            image.pixel(150, 250).unwrap().blue() > 200,
            "the screen elsewhere"
        );
        // Frozen: the backdrop wins over a fresh capture.
        c.set_backdrop(Some(screen));
        assert!(c.compose(None).unwrap().pixel(150, 250).unwrap().blue() > 200);
    }
}

#[cfg(test)]
mod sleep_tests {
    use super::*;

    #[test]
    fn sleeping_frees_buffers_and_waking_redraws() {
        let mut c = Canvas::new(400, 300, 1.0, ChannelOrder::Rgba).unwrap();
        let state = ToolState::default();
        let p = |x, y| Pointer {
            x,
            y,
            pressure: 1.0,
            mods: Mods::default(),
            double_click: false,
        };
        c.pointer_down(&state, p(10.0, 10.0), Button::Left);
        c.pointer_move(&state, p(200.0, 100.0));
        c.pointer_up(&state, p(200.0, 100.0));
        assert!(c.live.is_none(), "the live layer goes after the stroke");
        c.sleep();
        assert_eq!(c.cache.width(), 1);
        let mut target = Pixmap::new(400, 300).unwrap();
        assert_eq!(c.paint(&mut target.as_mut(), Repaint::Full), None);
        assert!(c.wake());
        c.paint(&mut target.as_mut(), Repaint::Full);
        assert!(
            target.pixel(105, 55).unwrap().alpha() > 200,
            "the stroke is back"
        );
    }
}
