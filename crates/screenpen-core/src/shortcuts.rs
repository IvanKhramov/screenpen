//! Rebindable shortcuts. Keys are physical keys named after the US layout, so
//! "P" stays the pen on a Russian layout too.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::tools::ToolKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A letter (uppercase), digit or punctuation key, named by its US-layout character.
    Char(char),
    Escape,
    Enter,
    Space,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    F(u8),
}

const NAMED: [(Key, &str); 15] = [
    (Key::Escape, "Esc"),
    (Key::Enter, "Enter"),
    (Key::Space, "Space"),
    (Key::Tab, "Tab"),
    (Key::Backspace, "Backspace"),
    (Key::Delete, "Delete"),
    (Key::Insert, "Insert"),
    (Key::Home, "Home"),
    (Key::End, "End"),
    (Key::PageUp, "PageUp"),
    (Key::PageDown, "PageDown"),
    (Key::Left, "Left"),
    (Key::Right, "Right"),
    (Key::Up, "Up"),
    (Key::Down, "Down"),
];

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "F{n}"),
            named => {
                let name = NAMED
                    .iter()
                    .find(|(k, _)| k == named)
                    .map_or("?", |(_, n)| n);
                f.write_str(name)
            }
        }
    }
}

impl FromStr for Key {
    type Err = String;
    fn from_str(s: &str) -> Result<Key, String> {
        if let Some((key, _)) = NAMED.iter().find(|(_, n)| n.eq_ignore_ascii_case(s)) {
            return Ok(*key);
        }
        if let Some(n) = s
            .strip_prefix(['F', 'f'])
            .and_then(|n| n.parse::<u8>().ok())
            && (1..=24).contains(&n)
        {
            return Ok(Key::F(n));
        }
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_graphic() => Ok(Key::Char(c.to_ascii_uppercase())),
            _ => Err(format!("unknown key {s:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    pub const fn key(key: Key) -> Chord {
        Chord {
            ctrl: false,
            alt: false,
            shift: false,
            key,
        }
    }

    pub const fn ctrl(key: Key) -> Chord {
        Chord {
            ctrl: true,
            alt: false,
            shift: false,
            key,
        }
    }

    pub const fn ctrl_shift(key: Key) -> Chord {
        Chord {
            ctrl: true,
            alt: false,
            shift: true,
            key,
        }
    }

    pub const fn ctrl_alt_shift(key: Key) -> Chord {
        Chord {
            ctrl: true,
            alt: true,
            shift: true,
            key,
        }
    }

    /// A global hotkey needs at least one of Ctrl/Alt, or it would eat normal typing.
    pub fn usable_globally(&self) -> bool {
        self.ctrl || self.alt || matches!(self.key, Key::F(_))
    }
}

/// What the Ctrl of a chord is called on this system: Cmd plays its part on a Mac.
pub const CTRL: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

impl Chord {
    fn write(&self, f: &mut impl fmt::Write, ctrl: &str) -> fmt::Result {
        if self.ctrl {
            write!(f, "{ctrl}+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        write!(f, "{}", self.key)
    }
}

/// As shown to the user: "Cmd+Z" on a Mac, "Ctrl+Z" elsewhere.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, CTRL)
    }
}

impl FromStr for Chord {
    type Err = String;
    fn from_str(s: &str) -> Result<Chord, String> {
        let s = s.trim();
        // "+" itself is a key: "Ctrl++" or "+".
        let (mods, key) = match s.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None if s == "+" => ("", "+"),
            None => s.rsplit_once('+').map_or(("", s), |(m, k)| (m, k)),
        };
        let mut chord = Chord::key(key.trim().parse()?);
        for m in mods.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => chord.ctrl = true,
                "alt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                other => return Err(format!("unknown modifier {other:?}")),
            }
        }
        Ok(chord)
    }
}

impl Serialize for Chord {
    /// Always "Ctrl", so the settings file reads the same on every system.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut text = String::new();
        self.write(&mut text, "Ctrl")
            .map_err(serde::ser::Error::custom)?;
        s.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Chord, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Tool(ToolKind),
    /// Index into the color swatches (0-8).
    Color(u8),
    Thinner,
    Thicker,
    /// A width preset (0-2), see `WIDTH_PRESETS`.
    Width(u8),
    /// A saved tool, color and width (0-2), and saving the current ones there.
    Preset(u8),
    SavePreset(u8),
    /// Text size, for the text being typed, the selected text or new text.
    TextSmaller,
    TextBigger,
    Bold,
    Italic,
    Clear,
    Undo,
    Redo,
    SelectAll,
    BringToFront,
    DeleteSelection,
    /// Deselect, or stop drawing when nothing is selected.
    Escape,
    /// Right after an arrow: type a caption at its tail.
    Caption,
    Ghost,
    Whiteboard,
    /// Dim everything except a circle around the pointer.
    Spotlight,
    /// Hold the current screen image under the ink.
    Freeze,
    /// A round lens that follows the pointer and enlarges the screen under it.
    Magnifier,
    /// Save the screen with the ink to a PNG and copy it to the clipboard.
    Snapshot,
    /// A countdown in the corner; again to remove it.
    Timer,
    /// In mouse mode, rings where clicks land and the shortcuts pressed.
    ShowInput,
    Help,
    /// Global: start or stop drawing.
    ToggleDrawing,
    /// Global: leave ghost mode (the overlay has no keyboard focus then).
    ExitGhost,
    /// Global.
    Quit,
}

impl Action {
    pub fn is_global(self) -> bool {
        matches!(
            self,
            Action::ToggleDrawing | Action::ExitGhost | Action::Quit
        )
    }

    /// Stable identifier for settings files and translations, e.g. "tool.pen", "color.3".
    pub fn name(self) -> String {
        match self {
            Action::Tool(t) => format!("tool.{}", t.name()),
            Action::Color(i) => format!("color.{}", i + 1),
            Action::Width(i) => format!("width.{}", i + 1),
            Action::Preset(i) => format!("preset.{}", i + 1),
            Action::SavePreset(i) => format!("save_preset.{}", i + 1),
            other => {
                let s = match other {
                    Action::Thinner => "thinner",
                    Action::Thicker => "thicker",
                    Action::TextSmaller => "text_smaller",
                    Action::TextBigger => "text_bigger",
                    Action::Bold => "bold",
                    Action::Italic => "italic",
                    Action::Clear => "clear",
                    Action::Undo => "undo",
                    Action::Redo => "redo",
                    Action::SelectAll => "select_all",
                    Action::BringToFront => "bring_to_front",
                    Action::DeleteSelection => "delete_selection",
                    Action::Escape => "escape",
                    Action::Caption => "caption",
                    Action::Ghost => "ghost",
                    Action::Whiteboard => "whiteboard",
                    Action::Spotlight => "spotlight",
                    Action::Freeze => "freeze",
                    Action::Magnifier => "magnifier",
                    Action::Snapshot => "snapshot",
                    Action::Timer => "timer",
                    Action::ShowInput => "show_input",
                    Action::Help => "help",
                    Action::ToggleDrawing => "toggle_drawing",
                    Action::ExitGhost => "exit_ghost",
                    Action::Quit => "quit",
                    Action::Tool(_)
                    | Action::Color(_)
                    | Action::Width(_)
                    | Action::Preset(_)
                    | Action::SavePreset(_) => unreachable!(),
                };
                s.to_string()
            }
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Shortcuts::default()
            .bindings
            .into_iter()
            .map(|b| b.action)
            .find(|a| a.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub action: Action,
    /// None means the shortcut is turned off.
    pub chord: Option<Chord>,
}

/// Every action with its chord, in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcuts {
    pub bindings: Vec<Binding>,
}

impl Default for Shortcuts {
    fn default() -> Self {
        use Key::Char;
        let key = |c: char| Some(Chord::key(Char(c)));
        let mut b: Vec<(Action, Option<Chord>)> = vec![
            (
                Action::ToggleDrawing,
                Some(Chord::ctrl_alt_shift(Char('A'))),
            ),
            (Action::ExitGhost, Some(Chord::ctrl_alt_shift(Char('G')))),
            (Action::Quit, Some(Chord::ctrl_alt_shift(Char('Q')))),
            (Action::Tool(ToolKind::Pen), key('P')),
            (Action::Tool(ToolKind::Highlighter), key('H')),
            (Action::Tool(ToolKind::Line), key('L')),
            (Action::Tool(ToolKind::Arrow), key('A')),
            (Action::Tool(ToolKind::Rectangle), key('R')),
            (Action::Tool(ToolKind::Ellipse), key('O')),
            (Action::Tool(ToolKind::Text), key('T')),
            (Action::Tool(ToolKind::Badge), key('N')),
            (Action::Tool(ToolKind::Eraser), key('E')),
            (Action::Tool(ToolKind::Select), key('V')),
            (Action::Tool(ToolKind::Laser), key('K')),
            (Action::Tool(ToolKind::Pixelate), key('B')),
        ];
        for i in 0..9u8 {
            b.push((Action::Color(i), key((b'1' + i) as char)));
        }
        b.extend([
            (Action::Thinner, key('[')),
            (Action::Thicker, key(']')),
            (Action::Width(0), None),
            (Action::Width(1), None),
            (Action::Width(2), None),
        ]);
        for i in 0..3u8 {
            let digit = Char((b'1' + i) as char);
            let shift = Chord {
                shift: true,
                ..Chord::key(digit)
            };
            b.push((Action::Preset(i), Some(shift)));
            b.push((Action::SavePreset(i), Some(Chord::ctrl_shift(digit))));
        }
        b.extend([
            (Action::TextSmaller, Some(Chord::ctrl(Char('-')))),
            (Action::TextBigger, Some(Chord::ctrl(Char('=')))),
            (Action::Bold, Some(Chord::ctrl(Char('B')))),
            (Action::Italic, Some(Chord::ctrl(Char('I')))),
            (Action::Clear, key('C')),
            (Action::Undo, Some(Chord::ctrl(Char('Z')))),
            (Action::Redo, Some(Chord::ctrl_shift(Char('Z')))),
            (Action::SelectAll, Some(Chord::ctrl(Char('A')))),
            (Action::BringToFront, Some(Chord::ctrl(Char(']')))),
            (Action::DeleteSelection, Some(Chord::key(Key::Delete))),
            (Action::Escape, Some(Chord::key(Key::Escape))),
            (Action::Caption, Some(Chord::key(Key::Enter))),
            (Action::Ghost, key('G')),
            (Action::Whiteboard, key('W')),
            (Action::Spotlight, key('S')),
            (Action::Freeze, key('F')),
            (Action::Magnifier, key('M')),
            (Action::Snapshot, Some(Chord::ctrl(Char('S')))),
            (Action::Timer, Some(Chord::ctrl(Char('T')))),
            (Action::ShowInput, None),
            (
                Action::Help,
                Some(Chord {
                    shift: true,
                    ..Chord::key(Char('/'))
                }),
            ),
        ]);
        Shortcuts {
            bindings: b
                .into_iter()
                .map(|(action, chord)| Binding { action, chord })
                .collect(),
        }
    }
}

impl Shortcuts {
    pub fn chord(&self, action: Action) -> Option<Chord> {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .and_then(|b| b.chord)
    }

    pub fn set(&mut self, action: Action, chord: Option<Chord>) {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.action == action) {
            b.chord = chord;
        }
    }

    /// The drawing-time (non-global) action bound to `chord`.
    pub fn local_action(&self, chord: Chord) -> Option<Action> {
        self.bindings
            .iter()
            .find(|b| !b.action.is_global() && b.chord == Some(chord))
            .map(|b| b.action)
    }

    /// Pairs of actions that share a chord in the same scope. Global chords conflict
    /// with everything, since they fire while drawing too.
    pub fn conflicts(&self) -> Vec<(Action, Action)> {
        let mut out = Vec::new();
        for (i, a) in self.bindings.iter().enumerate() {
            for b in &self.bindings[i + 1..] {
                if a.chord.is_some() && a.chord == b.chord {
                    out.push((a.action, b.action));
                }
            }
        }
        out
    }
}

/// Pen widths behind the S, M and L buttons.
pub const WIDTH_PRESETS: [f32; 3] = [2.0, 4.0, 8.0];

/// Width presets that Thinner and Thicker step through.
pub const WIDTH_STEPS: [f32; 10] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0];

pub fn next_width(width: f32, thicker: bool) -> f32 {
    if thicker {
        WIDTH_STEPS
            .iter()
            .copied()
            .find(|&w| w > width)
            .unwrap_or(WIDTH_STEPS[WIDTH_STEPS.len() - 1])
    } else {
        WIDTH_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&w| w < width)
            .unwrap_or(WIDTH_STEPS[0])
    }
}

/// Text size presets that TextSmaller and TextBigger step through.
pub const TEXT_STEPS: [f32; 15] = [
    12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 40.0, 48.0, 56.0, 64.0, 80.0, 96.0, 120.0,
];

pub fn next_text_size(size: f32, bigger: bool) -> f32 {
    if bigger {
        TEXT_STEPS
            .iter()
            .copied()
            .find(|&s| s > size)
            .unwrap_or(TEXT_STEPS[TEXT_STEPS.len() - 1])
    } else {
        TEXT_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&s| s < size)
            .unwrap_or(TEXT_STEPS[0])
    }
}
