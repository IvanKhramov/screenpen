//! Settings persisted as TOML. Every field has a default, so an old or partial file
//! still loads, and unknown names (a removed action, a typo) are skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use screenpen_core::{Action, Chord, Rgba, TextStyle, ToolKind};
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::palette::Item;
use crate::state::{Preset, ToolState, UI_SCALE};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "auto" follows the system language; "en" or "ru".
    pub language: String,
    pub tool: String,
    /// 1-based, like the keys that pick colors.
    pub color: usize,
    /// Nine swatches as "#RRGGBB".
    pub colors: Vec<String>,
    pub width: f32,
    /// 0..=1, how much the screen is darkened while drawing.
    pub dim: f32,
    /// 0 turns auto-fade off.
    pub fade_seconds: f32,
    pub hide_palette_from_capture: bool,
    /// Size of new text, in points.
    pub text_size: f32,
    /// Font family for text; empty is the system sans-serif.
    pub font: String,
    pub bold: bool,
    pub italic: bool,
    pub text_background: bool,
    pub text_outline: bool,
    /// Interface scale (palette, tooltips, cheat sheet, settings), 0.75..=2.
    pub ui_scale: f32,
    /// "system", "dark" or "light".
    pub theme: String,
    /// Shortcut letters in the palette button corners.
    pub key_badges: bool,
    pub palette: PaletteSettings,
    /// How long the timer (Ctrl+T) counts down.
    pub timer_minutes: f32,
    /// In mouse mode, show clicks and shortcuts (for recordings and lessons).
    pub show_input: bool,
    /// Shift+1..3: tool name, 1-based color and width.
    pub presets: Vec<PresetSettings>,
    /// Action name to chord, e.g. "tool.pen" = "P". An empty string turns it off.
    pub shortcuts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetSettings {
    pub tool: String,
    pub color: usize,
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteSettings {
    /// Files before the redesign have none (0) and get the new default palette.
    #[serde(default)]
    pub version: u32,
    pub items: Vec<String>,
    pub vertical: bool,
    /// Top-left corner in screen pixels; unset places it at the top center.
    pub x: Option<i32>,
    pub y: Option<i32>,
}

/// Bumped when the default palette changes enough to replace saved ones.
pub const PALETTE_VERSION: u32 = 2;

/// The swatch order before the redesign, to recognize untouched colors.
const OLD_COLORS: [Rgba; 9] = [
    Rgba::RED,
    Rgba::ORANGE,
    Rgba::YELLOW,
    Rgba::GREEN,
    Rgba::BLUE,
    Rgba::PURPLE,
    Rgba::PINK,
    Rgba::WHITE,
    Rgba::BLACK,
];

impl Default for PaletteSettings {
    fn default() -> Self {
        PaletteSettings {
            version: PALETTE_VERSION,
            items: Item::defaults().iter().map(|i| i.name()).collect(),
            vertical: false,
            x: None,
            y: None,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings::capture(
            &ToolState::default(),
            &PaletteSettings::default(),
            "auto",
            false,
        )
    }
}

/// Where the settings file lives on this OS.
pub fn default_path() -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("screenpen")
        .join("settings.toml")
}

pub fn hex(c: Rgba) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", b(c.red), b(c.green), b(c.blue))
}

pub fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim().strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    let c = |shift: u32| ((v >> shift) & 0xff) as f32 / 255.0;
    Some(Rgba::rgb(c(16), c(8), c(0)))
}

impl Settings {
    /// Reads `path`; a missing file gives defaults, a broken one gives defaults and the error.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (
                Settings::default(),
                Some(format!("{}: {e}", path.display())),
            ),
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    s.migrate();
                    (s, None)
                }
                Err(e) => (
                    Settings::default(),
                    Some(format!("{}: {e}", path.display())),
                ),
            },
        }
    }

    /// Brings a file from an older version up to date.
    fn migrate(&mut self) {
        if self.palette.version < 2 {
            // The redesigned palette: new items, new place (bottom center).
            self.palette = PaletteSettings {
                vertical: false,
                ..PaletteSettings::default()
            };
            let untouched = self.colors.len() == OLD_COLORS.len()
                && self
                    .colors
                    .iter()
                    .zip(OLD_COLORS)
                    .all(|(a, b)| a.eq_ignore_ascii_case(&hex(b)));
            if untouched {
                let color = self.colors.get(self.color.wrapping_sub(1)).cloned();
                self.colors = Rgba::PALETTE.iter().map(|c| hex(*c)).collect();
                self.color = color
                    .and_then(|c| self.colors.iter().position(|n| n.eq_ignore_ascii_case(&c)))
                    .map_or(1, |i| i + 1);
            }
        }
        self.palette.version = PALETTE_VERSION;
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write then rename, so a crash never leaves half a file.
        // Per-process name: the settings window and the app may save at the same time.
        let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    pub fn capture(
        state: &ToolState,
        palette: &PaletteSettings,
        language: &str,
        hide_palette: bool,
    ) -> Settings {
        Settings {
            language: language.to_string(),
            tool: state.tool.name().to_string(),
            color: state.color_index + 1,
            colors: state.colors.iter().map(|c| hex(*c)).collect(),
            width: state.width,
            dim: state.dim,
            fade_seconds: state.auto_fade.unwrap_or(0.0),
            hide_palette_from_capture: hide_palette,
            text_size: state.text_size,
            font: state.text_style.family.clone().unwrap_or_default(),
            bold: state.text_style.bold,
            italic: state.text_style.italic,
            text_background: state.text_style.background,
            text_outline: state.text_style.outline,
            ui_scale: state.ui_scale,
            theme: state.theme.clone(),
            key_badges: state.key_badges,
            palette: palette.clone(),
            timer_minutes: state.timer_minutes,
            show_input: state.show_input,
            presets: state
                .presets
                .iter()
                .map(|p| PresetSettings {
                    tool: p.tool.name().to_string(),
                    color: p.color_index + 1,
                    width: p.width,
                })
                .collect(),
            shortcuts: state
                .shortcuts
                .bindings
                .iter()
                .map(|b| {
                    (
                        b.action.name(),
                        b.chord.map(|c| c.to_string()).unwrap_or_default(),
                    )
                })
                .collect(),
        }
    }

    /// Applies the settings on top of `state`. Returns the problems found (bad chords,
    /// unknown actions), which are skipped.
    pub fn apply(&self, state: &mut ToolState) -> Vec<String> {
        let mut problems = Vec::new();
        state.tool = ToolKind::from_name(&self.tool).unwrap_or(ToolKind::Pen);
        for (i, text) in self.colors.iter().take(state.colors.len()).enumerate() {
            match parse_hex(text) {
                Some(c) => state.colors[i] = c,
                None => problems.push(format!("colors[{i}]: {text:?} is not #RRGGBB")),
            }
        }
        state.color_index = self.color.clamp(1, state.colors.len()) - 1;
        state.width = self.width.clamp(1.0, 64.0);
        state.dim = self.dim.clamp(0.0, 0.8);
        state.auto_fade = (self.fade_seconds > 0.0).then_some(self.fade_seconds);
        state.text_size = self.text_size.clamp(8.0, 200.0);
        state.text_style = TextStyle {
            family: (!self.font.trim().is_empty()).then(|| self.font.trim().to_string()),
            bold: self.bold,
            italic: self.italic,
            background: self.text_background,
            outline: self.text_outline,
        };
        let (min, max, _) = UI_SCALE;
        state.ui_scale = if self.ui_scale.is_finite() {
            self.ui_scale.clamp(min, max)
        } else {
            1.0
        };
        state.theme = match self.theme.as_str() {
            t @ ("dark" | "light") => t.to_string(),
            _ => "system".to_string(),
        };
        state.key_badges = self.key_badges;
        state.show_input = self.show_input;
        state.timer_minutes = if self.timer_minutes.is_finite() {
            self.timer_minutes.clamp(1.0, 180.0)
        } else {
            5.0
        };
        for (slot, p) in state.presets.iter_mut().zip(&self.presets) {
            match ToolKind::from_name(&p.tool) {
                Some(tool) => {
                    *slot = Preset {
                        tool,
                        color_index: p.color.clamp(1, state.colors.len()) - 1,
                        width: p.width.clamp(1.0, 64.0),
                    }
                }
                None => problems.push(format!("presets: unknown tool {:?}", p.tool)),
            }
        }
        for (name, text) in &self.shortcuts {
            let Some(action) = Action::from_name(name) else {
                problems.push(format!("shortcuts: unknown action {name:?}"));
                continue;
            };
            if text.trim().is_empty() {
                state.shortcuts.set(action, None);
                continue;
            }
            match text.parse::<Chord>() {
                Ok(chord) => state.shortcuts.set(action, Some(chord)),
                Err(e) => problems.push(format!("shortcuts.{name}: {e}")),
            }
        }
        problems
    }

    pub fn lang(&self, system: Lang) -> Lang {
        match self.language.as_str() {
            "ru" => Lang::Ru,
            "en" => Lang::En,
            _ => system,
        }
    }

    /// Palette items in order, skipping unknown names.
    pub fn palette_items(&self) -> Vec<Item> {
        self.palette
            .items
            .iter()
            .filter_map(|n| Item::from_name(n))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenpen_core::Key;

    #[test]
    fn defaults_round_trip_through_toml() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
        let mut state = ToolState::default();
        assert!(back.apply(&mut state).is_empty());
        assert_eq!(state.shortcuts, ToolState::default().shortcuts);
    }

    #[test]
    fn partial_file_keeps_defaults_and_reports_problems() {
        let text = r##"
            width = 8
            color = 3
            colors = ["#00FF00", "nope"]
            [shortcuts]
            "tool.pen" = "Q"
            "help" = ""
            "no.such" = "X"
            "undo" = "Hyper+Z"
        "##;
        let s: Settings = toml::from_str(text).unwrap();
        let mut state = ToolState::default();
        let problems = s.apply(&mut state);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(state.width, 8.0);
        assert_eq!(state.color_index, 2);
        assert_eq!(state.colors[0], Rgba::rgb(0.0, 1.0, 0.0));
        assert_eq!(
            state.shortcuts.chord(Action::Tool(ToolKind::Pen)),
            Some(Chord::key(Key::Char('Q')))
        );
        assert_eq!(state.shortcuts.chord(Action::Help), None);
        assert_eq!(
            state.shortcuts.chord(Action::Undo),
            Some(Chord::ctrl(Key::Char('Z')))
        );
        assert_eq!(s.palette_items(), Item::defaults());
    }

    #[test]
    fn hex_colors() {
        assert_eq!(hex(Rgba::RED), "#F53B36");
        assert_eq!(parse_hex("#F53B36").map(hex).as_deref(), Some("#F53B36"));
        assert_eq!(parse_hex("F53B36"), None);
    }
}
