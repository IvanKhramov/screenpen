//! The floating palette, its tooltips and the shortcut cheat sheet, drawn with
//! tiny-skia. Layout is in points; rendering takes the monitor scale.

use screenpen_core::render::{ChannelOrder, skia_color};
use screenpen_core::shortcuts::WIDTH_PRESETS;
use screenpen_core::{Action, Point, Rect, Rgba, Shortcuts, TextStyle, ToolKind, text};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, Stroke, Transform,
};

use crate::i18n::{Lang, T};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Action(Action),
    Settings,
    /// The shape tools behind one button; a row next to the palette picks the shape.
    Shapes,
    /// Opens a row with the rarer actions.
    More,
    /// Stops drawing.
    Close,
    /// The current text size, shown between A- and A+.
    TextSize,
}

const SHAPES: [ToolKind; 5] = [
    ToolKind::Line,
    ToolKind::Arrow,
    ToolKind::Rectangle,
    ToolKind::Ellipse,
    ToolKind::Pixelate,
];

const MORE: [Item; 5] = [
    Item::Action(Action::Whiteboard),
    Item::Action(Action::Freeze),
    Item::Action(Action::Snapshot),
    Item::Action(Action::Help),
    Item::Settings,
];

impl Item {
    pub fn defaults() -> Vec<Item> {
        let mut items = vec![Item::Action(Action::Ghost)];
        items.extend(
            [
                ToolKind::Pen,
                ToolKind::Highlighter,
                ToolKind::Text,
                ToolKind::Badge,
                ToolKind::Eraser,
                ToolKind::Select,
            ]
            .map(|t| Item::Action(Action::Tool(t))),
        );
        items.insert(3, Item::Shapes);
        items.extend(
            [
                Action::Tool(ToolKind::Laser),
                Action::Spotlight,
                Action::Magnifier,
            ]
            .map(Item::Action),
        );
        items.extend((0..6).map(|i| Item::Action(Action::Color(i))));
        items.extend((0..3).map(|i| Item::Action(Action::Width(i))));
        items.extend([Action::Undo, Action::Redo, Action::Clear].map(Item::Action));
        items.extend([Item::More, Item::Close]);
        items
    }

    /// Everything that can be on the palette, for the settings window.
    pub fn all() -> Vec<Item> {
        let mut items = vec![Item::Action(Action::Ghost)];
        items.extend(ToolKind::ALL.iter().map(|&t| Item::Action(Action::Tool(t))));
        items.push(Item::Shapes);
        items.extend([Action::Spotlight, Action::Magnifier].map(Item::Action));
        items.extend((0..9).map(|i| Item::Action(Action::Color(i))));
        items.extend((0..3).map(|i| Item::Action(Action::Width(i))));
        items.extend(
            [
                Action::Thinner,
                Action::Thicker,
                Action::TextSmaller,
                Action::TextBigger,
                Action::Bold,
                Action::Italic,
                Action::Undo,
                Action::Redo,
                Action::Clear,
                Action::SelectAll,
                Action::BringToFront,
                Action::DeleteSelection,
                Action::Whiteboard,
                Action::Freeze,
                Action::Snapshot,
                Action::Help,
            ]
            .map(Item::Action),
        );
        items.extend([Item::Settings, Item::More, Item::Close]);
        items
    }

    pub fn name(self) -> String {
        match self {
            Item::Action(a) => a.name(),
            Item::Settings => "settings".to_string(),
            Item::Shapes => "shapes".to_string(),
            Item::More => "more".to_string(),
            Item::Close => "close".to_string(),
            Item::TextSize => "text_size".to_string(),
        }
    }

    pub fn from_name(name: &str) -> Option<Item> {
        match name {
            "settings" => return Some(Item::Settings),
            "shapes" => return Some(Item::Shapes),
            "more" => return Some(Item::More),
            "close" => return Some(Item::Close),
            _ => {}
        }
        Action::from_name(name)
            .filter(|a| !a.is_global())
            .map(Item::Action)
    }

    /// Neighbours from different groups get a separator between them.
    fn group(self) -> u8 {
        match self {
            Item::Action(Action::Ghost) => 0,
            Item::Action(Action::Tool(ToolKind::Laser)) => 2,
            Item::Action(Action::Tool(_)) | Item::Shapes => 1,
            Item::Action(Action::Spotlight | Action::Magnifier) => 2,
            Item::Action(Action::Color(_) | Action::Width(_)) => 3,
            Item::Action(Action::Thinner | Action::Thicker) => 3,
            Item::Action(Action::TextSmaller | Action::TextBigger) | Item::TextSize => 7,
            Item::Action(Action::Bold | Action::Italic) => 8,
            Item::Action(Action::Undo | Action::Redo | Action::Clear) => 4,
            Item::Action(
                Action::SelectAll | Action::BringToFront | Action::DeleteSelection | Action::Escape,
            ) => 5,
            Item::Action(Action::Whiteboard | Action::Freeze | Action::Snapshot) => 9,
            _ => 6,
        }
    }

    /// The hit area along the row: swatches and width dots are narrower than buttons.
    fn extent(self) -> f32 {
        match self {
            Item::Action(Action::Color(_)) => SWATCH,
            Item::Action(Action::Width(_)) => WIDTH_CELL,
            Item::TextSize => 30.0,
            _ => CELL,
        }
    }

    pub fn label(self, lang: Lang) -> String {
        let ru = lang == Lang::Ru;
        match self {
            Item::Action(a) => lang.action(a),
            Item::Settings => lang.t(T::Settings).trim_end_matches('…').to_string(),
            Item::Shapes => if ru { "Фигуры" } else { "Shapes" }.to_string(),
            Item::More => if ru { "Ещё" } else { "More" }.to_string(),
            Item::Close => if ru {
                "Закончить рисование"
            } else {
                "Stop drawing"
            }
            .to_string(),
            Item::TextSize => if ru {
                "Размер текста"
            } else {
                "Text size"
            }
            .to_string(),
        }
    }

    /// The shortcut that does what this item does.
    fn action(self) -> Option<Action> {
        match self {
            Item::Action(a) => Some(a),
            Item::Close => Some(Action::ToggleDrawing),
            _ => None,
        }
    }

    /// "Pen  ·  P", or just the name when there's no shortcut.
    pub fn tooltip(self, lang: Lang, shortcuts: &Shortcuts) -> String {
        let label = self.label(lang);
        match self.action().and_then(|a| shortcuts.chord(a)) {
            Some(c) => format!("{label}  ·  {}", chord_label(c)),
            None => label,
        }
    }

    /// The short key shown in the button's corner: single keys only.
    fn badge(self, shortcuts: &Shortcuts) -> Option<String> {
        let c = shortcuts.chord(self.action()?)?;
        if c.ctrl || c.alt || matches!(self, Item::Action(Action::Color(_))) {
            return None;
        }
        let label = chord_label(c);
        (label.chars().count() == 1).then_some(label)
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// The colors of one theme.
struct Skin {
    background: Rgba,
    border: Rgba,
    ink: Rgba,
    hover: Rgba,
    separator: Rgba,
    shadow: Rgba,
}

impl Theme {
    fn skin(self) -> Skin {
        match self {
            Theme::Dark => Skin {
                background: Rgba::rgb(0.125, 0.125, 0.14).with_alpha(0.96),
                border: Rgba::WHITE.with_alpha(0.09),
                ink: Rgba::rgb(0.93, 0.93, 0.95),
                hover: Rgba::WHITE.with_alpha(0.1),
                separator: Rgba::WHITE.with_alpha(0.12),
                shadow: Rgba::BLACK.with_alpha(0.32),
            },
            Theme::Light => Skin {
                background: Rgba::rgb(0.988, 0.988, 0.992).with_alpha(0.98),
                border: Rgba::BLACK.with_alpha(0.08),
                ink: Rgba::rgb(0.15, 0.15, 0.17),
                hover: Rgba::BLACK.with_alpha(0.06),
                separator: Rgba::BLACK.with_alpha(0.1),
                shadow: Rgba::rgb(0.08, 0.12, 0.2).with_alpha(0.2),
            },
        }
    }

    /// "system", "dark" or "light" from the settings; "system" asks the OS.
    pub fn from_setting(setting: &str) -> Theme {
        match setting {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ if crate::theme::system_is_light() => Theme::Light,
            _ => Theme::Dark,
        }
    }
}

/// What the palette highlights.
pub struct Look {
    pub tool: ToolKind,
    pub bold: bool,
    pub italic: bool,
    pub spotlight: bool,
    pub magnifier: bool,
    pub frozen: bool,
    pub color_index: usize,
    pub colors: [Rgba; 9],
    pub ghost: bool,
    pub whiteboard: bool,
    pub width: f32,
    pub text_size: f32,
    pub theme: Theme,
    /// Shortcut letters in the button corners; None hides them.
    pub badges: Option<Shortcuts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Grip,
    Item(usize),
}

/// Button size, swatch and width-dot widths, inner padding, separator gap.
const CELL: f32 = 36.0;
const SWATCH: f32 = 28.0;
const WIDTH_CELL: f32 = 24.0;
const PAD: f32 = 5.0;
const GAP: f32 = 9.0;
const GRIP: f32 = 12.0;
/// Room around the bars for their shadow.
const MARGIN: f32 = 8.0;
/// Between the main bar and the context row.
const ROW_GAP: f32 = 6.0;
const CONTEXT_CELL: f32 = 32.0;
const ACCENT: Rgba = Rgba::rgb(0.04, 0.52, 1.0);

pub struct Palette {
    pub items: Vec<Item>,
    pub vertical: bool,
    pub hover: Option<usize>,
    /// The "More" row is open.
    pub more_open: bool,
    /// The context row goes before the main bar (above it, or left of it when vertical);
    /// the platform sets this from where the palette sits on the screen.
    pub context_first: bool,
    /// The row under or over the main bar, from the last `sync`.
    context: Vec<Item>,
    /// What the Shapes button picks.
    last_shape: ToolKind,
}

/// What a click on a palette item asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Run this action, like its shortcut does.
    Run(Action),
    Settings,
    /// Nothing beyond redrawing the palette.
    None,
}

/// Where everything is, in points.
struct Frame {
    cells: Vec<Rect>,
    grip: Rect,
    main: Rect,
    context: Option<Rect>,
    separators: Vec<Rect>,
    size: (f32, f32),
}

impl Palette {
    pub fn new(items: Vec<Item>, vertical: bool) -> Palette {
        Palette {
            items,
            vertical,
            hover: None,
            more_open: false,
            context_first: true,
            context: Vec::new(),
            last_shape: ToolKind::Arrow,
        }
    }

    /// Handles a click on item `i`. The More row closes on any other click.
    pub fn press(&mut self, i: usize, look: &Look) -> Press {
        let item = self.item(i);
        if item == Item::More {
            self.more_open = !self.more_open;
            return Press::None;
        }
        self.more_open = false;
        match item {
            Item::Shapes if SHAPES.contains(&look.tool) => Press::None,
            Item::Shapes => Press::Run(Action::Tool(self.last_shape)),
            Item::Close => Press::Run(Action::ToggleDrawing),
            Item::Settings => Press::Settings,
            Item::Action(a) => Press::Run(a),
            Item::More | Item::TextSize => Press::None,
        }
    }

    /// Main bar items, then the context row's.
    pub fn item(&self, i: usize) -> Item {
        self.items
            .get(i)
            .or_else(|| self.context.get(i - self.items.len()))
            .copied()
            .unwrap_or(Item::TextSize)
    }

    /// Updates the context row for the current tool; returns whether it changed.
    pub fn sync(&mut self, look: &Look) -> bool {
        let shape = SHAPES.contains(&look.tool);
        if shape {
            self.last_shape = look.tool;
        }
        let wanted: Vec<Item> = if self.more_open {
            MORE.to_vec()
        } else if shape && self.items.contains(&Item::Shapes) {
            SHAPES.map(|t| Item::Action(Action::Tool(t))).to_vec()
        } else if look.tool == ToolKind::Text {
            vec![
                Item::Action(Action::TextSmaller),
                Item::TextSize,
                Item::Action(Action::TextBigger),
                Item::Action(Action::Bold),
                Item::Action(Action::Italic),
            ]
        } else {
            Vec::new()
        };
        let wanted: Vec<Item> = wanted
            .into_iter()
            .filter(|i| *i == Item::TextSize || !self.items.contains(i))
            .collect();
        let changed = wanted != self.context;
        self.context = wanted;
        changed
    }

    /// Where the main bar's top-left corner is inside the window, in points;
    /// it moves when the context row opens before it.
    pub fn main_offset(&self) -> (f32, f32) {
        let m = self.frame().main;
        (m.x, m.y)
    }

    fn frame(&self) -> Frame {
        let mut cells = Vec::with_capacity(self.items.len() + self.context.len());
        let mut separators = Vec::new();
        let across = CELL + 2.0 * PAD;
        let ctx_across = CONTEXT_CELL + 2.0 * 4.0;
        let ctx_shift = if self.context.is_empty() || !self.context_first {
            0.0
        } else {
            ctx_across + ROW_GAP
        };
        let place = |along: f32, across_at: f32, len: f32, thick: f32| {
            if self.vertical {
                Rect::new(across_at, along, thick, len)
            } else {
                Rect::new(along, across_at, len, thick)
            }
        };

        // Main bar.
        let base = MARGIN + ctx_shift;
        let mut along = MARGIN + PAD + GRIP;
        let mut previous: Option<u8> = None;
        for item in &self.items {
            if previous.is_some_and(|g| g != item.group()) {
                separators.push(place(
                    along + GAP / 2.0 - 0.5,
                    base + PAD + 8.0,
                    1.0,
                    CELL - 16.0,
                ));
                along += GAP;
            }
            previous = Some(item.group());
            // Vertical bars give every item a full square.
            let len = if self.vertical { CELL } else { item.extent() };
            cells.push(place(along, base + PAD, len, CELL));
            along += len;
        }
        let main_len = along + PAD - MARGIN;
        let main = place(MARGIN, base, main_len, across);
        let grip = place(MARGIN, base, PAD + GRIP, across);

        // Context row, centered on the main bar.
        let mut context = None;
        if !self.context.is_empty() {
            let mut len = 8.0;
            let mut previous: Option<u8> = None;
            let mut offsets = Vec::new();
            for item in &self.context {
                if previous.is_some_and(|g| g != item.group()) {
                    offsets.push((len, None));
                    len += GAP;
                }
                previous = Some(item.group());
                let l = if *item == Item::TextSize {
                    30.0
                } else {
                    CONTEXT_CELL
                };
                offsets.push((len, Some(l)));
                len += l;
            }
            len += 4.0;
            let start = MARGIN + ((main_len - len) / 2.0).max(0.0);
            let across_at = if self.context_first {
                MARGIN
            } else {
                MARGIN + across + ROW_GAP
            };
            for (at, l) in offsets {
                match l {
                    Some(l) => cells.push(place(start + at, across_at + 4.0, l, CONTEXT_CELL)),
                    None => separators.push(place(
                        start + at + GAP / 2.0 - 0.5,
                        across_at + 4.0 + 7.0,
                        1.0,
                        CONTEXT_CELL - 14.0,
                    )),
                }
            }
            context = Some(place(start, across_at, len, ctx_across));
        }

        let along_total = MARGIN * 2.0
            + main_len.max(context.map_or(0.0, |c| if self.vertical { c.height } else { c.width }));
        let across_total = MARGIN * 2.0
            + across
            + if self.context.is_empty() {
                0.0
            } else {
                ctx_across + ROW_GAP
            };
        let size = if self.vertical {
            (across_total, along_total)
        } else {
            (along_total, across_total)
        };
        Frame {
            cells,
            grip,
            main,
            context,
            separators,
            size,
        }
    }

    /// Cell rectangles (main bar, then context row) and the grip, in points,
    /// plus the overall size.
    pub fn layout(&self) -> (Vec<Rect>, Rect, (f32, f32)) {
        let f = self.frame();
        (f.cells, f.grip, f.size)
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
        let skin = look.theme.skin();
        let f = self.frame();
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        for (bar, radius) in [(Some(f.main), 14.0), (f.context, 12.0)] {
            let Some(bar) = bar else { continue };
            // A soft shadow from a few widening translucent layers.
            for k in 1..=6 {
                let spread = k as f32 * 1.2;
                fill(
                    pixmap,
                    rounded(
                        Rect::new(
                            bar.x - spread,
                            bar.y - spread + 2.0,
                            bar.width + 2.0 * spread,
                            bar.height + 2.0 * spread,
                        ),
                        radius + spread,
                    ),
                    skin.shadow.with_alpha(skin.shadow.alpha / 7.0),
                    t,
                    order,
                );
            }
            fill(pixmap, rounded(bar, radius), skin.background, t, order);
            stroke(
                pixmap,
                rounded(bar.inset(0.5), radius - 0.5),
                skin.border,
                1.0,
                t,
                order,
            );
        }
        for s in &f.separators {
            fill(pixmap, rounded(*s, 0.0), skin.separator, t, order);
        }

        // Grip: two columns of dots.
        let c = f.grip.center();
        for i in -1..=1 {
            for j in [-1.0, 1.0] {
                let (x, y) = if self.vertical {
                    (c.x + i as f32 * 4.0, c.y + j * 2.5)
                } else {
                    (c.x + j * 2.5, c.y + i as f32 * 4.0)
                };
                fill(
                    pixmap,
                    PathBuilder::from_circle(x, y, 1.2),
                    skin.ink.with_alpha(0.4),
                    t,
                    order,
                );
            }
        }

        for (i, cell) in f.cells.iter().enumerate() {
            let item = self.item(i);
            let active = match item {
                Item::Action(Action::Tool(tool)) => tool == look.tool,
                Item::Shapes => SHAPES.contains(&look.tool),
                Item::Action(Action::Ghost) => look.ghost,
                Item::Action(Action::Whiteboard) => look.whiteboard,
                Item::Action(Action::Spotlight) => look.spotlight,
                Item::Action(Action::Magnifier) => look.magnifier,
                Item::Action(Action::Bold) => look.bold,
                Item::Action(Action::Italic) => look.italic,
                Item::Action(Action::Freeze) => look.frozen,
                Item::More => self.more_open,
                _ => false,
            };
            let button = match item {
                Item::Action(Action::Color(_)) | Item::TextSize => None,
                Item::Action(Action::Width(_)) => Some(cell.inset(1.0)),
                _ => Some(cell.inset(1.5)),
            };
            let mut ink = skin.ink;
            // In mouse mode the tool waits: an outline instead of a fill.
            let waiting = active
                && look.ghost
                && matches!(item, Item::Action(Action::Tool(_)) | Item::Shapes);
            if let (Some(b), true) = (button, waiting) {
                stroke(pixmap, rounded(b.inset(0.75), 8.5), ACCENT, 1.5, t, order);
            } else if let Some(b) = button {
                if active {
                    fill(pixmap, rounded(b, 9.0), ACCENT, t, order);
                    ink = Rgba::WHITE;
                } else if self.hover == Some(i) {
                    fill(pixmap, rounded(b, 9.0), skin.hover, t, order);
                }
            }
            if let Item::Action(Action::Width(w)) = item {
                let on = WIDTH_PRESETS.get(w as usize) == Some(&look.width);
                let c = cell.center();
                if on {
                    fill(
                        pixmap,
                        rounded(cell.inset(1.0), 8.0),
                        ACCENT.with_alpha(0.18),
                        t,
                        order,
                    );
                }
                let r = [2.0, 3.5, 5.5][w as usize % 3];
                fill(
                    pixmap,
                    PathBuilder::from_circle(c.x, c.y, r),
                    if on { ACCENT } else { ink },
                    t,
                    order,
                );
                continue;
            }
            if item == Item::TextSize {
                let label = format!("{}", look.text_size.round() as i32);
                fill(pixmap, glyph(*cell, &label, 12.5), ink, t, order);
                continue;
            }
            draw_icon(pixmap, item, *cell, look, ink, skin.background, t, order);
            if let Some(key) = look.badges.as_ref().and_then(|s| item.badge(s)) {
                let p = text::path(&key, 7.5, Point::default());
                if let Some(p) = p {
                    let b = p.bounds();
                    let p = p.transform(Transform::from_translate(
                        cell.right() - 3.5 - b.right(),
                        cell.bottom() - 2.5 - b.bottom(),
                    ));
                    fill(
                        pixmap,
                        p,
                        ink.with_alpha(if active { 0.85 } else { 0.5 }),
                        t,
                        order,
                    );
                }
            }
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

#[allow(clippy::too_many_arguments)]
fn draw_icon(
    pixmap: &mut PixmapMut,
    item: Item,
    cell: Rect,
    look: &Look,
    ink: Rgba,
    background: Rgba,
    t: Transform,
    o: ChannelOrder,
) {
    let background = background.with_alpha(1.0);
    let line = |pixmap: &mut PixmapMut, pts: &[(f32, f32)], w: f32| {
        stroke(pixmap, polyline(cell, pts, false), ink, w, t, o)
    };
    let c = cell.center();
    match item {
        Item::Shapes => {
            stroke(
                pixmap,
                rounded(Rect::new(c.x - 8.0, c.y - 1.0, 9.0, 9.0), 1.5),
                ink,
                1.8,
                t,
                o,
            );
            stroke(
                pixmap,
                PathBuilder::from_circle(c.x + 3.0, c.y - 3.0, 5.0),
                ink,
                1.8,
                t,
                o,
            );
            return;
        }
        Item::More => {
            for dx in [-6.0, 0.0, 6.0] {
                fill(
                    pixmap,
                    PathBuilder::from_circle(c.x + dx, c.y, 1.7),
                    ink,
                    t,
                    o,
                );
            }
            return;
        }
        Item::Close => {
            line(pixmap, &[(5.0, 5.0), (15.0, 15.0)], 1.9);
            line(pixmap, &[(15.0, 5.0), (5.0, 15.0)], 1.9);
            return;
        }
        Item::TextSize | Item::Action(Action::Width(_)) => return,
        Item::Action(_) | Item::Settings => {}
    }
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
        Action::Caption => {
            line(pixmap, &[(4.0, 16.0), (10.0, 10.0)], 2.0);
            fill(
                pixmap,
                glyph(Rect::new(6.0, 0.0, 14.0, 12.0), "T", 11.0),
                ink,
                t,
                o,
            );
        }
        Action::Preset(i) | Action::SavePreset(i) => {
            let r = tiny_skia::Rect::from_xywh(3.0, 3.0, 14.0, 14.0)
                .and_then(|r| rounded(Rect::from_skia(r), 3.5));
            stroke(pixmap, r, ink, 1.6, t, o);
            fill(pixmap, glyph(cell, &(i + 1).to_string(), 11.0), ink, t, o);
        }
        Action::Tool(ToolKind::Pixelate) => {
            // A checkerboard of blocks.
            let c = cell.center();
            for i in 0..3 {
                for j in 0..3 {
                    let alpha = if (i + j) % 2 == 0 { 1.0 } else { 0.35 };
                    let (x, y) = (c.x - 7.5 + i as f32 * 5.0, c.y - 7.5 + j as f32 * 5.0);
                    let block =
                        tiny_skia::Rect::from_xywh(x, y, 5.0, 5.0).map(PathBuilder::from_rect);
                    fill(pixmap, block, ink.with_alpha(ink.alpha * alpha), t, o);
                }
            }
        }
        Action::Tool(ToolKind::Text) => fill(pixmap, glyph(cell, "T", 18.0), ink, t, o),
        Action::Tool(ToolKind::Badge) => {
            let c = cell.center();
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 8.0), ink, t, o);
            fill(pixmap, glyph(cell, "1", 11.0), background, t, o);
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
            // A dashed selection frame.
            for (a, b) in [
                ((3.0, 3.0), (7.0, 3.0)),
                ((10.0, 3.0), (13.0, 3.0)),
                ((16.0, 3.0), (17.0, 3.0)),
                ((17.0, 3.0), (17.0, 7.0)),
                ((17.0, 10.0), (17.0, 13.0)),
                ((17.0, 16.0), (17.0, 17.0)),
                ((17.0, 17.0), (13.0, 17.0)),
                ((10.0, 17.0), (7.0, 17.0)),
                ((4.0, 17.0), (3.0, 17.0)),
                ((3.0, 17.0), (3.0, 13.0)),
                ((3.0, 10.0), (3.0, 7.0)),
                ((3.0, 4.0), (3.0, 3.0)),
            ] {
                line(pixmap, &[a, b], 1.7);
            }
        }
        Action::Color(i) => {
            let c = cell.center();
            let color = look.colors.get(i as usize).copied().unwrap_or(Rgba::BLACK);
            if i as usize == look.color_index {
                stroke(
                    pixmap,
                    PathBuilder::from_circle(c.x, c.y, 12.0),
                    ink,
                    2.0,
                    t,
                    o,
                );
            }
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 9.0), color, t, o);
            // A swatch close to the bar's color needs an edge to be seen.
            let luma = |c: Rgba| 0.3 * c.red + 0.59 * c.green + 0.11 * c.blue;
            if (luma(color) - luma(background)).abs() < 0.25 {
                stroke(
                    pixmap,
                    PathBuilder::from_circle(c.x, c.y, 9.0),
                    ink.with_alpha(0.35),
                    1.0,
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
            let pts = [
                (5.0, 2.5),
                (5.0, 16.0),
                (8.4, 12.8),
                (10.9, 17.8),
                (13.0, 16.8),
                (10.6, 11.9),
                (15.2, 11.9),
            ];
            stroke(pixmap, polyline(cell, &pts, true), ink, 1.7, t, o);
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
            // A lit circle with rays.
            let c = cell.center();
            fill(pixmap, PathBuilder::from_circle(c.x, c.y, 4.0), ink, t, o);
            for k in 0..8 {
                let (s, co) = (k as f32 * std::f32::consts::FRAC_PI_4).sin_cos();
                let mut pb = PathBuilder::new();
                pb.move_to(c.x + co * 6.5, c.y + s * 6.5);
                pb.line_to(c.x + co * 9.0, c.y + s * 9.0);
                stroke(pixmap, pb.finish(), ink, 1.7, t, o);
            }
        }
        Action::Magnifier => {
            // A magnifying glass.
            let c = Point::new(cell.x + 14.0, cell.y + 14.0);
            stroke(
                pixmap,
                PathBuilder::from_circle(c.x, c.y, 6.5),
                ink,
                2.2,
                t,
                o,
            );
            stroke(
                pixmap,
                polyline(cell, &[(13.0, 13.0), (17.5, 17.5)], false),
                ink,
                3.0,
                t,
                o,
            );
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
        Action::Escape
        | Action::ToggleDrawing
        | Action::ExitGhost
        | Action::Quit
        | Action::Width(_) => {}
    }
}

// MARK: magnifier lens

/// Lens radius in points, and zoom limits and step for the wheel (no magnifier on Linux).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub const LENS_RADIUS: f32 = 130.0;
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub const LENS_ZOOM: (f32, f32, f32) = (1.5, 6.0, 0.5);

impl Look {
    /// A plain look for previews: pen, the first color, no modes.
    pub fn preview(theme: Theme, colors: [Rgba; 9]) -> Look {
        Look {
            tool: ToolKind::Pen,
            bold: false,
            italic: false,
            spotlight: false,
            magnifier: false,
            frozen: false,
            color_index: 0,
            colors,
            ghost: false,
            whiteboard: false,
            width: WIDTH_PRESETS[1],
            text_size: 28.0,
            theme,
            badges: None,
        }
    }
}

/// One palette button on a transparent square, for the settings window.
pub fn render_item_icon(item: Item, look: &Look, scale: f32) -> Option<Pixmap> {
    let size = (CELL * scale).ceil() as u32;
    let mut pixmap = Pixmap::new(size, size)?;
    let cell = Rect::new(0.0, 0.0, CELL, CELL);
    let skin = look.theme.skin();
    let t = Transform::from_scale(scale, scale);
    let o = ChannelOrder::Rgba;
    let mut target = pixmap.as_mut();
    match item {
        Item::Action(Action::Width(w)) => {
            let c = cell.center();
            let r = [2.0, 3.5, 5.5][w as usize % 3];
            fill(
                &mut target,
                PathBuilder::from_circle(c.x, c.y, r),
                skin.ink,
                t,
                o,
            );
        }
        Item::TextSize => fill(&mut target, glyph(cell, "28", 12.5), skin.ink, t, o),
        _ => draw_icon(
            &mut target,
            item,
            cell,
            look,
            skin.ink,
            skin.background,
            t,
            o,
        ),
    }
    Some(pixmap)
}

/// A cursor that shows the brush: a dot as wide as the stroke, in its color, ringed
/// light and dark so it shows on any background. Returns the image (straight RGBA
/// order as `order` says) and its hot spot; None for tools that keep a system cursor.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn render_cursor(
    tool: ToolKind,
    style: &screenpen_core::Style,
    scale: f32,
    order: ChannelOrder,
) -> Option<(Pixmap, (u32, u32))> {
    if !matches!(
        tool,
        ToolKind::Pen
            | ToolKind::Highlighter
            | ToolKind::Line
            | ToolKind::Arrow
            | ToolKind::Rectangle
            | ToolKind::Ellipse
    ) {
        return None;
    }
    let radius = (style.width * scale / 2.0).clamp(2.5, 60.0);
    let size = (2.0 * radius + 6.0).ceil() as u32;
    let mut pixmap = Pixmap::new(size, size)?;
    let c = size as f32 / 2.0;
    let mut target = pixmap.as_mut();
    let t = Transform::identity();
    let opacity = if tool == ToolKind::Highlighter {
        0.55
    } else {
        1.0
    };
    fill(
        &mut target,
        PathBuilder::from_circle(c, c, radius),
        style.color.with_alpha(opacity),
        t,
        order,
    );
    stroke(
        &mut target,
        PathBuilder::from_circle(c, c, radius + 0.75),
        Rgba::WHITE.with_alpha(0.95),
        1.5,
        t,
        order,
    );
    stroke(
        &mut target,
        PathBuilder::from_circle(c, c, radius + 2.0),
        Rgba::BLACK.with_alpha(0.55),
        1.0,
        t,
        order,
    );
    let hot = size / 2;
    Some((pixmap, (hot, hot)))
}

/// `source` (the screen around the pointer) enlarged into a round lens `diameter`
/// pixels wide, with a thin ring. Byte order follows `source`. Used by the macOS
/// backend; Windows has the system Magnification API, Linux has no magnifier.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn render_lens(source: &Pixmap, diameter: u32) -> Option<Pixmap> {
    let mut lens = Pixmap::new(diameter, diameter)?;
    let d = diameter as f32;
    let mut mask = tiny_skia::Mask::new(diameter, diameter)?;
    let circle = PathBuilder::from_circle(d / 2.0, d / 2.0, d / 2.0 - 1.0)?;
    mask.fill_path(&circle, FillRule::Winding, true, Transform::identity());
    let k = d / source.width().max(1) as f32;
    let paint = tiny_skia::PixmapPaint {
        quality: tiny_skia::FilterQuality::Bilinear,
        ..Default::default()
    };
    lens.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &paint,
        Transform::from_scale(k, k),
        Some(&mask),
    );
    let mut ring = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    ring.set_color(tiny_skia::Color::from_rgba8(255, 255, 255, 220));
    lens.stroke_path(
        &circle,
        &ring,
        &Stroke {
            width: 2.5,
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
    Some(lens)
}

// MARK: tooltip and cheat sheet

const TIP_BACKGROUND: Rgba = Rgba::rgb(0.07, 0.07, 0.08);
const TIP_INK: Rgba = Rgba::rgb(0.95, 0.95, 0.97);

/// A key chip: `key` in a rounded box with its left edge at `x` and text top at `y`.
/// Returns the chip width.
#[allow(clippy::too_many_arguments)]
fn chip(
    target: &mut PixmapMut,
    key: &str,
    size: f32,
    x: f32,
    y: f32,
    fill_color: Rgba,
    ink: Rgba,
    t: Transform,
    order: ChannelOrder,
) -> f32 {
    let Some(glyphs) = text::path(key, size, Point::new(x + 6.0, y)) else {
        return 0.0;
    };
    let b = glyphs.bounds();
    let w = (b.right() - x + 6.0).max(size + 6.0);
    let h = text::line_height(size) + 2.0;
    fill(
        target,
        rounded(Rect::new(x, y - 1.0, w, h), 5.0),
        fill_color,
        t,
        order,
    );
    // Center narrow keys like "E" in their chip.
    let dx = (w - (b.right() - b.left()) - 12.0).max(0.0) / 2.0 + (x + 6.0 - b.left());
    fill(
        target,
        glyphs.transform(Transform::from_translate(dx, 0.0)),
        ink,
        t,
        order,
    );
    w
}

/// Measures `key`'s chip without drawing it.
fn chip_width(key: &str, size: f32) -> f32 {
    text::path(key, size, Point::new(6.0, 0.0))
        .map_or(0.0, |g| (g.bounds().right() + 6.0).max(size + 6.0))
}

/// A small dark label: "Name  ·  Key" shows the key in a chip.
pub fn render_tooltip(label: &str, scale: f32, order: ChannelOrder) -> Option<Pixmap> {
    let size = 12.5;
    let (name, key) = match label.split_once("  ·  ") {
        Some((name, key)) => (name, Some(key)),
        None => (label, None),
    };
    let glyphs = text::path(name, size, Point::new(10.0, 6.0))?;
    let b = glyphs.bounds();
    let key_w = key.map_or(0.0, |k| chip_width(k, 11.0) + 8.0);
    let (w, h) = (
        b.right() + 10.0 + key_w - if key.is_some() { 3.0 } else { 0.0 },
        text::line_height(size) + 12.0,
    );
    let mut pixmap = Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)?;
    let t = Transform::from_scale(scale, scale);
    let mut target = pixmap.as_mut();
    fill(
        &mut target,
        rounded(Rect::new(0.0, 0.0, w, h), 7.0),
        TIP_BACKGROUND.with_alpha(0.96),
        t,
        order,
    );
    fill(&mut target, Some(glyphs), TIP_INK, t, order);
    if let Some(key) = key {
        let y = (h - text::line_height(11.0)) / 2.0;
        chip(
            &mut target,
            key,
            11.0,
            b.right() + 8.0,
            y,
            TIP_INK.with_alpha(0.16),
            TIP_INK,
            t,
            order,
        );
    }
    Some(pixmap)
}

/// `s` cut with "…" to fit `width` at `size`.
fn fit(s: &str, size: f32, width: f32) -> String {
    let w = |s: &str| text::path(s, size, Point::default()).map_or(0.0, |p| p.bounds().right());
    if w(s) <= width {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let cut: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if w(&cut) <= width {
            return cut;
        }
    }
    String::new()
}

/// Which cheat sheet column an action belongs to.
fn sheet_group(a: Action) -> usize {
    match a {
        Action::Tool(ToolKind::Laser) => 1,
        Action::Tool(_) | Action::Color(_) | Action::Thinner | Action::Thicker => 0,
        Action::Ghost
        | Action::Whiteboard
        | Action::Spotlight
        | Action::Magnifier
        | Action::Freeze
        | Action::Snapshot
        | Action::ToggleDrawing
        | Action::ExitGhost
        | Action::Quit
        | Action::Help => 1,
        _ => 2,
    }
}

/// Every shortcut that has a key, grouped in three columns, with the text editing
/// keys and the pen modifiers.
pub fn render_cheat_sheet(
    lang: Lang,
    shortcuts: &Shortcuts,
    scale: f32,
    order: ChannelOrder,
) -> Option<Pixmap> {
    let ru = lang == Lang::Ru;
    // Nine color rows read as noise: show them as one "1–9" row when all are bound.
    let colors: Vec<_> = shortcuts
        .bindings
        .iter()
        .filter(|b| matches!(b.action, Action::Color(_)))
        .filter_map(|b| b.chord)
        .collect();
    let fold_colors = colors.len() == 9;
    let mut columns: [Vec<(String, String)>; 3] = Default::default();
    for b in &shortcuts.bindings {
        let Some(c) = b.chord else { continue };
        let row = match b.action {
            Action::Color(0) if fold_colors => (
                if ru { "Цвета" } else { "Colors" }.to_string(),
                format!("{}–{}", chord_label(colors[0]), chord_label(colors[8])),
            ),
            Action::Color(_) if fold_colors => continue,
            Action::Preset(0) | Action::SavePreset(0) => {
                let last = if b.action == Action::Preset(0) {
                    Action::Preset(2)
                } else {
                    Action::SavePreset(2)
                };
                let range = match shortcuts.chord(last) {
                    Some(end) => format!("{}–{}", chord_label(c), end.key),
                    None => chord_label(c),
                };
                let name = match (b.action, ru) {
                    (Action::Preset(_), true) => "Стили",
                    (Action::Preset(_), false) => "Styles",
                    (_, true) => "Сохранить стиль",
                    (_, false) => "Save style",
                };
                (name.to_string(), range)
            }
            Action::Preset(_) | Action::SavePreset(_) => continue,
            action => (lang.action(action), chord_label(c)),
        };
        columns[sheet_group(b.action)].push(row);
    }
    let text_keys: Vec<(String, String)> = [
        (
            if ru {
                "Выделить всё"
            } else {
                "Select all"
            },
            "Ctrl+A",
        ),
        (
            if ru {
                "Копировать, вырезать"
            } else {
                "Copy, cut"
            },
            "Ctrl+C  X",
        ),
        (if ru { "Вставить" } else { "Paste" }, "Ctrl+V"),
        (if ru { "По словам" } else { "By words" }, "Ctrl+←→"),
        (if ru { "Выделять" } else { "Select" }, "Shift+←→"),
        (if ru { "Готово" } else { "Done" }, "Ctrl+Enter"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let titles = if ru {
        ["Рисование", "Показ", "Правка", "Пока печатаете"]
    } else {
        ["Drawing", "Presenting", "Editing", "While typing"]
    };

    let skin = Theme::Dark.skin();
    let (size, row_h, pad, col_w, col_gap) = (13.0, 25.0, 28.0, 320.0, 28.0);
    let head_h = 80.0;
    let heading = 26.0;
    let col_height = |rows: usize| heading + rows as f32 * row_h;
    let third = col_height(columns[2].len()) + 16.0 + col_height(text_keys.len());
    let body_h = col_height(columns[0].len())
        .max(col_height(columns[1].len()))
        .max(third);
    let w = pad * 2.0 + col_w * 3.0 + col_gap * 2.0;
    let note_y = head_h + body_h + 14.0;
    let h = note_y + 2.0 * 22.0 + pad;
    let mut pixmap = Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)?;
    let t = Transform::from_scale(scale, scale);
    let mut target = pixmap.as_mut();
    let card = Rect::new(0.0, 0.0, w, h);
    fill(
        &mut target,
        rounded(card, 18.0),
        skin.background.with_alpha(0.98),
        t,
        order,
    );
    stroke(
        &mut target,
        rounded(card.inset(0.5), 17.5),
        skin.border,
        1.0,
        t,
        order,
    );
    fill(
        &mut target,
        text::path_styled(
            lang.t(T::CheatSheet),
            19.0,
            Point::new(pad, pad - 4.0),
            &TextStyle {
                bold: true,
                ..TextStyle::default()
            },
        ),
        skin.ink,
        t,
        order,
    );
    fill(
        &mut target,
        text::path(
            lang.t(T::CheatSheetClose),
            12.5,
            Point::new(pad, pad + 22.0),
        ),
        skin.ink.with_alpha(0.55),
        t,
        order,
    );

    let section =
        |target: &mut PixmapMut, title: &str, rows: &[(String, String)], x: f32, y: f32| {
            fill(
                target,
                text::path_styled(
                    &title.to_uppercase(),
                    10.5,
                    Point::new(x, y),
                    &TextStyle {
                        bold: true,
                        ..TextStyle::default()
                    },
                ),
                skin.ink.with_alpha(0.45),
                t,
                order,
            );
            for (i, (name, key)) in rows.iter().enumerate() {
                let ry = y + heading + i as f32 * row_h;
                let kw = chip_width(key, 11.5);
                let name = fit(name, size, col_w - kw - 12.0);
                fill(
                    target,
                    text::path(&name, size, Point::new(x, ry)),
                    skin.ink,
                    t,
                    order,
                );
                chip(
                    target,
                    key,
                    11.5,
                    x + col_w - kw,
                    ry + 1.0,
                    skin.ink.with_alpha(0.1),
                    skin.ink,
                    t,
                    order,
                );
            }
        };
    for (i, rows) in columns.iter().enumerate() {
        let x = pad + i as f32 * (col_w + col_gap);
        section(&mut target, titles[i], rows, x, head_h);
    }
    let x = pad + 2.0 * (col_w + col_gap);
    section(
        &mut target,
        titles[3],
        &text_keys,
        x,
        head_h + col_height(columns[2].len()) + 16.0,
    );
    fill(
        &mut target,
        text::path(lang.t(T::WithPen), 12.5, Point::new(pad, note_y)),
        ACCENT,
        t,
        order,
    );
    fill(
        &mut target,
        text::path(lang.t(T::Magnet), 12.5, Point::new(pad, note_y + 22.0)),
        ACCENT,
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
            magnifier: false,
            frozen: false,
            color_index: 0,
            colors: Rgba::PALETTE,
            ghost: false,
            whiteboard: false,
            width: 4.0,
            text_size: 28.0,
            theme: Theme::Dark,
            badges: Some(Shortcuts::default()),
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
    fn layout_and_hit_testing_with_the_context_row() {
        for vertical in [false, true] {
            for first in [false, true] {
                let mut p = Palette::new(Item::defaults(), vertical);
                p.context_first = first;
                let plain = p.layout().2;
                p.sync(&Look {
                    tool: ToolKind::Arrow,
                    ..look()
                });
                let (cells, grip, (w, h)) = p.layout();
                assert_eq!(cells.len(), p.items.len() + SHAPES.len());
                assert!(if vertical { h > w } else { w > h });
                assert!(w * h > plain.0 * plain.1, "the shapes row adds room");
                assert_eq!(p.hit(grip.center()), Some(Hit::Grip));
                for (i, c) in cells.iter().enumerate() {
                    assert_eq!(p.hit(c.center()), Some(Hit::Item(i)));
                }
                assert_eq!(
                    p.item(p.items.len() + 1),
                    Item::Action(Action::Tool(ToolKind::Arrow))
                );
                // The main bar moves only when the row opens before it.
                assert_eq!(p.main_offset() != (MARGIN, MARGIN), first);
            }
        }
    }

    #[test]
    fn presses() {
        let mut p = Palette::new(Item::defaults(), false);
        let at = |p: &Palette, item: Item| p.items.iter().position(|i| *i == item).unwrap();
        let more = at(&p, Item::More);
        assert_eq!(p.press(more, &look()), Press::None);
        assert!(p.more_open);
        p.sync(&look());
        let settings = p.items.len() + MORE.iter().position(|i| *i == Item::Settings).unwrap();
        assert_eq!(p.item(settings), Item::Settings);
        assert_eq!(p.press(settings, &look()), Press::Settings);
        assert!(!p.more_open, "picking from the row closes it");
        // Shapes picks the last shape used.
        let shapes = at(&p, Item::Shapes);
        assert_eq!(
            p.press(shapes, &look()),
            Press::Run(Action::Tool(ToolKind::Arrow))
        );
        p.sync(&Look {
            tool: ToolKind::Ellipse,
            ..look()
        });
        assert_eq!(
            p.press(shapes, &look()),
            Press::Run(Action::Tool(ToolKind::Ellipse))
        );
        let close = at(&p, Item::Close);
        assert_eq!(p.press(close, &look()), Press::Run(Action::ToggleDrawing));
    }

    #[test]
    fn badges_are_single_keys() {
        let s = Shortcuts::default();
        assert_eq!(
            Item::Action(Action::Tool(ToolKind::Pen))
                .badge(&s)
                .as_deref(),
            Some("P")
        );
        assert_eq!(
            Item::Action(Action::Undo).badge(&s),
            None,
            "Ctrl+Z is too long"
        );
        assert_eq!(Item::Action(Action::Color(0)).badge(&s), None);
        assert_eq!(Item::Action(Action::Help).badge(&s).as_deref(), Some("?"));
    }

    fn render(p: &mut Palette, look: &Look) -> Pixmap {
        p.sync(look);
        let (w, h) = p.size_px(2.0);
        let mut pixmap = Pixmap::new(w, h).unwrap();
        p.render(&mut pixmap.as_mut(), 2.0, ChannelOrder::Rgba, look);
        pixmap
    }

    #[test]
    fn renders_every_item_and_the_extras() {
        let mut p = Palette::new(Item::all(), false);
        p.hover = Some(3);
        let pixmap = render(&mut p, &look());
        let (cells, _, _) = p.layout();
        // Every cell has some ink brighter than the background.
        for (i, c) in cells.iter().enumerate() {
            // A black swatch on the dark bar is dark by design.
            if p.item(i) == Item::Action(Action::Color(5)) {
                continue;
            }
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
            assert!(lit, "{:?} has no icon", p.item(i));
        }
        assert!(render_tooltip("Pen  ·  P", 1.5, ChannelOrder::Rgba).is_some());
        let sheet =
            render_cheat_sheet(Lang::Ru, &Shortcuts::default(), 1.0, ChannelOrder::Rgba).unwrap();
        assert!(sheet.width() > 700 && sheet.height() > 300);
        if let Ok(dir) = std::env::var("SCREENPEN_SELFTEST") {
            let shots = [
                ("palette-dark", Item::defaults(), look()),
                (
                    "palette-light-shapes",
                    Item::defaults(),
                    Look {
                        tool: ToolKind::Arrow,
                        theme: Theme::Light,
                        color_index: 3,
                        ..look()
                    },
                ),
                (
                    "palette-text",
                    Item::defaults(),
                    Look {
                        tool: ToolKind::Text,
                        bold: true,
                        ..look()
                    },
                ),
                (
                    "palette-ghost",
                    Item::defaults(),
                    Look {
                        ghost: true,
                        ..look()
                    },
                ),
            ];
            for (name, items, look) in shots {
                let mut p = Palette::new(items, false);
                render(&mut p, &look)
                    .save_png(format!("{dir}/{name}.png"))
                    .unwrap();
            }
            let mut p = Palette::new(Item::defaults(), true);
            p.more_open = true;
            render(&mut p, &look())
                .save_png(format!("{dir}/palette-vertical-more.png"))
                .unwrap();
            render_tooltip("Ластик  ·  E", 2.0, ChannelOrder::Rgba)
                .unwrap()
                .save_png(format!("{dir}/tooltip.png"))
                .unwrap();
            sheet.save_png(format!("{dir}/cheatsheet.png")).unwrap();
        }
    }

    #[test]
    fn brush_cursor_is_as_wide_as_the_stroke() {
        let style = screenpen_core::Style::new(Rgba::RED, 8.0);
        let (small, hot) = render_cursor(ToolKind::Pen, &style, 1.0, ChannelOrder::Rgba).unwrap();
        let (big, _) = render_cursor(ToolKind::Pen, &style, 2.0, ChannelOrder::Rgba).unwrap();
        assert!(big.width() > small.width());
        let middle = small.pixel(hot.0, hot.1).unwrap();
        assert!(
            middle.red() > 200 && middle.green() < 100,
            "the dot has the color"
        );
        assert!(render_cursor(ToolKind::Text, &style, 1.0, ChannelOrder::Rgba).is_none());
    }

    #[test]
    fn lens_is_round_and_enlarged() {
        let mut source = Pixmap::new(20, 20).unwrap();
        source.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        let lens = render_lens(&source, 100).unwrap();
        assert_eq!(
            lens.pixel(50, 50).unwrap().blue(),
            255,
            "the middle shows the source"
        );
        assert_eq!(
            lens.pixel(2, 2).unwrap().alpha(),
            0,
            "corners are outside the circle"
        );
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
