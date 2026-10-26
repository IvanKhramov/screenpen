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

use screenpen_core::annotation::{AnnotationId, now};
use screenpen_core::render::{ChannelOrder, bounds, draw, hit_test, segment_outline, skia_color};
use screenpen_core::tools::shape_geometry;
use screenpen_core::{
    Action, Annotation, AnnotationKind, BadgeFormat, Document, Geometry, Point, PressureFilter,
    Rect, Rgba, StrokeBuilder, Style, ToolKind,
};
use tiny_skia::{
    FillRule, IntRect, Paint, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Stroke, StrokeDash,
    Transform,
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
    /// Tool, color or width changed: every screen and the palette should refresh.
    StateChanged,
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
        start: Point,
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
    Marquee {
        start: Point,
        initial: HashSet<AnnotationId>,
        rect: Rect,
    },
}

struct TextEdit {
    origin: Point,
    string: String,
    font_size: f32,
    color: Rgba,
    /// The annotation being edited, hidden from the cache meanwhile.
    editing: Option<AnnotationId>,
}

/// Auto-fade: how long the fade-out itself takes.
const FADE_DURATION: f64 = 0.4;
const ACCENT: Rgba = Rgba::rgb(0.04, 0.52, 1.0);

pub struct Canvas {
    pub doc: Document,
    scale: f32,
    order: ChannelOrder,
    cache: Pixmap,
    live: Pixmap,
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
}

impl Canvas {
    pub fn new(width: u32, height: u32, scale: f32, order: ChannelOrder) -> Option<Canvas> {
        Some(Canvas {
            doc: Document::new(),
            scale,
            order,
            cache: Pixmap::new(width, height)?,
            live: Pixmap::new(width, height)?,
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
        })
    }

    pub fn width(&self) -> u32 {
        self.cache.width()
    }

    pub fn height(&self) -> u32 {
        self.cache.height()
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
        cache.fill(skia_color(self.background, self.order));
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
        draw(&mut self.cache.as_mut(), &a, transform, self.order);
        let repaint = bounds(&a).map_or(Repaint::Full, |b| Repaint::Rect(self.to_pixels(b)));
        self.doc.insert(a);
        repaint
    }

    /// Drops annotations whose fade finished and redraws the ones fading.
    /// Returns whether something is still fading (keep ticking).
    pub fn tick(&mut self) -> (Repaint, bool) {
        let Some(after) = self.fade_after else {
            return (Repaint::None, false);
        };
        let at = now();
        let mut expired = HashSet::new();
        let mut fading = false;
        for a in self.doc.annotations() {
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
            && let Some(part) = self.live.clone_rect(both)
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
        self.draw_overlays(target);
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

    fn draw_overlays(&self, target: &mut PixmapMut) {
        let t = self.transform();
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
            draw(target, &a, t, self.order);
            if let Some(path) = self.caret(&a).to_skia().map(PathBuilder::from_rect) {
                let mut caret = accent.clone();
                caret.set_color(skia_color(a.style.color, self.order));
                target.fill_path(&path, &caret, FillRule::Winding, t, None);
            }
        }
    }

    // MARK: pointer

    pub fn pointer_down(&mut self, state: &ToolState, p: Pointer, button: Button) -> Repaint {
        let at = self.to_points(&p);
        if button == Button::Right {
            // Hold the right button to erase with any tool.
            return self.begin_erase(state, at);
        }
        if self.text.is_some() {
            let committed = self.end_text();
            if state.tool == ToolKind::Text {
                return committed;
            }
        }
        match state.tool {
            ToolKind::Pen if p.mods.ctrl && p.mods.alt => self.begin_shape(ToolKind::Ellipse, at),
            ToolKind::Pen if p.mods.ctrl => self.begin_shape(ToolKind::Rectangle, at),
            ToolKind::Pen if p.mods.alt => self.begin_shape(ToolKind::Arrow, at),
            ToolKind::Pen | ToolKind::Highlighter => self.begin_freehand(state, state.tool, &p, at),
            tool @ (ToolKind::Line | ToolKind::Arrow | ToolKind::Rectangle | ToolKind::Ellipse) => {
                self.begin_shape(tool, at)
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
        }
    }

    pub fn pointer_move(&mut self, state: &ToolState, p: Pointer) -> Repaint {
        let at = self.to_points(&p);
        match &mut self.gesture {
            Gesture::Idle => Repaint::None,
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
                preview,
            } => {
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
        if state.tool == ToolKind::Eraser && matches!(self.gesture, Gesture::Idle) {
            self.eraser_at = Some((self.to_points(&p), state.eraser_radius()));
        } else {
            self.eraser_at = None;
        }
        // Overlays repaint themselves in `paint`.
        Repaint::None
    }

    pub fn pointer_up(&mut self, state: &ToolState, p: Pointer) -> Repaint {
        let at = self.to_points(&p);
        let gesture = std::mem::replace(&mut self.gesture, Gesture::Idle);
        match gesture {
            Gesture::Idle => Repaint::None,
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
            Gesture::Shape { preview, .. } => match preview {
                Some(a) => self.commit(a),
                None => Repaint::None,
            },
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
            Gesture::Marquee { .. } => Repaint::None,
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

    fn begin_shape(&mut self, tool: ToolKind, at: Point) -> Repaint {
        self.gesture = Gesture::Shape {
            tool,
            start: at,
            preview: None,
        };
        Repaint::None
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
        self.live
            .fill_path(&path, &paint, FillRule::Winding, t, None);
        let b = path.bounds();
        let px = self.to_pixels(Rect::from_skia(b)).inset(-2.0);
        self.live_bounds = Some(self.live_bounds.map_or(px, |r| r.union(px)));
        Repaint::Rect(px)
    }

    fn clear_live(&mut self) {
        if let Some(r) = self.live_bounds.take()
            && let Some(rect) = clip(r, self.width(), self.height())
        {
            clear_rect(&mut self.live, rect);
        }
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
            return self.edit_text(&hit);
        }
        // Put the click at the middle of the first line, where the caret appears.
        let font_size = state.font_size();
        let origin = Point::new(
            at.x,
            at.y - screenpen_core::text::line_height(font_size) / 2.0,
        );
        self.text = Some(TextEdit {
            origin,
            string: String::new(),
            font_size,
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
        } = &a.geometry
        else {
            return Repaint::None;
        };
        self.selection.clear();
        self.text = Some(TextEdit {
            origin: *origin,
            string: string.clone(),
            font_size: *font_size,
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
                string: t.string.clone(),
                font_size: t.font_size,
            },
            Style::new(t.color, 2.0),
        ))
    }

    /// A thin bar after the last character.
    fn caret(&self, a: &Annotation) -> Rect {
        let Geometry::Text {
            origin,
            string,
            font_size,
        } = &a.geometry
        else {
            return Rect::default();
        };
        let line_height = screenpen_core::text::line_height(*font_size);
        let lines = string.split('\n').count().max(1);
        let last = string.rsplit('\n').next().unwrap_or("");
        // Glyph bounds ignore trailing spaces, so add roughly a space width for each.
        let trailing = (last.len() - last.trim_end().len()) as f32 * font_size * 0.28;
        let ink = screenpen_core::text::path(last.trim_end(), *font_size, *origin)
            .map_or(origin.x, |p| p.bounds().right() + 1.0);
        let x = ink + trailing;
        Rect::new(
            x,
            origin.y + (lines - 1) as f32 * line_height + line_height * 0.1,
            2.0,
            line_height * 0.85,
        )
    }

    /// Typed text while the text tool is active.
    pub fn type_char(&mut self, c: char) -> Repaint {
        let Some(t) = &mut self.text else {
            return Repaint::None;
        };
        match c {
            '\u{8}' => {
                t.string.pop();
            }
            '\r' | '\n' => t.string.push('\n'),
            c if !c.is_control() => t.string.push(c),
            _ => return Repaint::None,
        }
        Repaint::None
    }

    /// Commits the text being edited (or deletes it when emptied).
    pub fn end_text(&mut self) -> Repaint {
        let Some(t) = self.text.take() else {
            return Repaint::None;
        };
        let string = t.string.trim().to_string();
        let geometry = Geometry::Text {
            origin: t.origin,
            string: string.clone(),
            font_size: t.font_size,
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
            Action::Thinner | Action::Thicker => {
                state.step_width(action == Action::Thicker);
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
            Action::ToggleDrawing | Action::ExitGhost | Action::Quit => (Repaint::None, None),
        }
    }

    fn prune_selection(&mut self) {
        let ids: HashSet<_> = self.doc.annotations().iter().map(|a| a.id).collect();
        self.selection.retain(|id| ids.contains(id));
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

fn clear_rect(pixmap: &mut Pixmap, rect: IntRect) {
    let stride = pixmap.width() as usize * 4;
    let (x, w) = (rect.x() as usize * 4, rect.width() as usize * 4);
    let data = pixmap.data_mut();
    for row in rect.y() as usize..rect.y() as usize + rect.height() as usize {
        let at = row * stride + x;
        data[at..at + w].fill(0);
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
        c.perform(&mut state, Action::Color(4));
        let colors: Vec<_> = c.doc.annotations().iter().map(|a| a.style.color).collect();
        assert_eq!(colors, [Rgba::BLUE, Rgba::RED]);
        assert_eq!(
            state.color_index, 0,
            "recoloring a selection keeps the tool color"
        );
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
        for ch in "Hi!\u{8}".chars() {
            c.type_char(ch);
        }
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
}
