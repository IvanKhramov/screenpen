//! The floating palette, its tooltips and the shortcut cheat sheet, drawn with
//! tiny-skia. Layout is in points; rendering takes the monitor scale.

use screenpen_core::render::{ChannelOrder, skia_color};
use screenpen_core::{Action, Point, Rect, Rgba, Shortcuts, TextStyle, ToolKind, text};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, Stroke, Transform,
};

use crate::i18n::{Lang, T};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Action(Action),
    Settings,
}

impl Item {
    pub fn defaults() -> Vec<Item> {
        let mut items: Vec<Item> = ToolKind::ALL
            .iter()
            .map(|&t| Item::Action(Action::Tool(t)))
            .collect();
        items.extend((0..9).map(|i| Item::Action(Action::Color(i))));
        items.extend(
            [
                Action::Thinner,
                Action::Thicker,
                Action::TextSmaller,
                Action::TextBigger,
                Action::Undo,
                Action::Redo,
                Action::Clear,
                Action::Ghost,
                Action::Whiteboard,
                Action::Spotlight,
                Action::Freeze,
                Action::Snapshot,
                Action::Help,
            ]
            .map(Item::Action),
        );
        items.push(Item::Settings);
        items
    }

    /// Everything that can be on the palette, for the settings window (phase 3b).
    #[allow(dead_code)]
    pub fn all() -> Vec<Item> {
        let mut items = Item::defaults();
        items.insert(items.len() - 1, Item::Action(Action::Bold));
        items.insert(items.len() - 1, Item::Action(Action::Italic));
        items.insert(items.len() - 1, Item::Action(Action::SelectAll));
        items.insert(items.len() - 1, Item::Action(Action::BringToFront));
        items.insert(items.len() - 1, Item::Action(Action::DeleteSelection));
        items
    }

    pub fn name(self) -> String {
        match self {
            Item::Action(a) => a.name(),
            Item::Settings => "settings".to_string(),
        }
    }

    pub fn from_name(name: &str) -> Option<Item> {
        if name == "settings" {
            return Some(Item::Settings);
        }
        Action::from_name(name)
            .filter(|a| !a.is_global())
            .map(Item::Action)
    }

    /// Items next to each other in the same group sit closer together.
    fn group(self) -> u8 {
        match self {
            Item::Action(Action::Tool(_)) => 0,
            Item::Action(Action::Color(_)) => 1,
            Item::Action(Action::Thinner | Action::Thicker) => 2,
            Item::Action(
                Action::TextSmaller | Action::TextBigger | Action::Bold | Action::Italic,
            ) => 7,
            Item::Action(Action::Undo | Action::Redo | Action::Clear) => 3,
            Item::Action(
                Action::SelectAll | Action::BringToFront | Action::DeleteSelection | Action::Escape,
            ) => 4,
            Item::Action(
                Action::Ghost | Action::Whiteboard | Action::Spotlight | Action::Freeze,
            ) => 5,
            _ => 6,
        }
    }

    pub fn label(self, lang: Lang) -> String {
        match self {
            Item::Action(a) => lang.action(a),
            Item::Settings => lang.t(T::Settings).trim_end_matches('…').to_string(),
        }
    }

    /// "Pen · P", or just the name when there's no shortcut.
    pub fn tooltip(self, lang: Lang, shortcuts: &Shortcuts) -> String {
        let label = self.label(lang);
        match self {
            Item::Action(a) => match shortcuts.chord(a) {
                Some(c) => format!("{label}  ·  {}", chord_label(c)),
                None => label,
            },
            Item::Settings => label,
        }
    }
}

/// "?" reads better than "Shift+/".
pub fn chord_label(c: screenpen_core::Chord) -> String {
    if c == (screenpen_core::Chord {
        shift: true,
        ..screenpen_core::Chord::key(screenpen_core::Key::Char('/'))
    }) {
        return "?".to_string();
    }
    c.to_string()
}

/// What the palette highlights.
pub struct Look {
    pub tool: ToolKind,
    pub bold: bool,
    pub italic: bool,
    pub spotlight: bool,
    pub frozen: bool,
    pub color_index: usize,
    pub colors: [Rgba; 9],
    pub ghost: bool,
    pub whiteboard: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Grip,
    Item(usize),
}

const CELL: f32 = 32.0;
const PAD: f32 = 4.0;
const GAP: f32 = 6.0;
const GRIP: f32 = 14.0;
const BACKGROUND: Rgba = Rgba {
    red: 0.17,
    green: 0.17,
    blue: 0.18,
    alpha: 0.95,
};
const INK: Rgba = Rgba::rgb(0.92, 0.92, 0.94);
const ACCENT: Rgba = Rgba::rgb(0.04, 0.52, 1.0);

pub struct Palette {
    pub items: Vec<Item>,
    pub vertical: bool,
    pub hover: Option<usize>,
}

impl Palette {
    /// Cell rectangles and the grip, in points, plus the overall size.
    pub fn layout(&self) -> (Vec<Rect>, Rect, (f32, f32)) {
        let mut cells = Vec::with_capacity(self.items.len());
        let mut along = PAD + GRIP;
        let mut previous: Option<u8> = None;
        for item in &self.items {
            if previous.is_some_and(|g| g != item.group()) {
                along += GAP;
            }
            previous = Some(item.group());
            cells.push(if self.vertical {
                Rect::new(PAD, along, CELL, CELL)
            } else {
                Rect::new(along, PAD, CELL, CELL)
            });
            along += CELL;
        }
        let length = along + PAD;
        let across = CELL + 2.0 * PAD;
        if self.vertical {
            (
                cells,
                Rect::new(0.0, 0.0, across, PAD + GRIP),
                (across, length),
            )
        } else {
            (
                cells,
                Rect::new(0.0, 0.0, PAD + GRIP, across),
                (length, across),
            )
        }
    }

    pub fn size_px(&self, scale: f32) -> (u32, u32) {
        let (_, _, (w, h)) = self.layout();
        ((w * scale).ceil() as u32, (h * scale).ceil() as u32)
    }

    pub fn hit(&self, p: Point) -> Option<Hit> {
        let (cells, grip, _) = self.layout();
        if grip.contains(p) {
            return Some(Hit::Grip);
        }
        cells.iter().position(|c| c.contains(p)).map(Hit::Item)
    }

    pub fn render(&self, pixmap: &mut PixmapMut, scale: f32, order: ChannelOrder, look: &Look) {
        let t = Transform::from_scale(scale, scale);
        let (cells, grip, (w, h)) = self.layout();
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        fill(
            pixmap,
            rounded(Rect::new(0.0, 0.0, w, h), 10.0),
            BACKGROUND,
            t,
            order,
        );

        // Grip: two columns of dots.
        let c = grip.center();
        let (dx, dy) = if self.vertical {
            (4.0, 2.5)
        } else {
            (2.5, 4.0)
        };
        for i in -1..=1 {
            for j in [-1.0, 1.0] {
                let (x, y) = if self.vertical {
                    (c.x + i as f32 * dx * 1.6, c.y + j * dy)
                } else {
                    (c.x + j * dx, c.y + i as f32 * dy * 1.6)
                };
                fill(
                    pixmap,
                    PathBuilder::from_circle(x, y, 1.3),
                    INK.with_alpha(0.55),
                    t,
                    order,
                );
            }
        }

        for (i, (item, cell)) in self.items.iter().zip(&cells).enumerate() {
            let active = match item {
                Item::Action(Action::Tool(tool)) => *tool == look.tool,
                Item::Action(Action::Ghost) => look.ghost,
                Item::Action(Action::Whiteboard) => look.whiteboard,
                Item::Action(Action::Spotlight) => look.spotlight,
                Item::Action(Action::Bold) => look.bold,
                Item::Action(Action::Italic) => look.italic,
                Item::Action(Action::Freeze) => look.frozen,
                _ => false,
            };
            if active {
                fill(pixmap, rounded(cell.inset(1.0), 7.0), ACCENT, t, order);
            } else if self.hover == Some(i) {
                fill(
                    pixmap,
                    rounded(cell.inset(1.0), 7.0),
                    INK.with_alpha(0.14),
                    t,
                    order,
                );
            }
            draw_icon(pixmap, *item, *cell, look, t, order);
        }
    }
}

fn rounded(r: Rect, radius: f32) -> Option<Path> {
    screenpen_core::render::rounded_rect(r, radius)
}

fn fill(
    pixmap: &mut PixmapMut,
    path: Option<Path>,
    color: Rgba,
    t: Transform,
    order: ChannelOrder,
) {
    let Some(path) = path else { return };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(skia_color(color, order));
    pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
}

fn stroke(
    pixmap: &mut PixmapMut,
    path: Option<Path>,
    color: Rgba,
    width: f32,
    t: Transform,
    order: ChannelOrder,
) {
    let Some(path) = path else { return };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(skia_color(color, order));
    let s = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &s, t, None);
}

/// Builds a path in a 20x20 icon box centered in `cell`.
fn icon_path(
    cell: Rect,
    build: impl FnOnce(&mut PathBuilder, &dyn Fn(f32, f32) -> (f32, f32)),
) -> Option<Path> {
    let (ox, oy) = (
        cell.x + (cell.width - 20.0) / 2.0,
        cell.y + (cell.height - 20.0) / 2.0,
    );
    let at = move |x: f32, y: f32| (ox + x, oy + y);
    let mut pb = PathBuilder::new();
    build(&mut pb, &at);
    pb.finish()
}

fn polyline(cell: Rect, points: &[(f32, f32)], close: bool) -> Option<Path> {
    icon_path(cell, |pb, at| {
        for (i, &(x, y)) in points.iter().enumerate() {
            let (x, y) = at(x, y);
            if i == 0 {
                pb.move_to(x, y)
            } else {
                pb.line_to(x, y)
            }
        }
        if close {
            pb.close();
        }
    })
}

fn glyph(cell: Rect, s: &str, size: f32) -> Option<Path> {
    let path = text::path(s, size, Point::default())?;
    let b = path.bounds();
    let c = cell.center();
    path.transform(Transform::from_translate(
        c.x - (b.left() + b.right()) / 2.0,
        c.y - (b.top() + b.bottom()) / 2.0,
    ))
}

fn draw_icon(
    pixmap: &mut PixmapMut,
    item: Item,
    cell: Rect,
    look: &Look,
    t: Transform,
    o: ChannelOrder,
) {
    let ink = INK;
    let line = |pixmap: &mut PixmapMut, pts: &[(f32, f32)], w: f32| {
        stroke(pixmap, polyline(cell, pts, false), ink, w, t, o)
    };
    let Item::Action(action) = item else {
        // Settings: a gear.
        let c = cell.center();
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            let (s, co) = a.sin_cos();
            let mut pb = PathBuilder::new();
            pb.move_to(c.x + co * 5.5, c.y + s * 5.5);
            pb.line_to(c.x + co * 8.5, c.y + s * 8.5);
            stroke(pixmap, pb.finish(), ink, 2.6, t, o);
        }
        stroke(
            pixmap,
            PathBuilder::from_circle(c.x, c.y, 5.5),
            ink,
            2.2,
            t,
            o,
        );
        return;
    };
    match action {
        Action::Tool(ToolKind::Pen) => {
            line(pixmap, &[(5.0, 15.0), (14.5, 5.5)], 3.2);
            fill(
                pixmap,
                polyline(cell, &[(3.0, 17.0), (4.0, 13.5), (6.5, 16.0)], true),
                ink,
                t,
                o,
            );
        }
        Action::Tool(ToolKind::Highlighter) => {
            let color = look
                .colors
                .get(look.color_index)
                .copied()
                .unwrap_or(Rgba::YELLOW);
            stroke(
                pixmap,
                polyline(cell, &[(4.0, 13.0), (16.0, 13.0)], false),
                color.with_alpha(0.55),
                6.0,
                t,
                o,
            );
            line(pixmap, &[(6.0, 6.0), (14.0, 6.0)], 1.8);
        }
        Action::Tool(ToolKind::Line) => line(pixmap, &[(4.0, 16.0), (16.0, 4.0)], 2.0),
        Action::Tool(ToolKind::Arrow) => {
            line(pixmap, &[(4.0, 16.0), (14.5, 5.5)], 2.0);
            fill(
                pixmap,
                polyline(cell, &[(16.5, 3.5), (9.5, 5.5), (14.5, 10.5)], true),
                ink,
                t,
                o,
            );
        }
        Action::Tool(ToolKind::Rectangle) => stroke(
            pixmap,
            rounded(
                Rect::new(cell.center().x - 7.5, cell.center().y - 5.5, 15.0, 11.0),
                2.0,
            ),
            ink,
            2.0,
            t,
            o,
        ),
        Action::Tool(ToolKind::Ellipse) => {
            let c = cell.center();
            let oval = tiny_skia::Rect::from_xywh(c.x - 8.0, c.y - 5.5, 16.0, 11.0)
                .and_then(PathBuilder::from_oval);
            stroke(pixmap, oval, ink, 2.0, t, o);
        }
        Action::Tool(ToolKind::Text) => fill(pixmap, glyph(cell, "T", 18.0), ink, t, o),
        Action::Tool(ToolKind::Badge) => {
            let c = cell.center();
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 8.0), ink, t, o);
            fill(
                pixmap,
                glyph(cell, "1", 11.0),
                BACKGROUND.with_alpha(1.0),
                t,
                o,
            );
        }
        Action::Tool(ToolKind::Eraser) => {
            let body = [
                (3.0, 13.0),
                (10.0, 6.0),
                (17.0, 13.0),
                (13.0, 17.0),
                (7.0, 17.0),
            ];
            stroke(pixmap, polyline(cell, &body, true), ink, 1.8, t, o);
            fill(
                pixmap,
                polyline(
                    cell,
                    &[
                        (6.5, 9.5),
                        (13.5, 16.5),
                        (13.0, 17.0),
                        (7.0, 17.0),
                        (3.0, 13.0),
                    ],
                    true,
                ),
                ink,
                t,
                o,
            );
        }
        Action::Tool(ToolKind::Select) => {
            let pts = [
                (6.0, 3.0),
                (6.0, 16.0),
                (9.2, 12.8),
                (11.6, 17.6),
                (13.6, 16.6),
                (11.2, 11.9),
                (15.5, 11.9),
            ];
            fill(pixmap, polyline(cell, &pts, true), ink, t, o);
        }
        Action::Color(i) => {
            let c = cell.center();
            let color = look.colors.get(i as usize).copied().unwrap_or(Rgba::BLACK);
            if i as usize == look.color_index {
                stroke(
                    pixmap,
                    PathBuilder::from_circle(c.x, c.y, 10.0),
                    ink,
                    2.0,
                    t,
                    o,
                );
            }
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 7.5), color, t, o);
            if color.red + color.green + color.blue < 0.4 {
                stroke(
                    pixmap,
                    PathBuilder::from_circle(c.x, c.y, 7.5),
                    ink.with_alpha(0.6),
                    1.2,
                    t,
                    o,
                );
            }
        }
        Action::Thinner | Action::Thicker => {
            let c = cell.center();
            let thick = action == Action::Thicker;
            let r = if thick { 4.5 } else { 1.8 };
            fill(
                pixmap,
                PathBuilder::from_circle(c.x - 3.0, c.y, r),
                ink,
                t,
                o,
            );
            // Small plus or minus at the right.
            line(pixmap, &[(14.0, 10.0), (19.0, 10.0)], 1.6);
            if thick {
                line(pixmap, &[(16.5, 7.5), (16.5, 12.5)], 1.6);
            }
        }
        Action::Undo | Action::Redo => {
            let flip = action == Action::Redo;
            let x = |v: f32| if flip { 20.0 - v } else { v };
            let curve = icon_path(cell, |pb, at| {
                let (a, b) = (at(x(15.5), 15.5), at(x(15.5), 7.5));
                let (c, d) = (at(x(6.0), 7.5), at(x(15.5), 15.5));
                pb.move_to(a.0, a.1);
                pb.quad_to(b.0, b.1, c.0, c.1);
                let _ = d;
            });
            stroke(pixmap, curve, ink, 2.0, t, o);
            fill(
                pixmap,
                polyline(cell, &[(x(3.0), 7.5), (x(8.0), 3.5), (x(8.0), 11.5)], true),
                ink,
                t,
                o,
            );
        }
        Action::Clear => {
            line(pixmap, &[(3.5, 6.0), (16.5, 6.0)], 1.8);
            line(
                pixmap,
                &[(8.0, 6.0), (8.5, 3.5), (11.5, 3.5), (12.0, 6.0)],
                1.6,
            );
            stroke(
                pixmap,
                polyline(
                    cell,
                    &[(5.5, 6.0), (6.5, 17.0), (13.5, 17.0), (14.5, 6.0)],
                    false,
                ),
                ink,
                1.8,
                t,
                o,
            );
            line(pixmap, &[(10.0, 9.0), (10.0, 14.0)], 1.4);
        }
        Action::Ghost => {
            let body = icon_path(cell, |pb, at| {
                let p = |x, y| at(x, y);
                let (a, b, c) = (p(4.0, 17.0), p(4.0, 3.0), p(16.0, 3.0));
                pb.move_to(a.0, a.1);
                pb.line_to(p(4.0, 9.0).0, p(4.0, 9.0).1);
                pb.cubic_to(b.0, b.1, c.0, c.1, p(16.0, 9.0).0, p(16.0, 9.0).1);
                for (x, y) in [
                    (16.0, 17.0),
                    (14.0, 15.0),
                    (12.0, 17.0),
                    (10.0, 15.0),
                    (8.0, 17.0),
                    (6.0, 15.0),
                ] {
                    let q = p(x, y);
                    pb.line_to(q.0, q.1);
                }
                pb.close();
            });
            fill(
                pixmap,
                body,
                if look.ghost { Rgba::WHITE } else { ink },
                t,
                o,
            );
            let c = cell.center();
            for dx in [-2.5, 2.5] {
                fill(
                    pixmap,
                    PathBuilder::from_circle(c.x + dx, c.y - 1.5, 1.4),
                    BACKGROUND.with_alpha(1.0),
                    t,
                    o,
                );
            }
        }
        Action::Whiteboard => {
            stroke(
                pixmap,
                rounded(
                    Rect::new(cell.center().x - 8.0, cell.center().y - 7.0, 16.0, 11.0),
                    1.5,
                ),
                ink,
                1.8,
                t,
                o,
            );
            line(pixmap, &[(7.0, 14.0), (5.0, 18.0)], 1.6);
            line(pixmap, &[(13.0, 14.0), (15.0, 18.0)], 1.6);
        }
        Action::Help => fill(pixmap, glyph(cell, "?", 17.0), ink, t, o),
        Action::TextSmaller | Action::TextBigger => {
            let bigger = action == Action::TextBigger;
            let a = glyph(
                Rect::new(cell.x - 3.0, cell.y, cell.width, cell.height),
                "A",
                if bigger { 19.0 } else { 13.0 },
            );
            fill(pixmap, a, ink, t, o);
            line(pixmap, &[(15.5, 10.0), (20.0, 10.0)], 1.6);
            if bigger {
                line(pixmap, &[(17.75, 7.75), (17.75, 12.25)], 1.6);
            }
        }
        Action::Bold | Action::Italic => {
            let style = TextStyle {
                bold: action == Action::Bold,
                italic: action == Action::Italic,
                ..TextStyle::default()
            };
            let letter = if action == Action::Bold { "B" } else { "I" };
            if let Some(path) = text::path_styled(letter, 18.0, Point::default(), &style) {
                let b = path.bounds();
                let c = cell.center();
                let centered = path.transform(Transform::from_translate(
                    c.x - (b.left() + b.right()) / 2.0,
                    c.y - (b.top() + b.bottom()) / 2.0,
                ));
                fill(pixmap, centered, ink, t, o);
            }
        }
        Action::Tool(ToolKind::Laser) => {
            let c = cell.center();
            let red = Rgba::rgb(1.0, 0.25, 0.25);
            fill(
                pixmap,
                PathBuilder::from_circle(c.x + 3.0, c.y - 3.0, 6.5),
                red.with_alpha(0.3),
                t,
                o,
            );
            fill(
                pixmap,
                PathBuilder::from_circle(c.x + 3.0, c.y - 3.0, 3.0),
                red,
                t,
                o,
            );
            stroke(
                pixmap,
                polyline(cell, &[(3.0, 17.0), (10.5, 9.5)], false),
                ink,
                2.4,
                t,
                o,
            );
        }
        Action::Spotlight => {
            let c = cell.center();
            fill(
                pixmap,
                rounded(cell.inset(5.0), 4.0),
                ink.with_alpha(0.35),
                t,
                o,
            );
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 5.0), ink, t, o);
        }
        Action::Freeze => {
            // A snowflake: three crossing lines with small ticks.
            let c = cell.center();
            for k in 0..3 {
                let a = k as f32 * std::f32::consts::PI / 3.0 + std::f32::consts::FRAC_PI_2;
                let (s, co) = a.sin_cos();
                let mut pb = PathBuilder::new();
                pb.move_to(c.x - co * 8.0, c.y - s * 8.0);
                pb.line_to(c.x + co * 8.0, c.y + s * 8.0);
                stroke(pixmap, pb.finish(), ink, 1.8, t, o);
            }
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 2.0), ink, t, o);
        }
        Action::Snapshot => {
            // A camera.
            stroke(
                pixmap,
                rounded(Rect::new(cell.x + 7.0, cell.y + 11.0, 18.0, 13.0), 2.5),
                ink,
                1.8,
                t,
                o,
            );
            fill(
                pixmap,
                polyline(
                    cell,
                    &[(7.0, 4.5), (13.0, 4.5), (14.5, 7.0), (5.5, 7.0)],
                    true,
                ),
                ink,
                t,
                o,
            );
            let c = Point::new(cell.x + 16.0, cell.y + 17.5);
            stroke(
                pixmap,
                PathBuilder::from_circle(c.x, c.y, 3.5),
                ink,
                1.8,
                t,
                o,
            );
        }
        Action::SelectAll => {
            let dashed = Stroke {
                width: 1.6,
                dash: tiny_skia::StrokeDash::new(vec![2.5, 2.0], 0.0),
                ..Stroke::default()
            };
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color(skia_color(ink, o));
            if let Some(path) = rounded(
                Rect::new(cell.center().x - 7.5, cell.center().y - 7.5, 15.0, 15.0),
                2.0,
            ) {
                pixmap.stroke_path(&path, &paint, &dashed, t, None);
            }
        }
        Action::BringToFront => {
            stroke(
                pixmap,
                rounded(Rect::new(cell.x + 6.0, cell.y + 12.0, 11.0, 11.0), 1.5),
                ink.with_alpha(0.5),
                1.6,
                t,
                o,
            );
            fill(
                pixmap,
                rounded(Rect::new(cell.x + 12.0, cell.y + 7.0, 12.0, 12.0), 1.5),
                ink,
                t,
                o,
            );
        }
        Action::DeleteSelection => {
            line(pixmap, &[(5.0, 5.0), (15.0, 15.0)], 2.2);
            line(pixmap, &[(15.0, 5.0), (5.0, 15.0)], 2.2);
        }
        Action::Escape | Action::ToggleDrawing | Action::ExitGhost | Action::Quit => {}
    }
}

// MARK: tooltip and cheat sheet

/// A small dark label with `text`, sized to fit.
pub fn render_tooltip(label: &str, scale: f32, order: ChannelOrder) -> Option<Pixmap> {
    let size = 13.0;
    let glyphs = text::path(label, size, Point::new(10.0, 6.0))?;
    let b = glyphs.bounds();
    let (w, h) = (b.right() + 10.0, text::line_height(size) + 12.0);
    let mut pixmap = Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)?;
    let t = Transform::from_scale(scale, scale);
    let mut target = pixmap.as_mut();
    fill(
        &mut target,
        rounded(Rect::new(0.0, 0.0, w, h), 6.0),
        Rgba::rgb(0.1, 0.1, 0.11).with_alpha(0.96),
        t,
        order,
    );
    fill(&mut target, Some(glyphs), INK, t, order);
    Some(pixmap)
}

/// Every shortcut that has a key, in two columns, plus the pen modifiers.
pub fn render_cheat_sheet(
    lang: Lang,
    shortcuts: &Shortcuts,
    scale: f32,
    order: ChannelOrder,
) -> Option<Pixmap> {
    // Nine color rows read as noise: show them as one "1–9" row when all are bound.
    let colors: Vec<_> = shortcuts
        .bindings
        .iter()
        .filter(|b| matches!(b.action, Action::Color(_)))
        .filter_map(|b| b.chord)
        .collect();
    let fold_colors = colors.len() == 9;
    let mut rows: Vec<(String, String)> = Vec::new();
    for b in &shortcuts.bindings {
        let Some(c) = b.chord else { continue };
        match b.action {
            Action::Color(0) if fold_colors => rows.push((
                format!("{}–{}", chord_label(colors[0]), chord_label(colors[8])),
                if lang == Lang::Ru {
                    "Цвета"
                } else {
                    "Colors"
                }
                .to_string(),
            )),
            Action::Color(_) if fold_colors => {}
            action => rows.push((chord_label(c), lang.action(action))),
        }
    }
    let (title_size, size, row_h, pad) = (20.0, 14.0, 24.0, 24.0);
    let (key_w, name_w, col_gap) = (150.0, 230.0, 32.0);
    let per_column = rows.len().div_ceil(2);
    let col_w = key_w + name_w;
    let w = pad * 2.0 + col_w * 2.0 + col_gap;
    let note_y = pad + 40.0 + per_column as f32 * row_h + 12.0;
    let h = note_y + 2.0 * row_h + pad;
    let mut pixmap = Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)?;
    let t = Transform::from_scale(scale, scale);
    let mut target = pixmap.as_mut();
    fill(
        &mut target,
        rounded(Rect::new(0.0, 0.0, w, h), 14.0),
        BACKGROUND,
        t,
        order,
    );
    fill(
        &mut target,
        text::path(lang.t(T::CheatSheet), title_size, Point::new(pad, pad)),
        INK,
        t,
        order,
    );
    for (i, (key, name)) in rows.iter().enumerate() {
        let (col, row) = (i / per_column, i % per_column);
        let x = pad + col as f32 * (col_w + col_gap);
        let y = pad + 40.0 + row as f32 * row_h;
        fill(
            &mut target,
            text::path(key, size, Point::new(x, y)),
            ACCENT.with_alpha(1.0),
            t,
            order,
        );
        fill(
            &mut target,
            text::path(name, size, Point::new(x + key_w, y)),
            INK,
            t,
            order,
        );
    }
    let muted = INK.with_alpha(0.65);
    fill(
        &mut target,
        text::path(lang.t(T::WithPen), 13.0, Point::new(pad, note_y)),
        muted,
        t,
        order,
    );
    fill(
        &mut target,
        text::path(
            lang.t(T::CheatSheetClose),
            13.0,
            Point::new(pad, note_y + row_h),
        ),
        muted,
        t,
        order,
    );
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look() -> Look {
        Look {
            tool: ToolKind::Pen,
            bold: false,
            italic: false,
            spotlight: false,
            frozen: false,
            color_index: 0,
            colors: Rgba::PALETTE,
            ghost: false,
            whiteboard: false,
        }
    }

    #[test]
    fn item_names_round_trip() {
        for item in Item::all() {
            assert_eq!(Item::from_name(&item.name()), Some(item), "{item:?}");
        }
        assert_eq!(
            Item::from_name("quit"),
            None,
            "global actions don't go on the palette"
        );
    }

    #[test]
    fn layout_and_hit_testing_in_both_orientations() {
        for vertical in [false, true] {
            let p = Palette {
                items: Item::defaults(),
                vertical,
                hover: None,
            };
            let (cells, grip, (w, h)) = p.layout();
            assert_eq!(cells.len(), p.items.len());
            assert!(if vertical { h > w } else { w > h });
            assert_eq!(p.hit(grip.center()), Some(Hit::Grip));
            for (i, c) in cells.iter().enumerate() {
                assert_eq!(p.hit(c.center()), Some(Hit::Item(i)));
            }
        }
    }

    #[test]
    fn renders_every_item_and_the_extras() {
        let p = Palette {
            items: Item::all(),
            vertical: false,
            hover: Some(3),
        };
        let (w, h) = p.size_px(2.0);
        let mut pixmap = Pixmap::new(w, h).unwrap();
        p.render(&mut pixmap.as_mut(), 2.0, ChannelOrder::Rgba, &look());
        let (cells, _, _) = p.layout();
        // Every cell has some ink brighter than the background.
        for (item, c) in p.items.iter().zip(cells) {
            let c = c.center();
            let lit = (-10..=10).any(|dx| {
                (-10..=10).any(|dy| {
                    let px = pixmap.pixel(
                        ((c.x + dx as f32) * 2.0) as u32,
                        ((c.y + dy as f32) * 2.0) as u32,
                    );
                    px.is_some_and(|px| px.red().max(px.green()).max(px.blue()) > 120)
                })
            });
            assert!(lit, "{item:?} has no icon");
        }
        assert!(render_tooltip("Pen  ·  P", 1.5, ChannelOrder::Rgba).is_some());
        let sheet =
            render_cheat_sheet(Lang::Ru, &Shortcuts::default(), 1.0, ChannelOrder::Rgba).unwrap();
        assert!(sheet.width() > 700 && sheet.height() > 300);
        if let Ok(dir) = std::env::var("SCREENPEN_SELFTEST") {
            pixmap.save_png(format!("{dir}/palette.png")).unwrap();
            sheet.save_png(format!("{dir}/cheatsheet.png")).unwrap();
        }
    }

    #[test]
    fn tooltip_text() {
        let s = Shortcuts::default();
        assert_eq!(
            Item::Action(Action::Tool(ToolKind::Pen)).tooltip(Lang::Ru, &s),
            "Перо  ·  P"
        );
        assert_eq!(
            Item::Action(Action::Help).tooltip(Lang::En, &s),
            "Shortcuts  ·  ?"
        );
        assert_eq!(Item::Settings.tooltip(Lang::En, &s), "Settings");
    }
}
