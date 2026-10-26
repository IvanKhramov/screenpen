//! Settings persisted as TOML. Every field has a default, so an old or partial file
//! still loads, and unknown names (a removed action, a typo) are skipped.

use std::collections::BTreeMap;
use std::path::Path;

use screenpen_core::{Action, Chord, Rgba, ToolKind};
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::palette::Item;
use crate::state::ToolState;

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
    pub palette: PaletteSettings,
    /// Action name to chord, e.g. "tool.pen" = "P". An empty string turns it off.
    pub shortcuts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteSettings {
    pub items: Vec<String>,
    pub vertical: bool,
    /// Top-left corner in screen pixels; unset places it at the top center.
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for PaletteSettings {
    fn default() -> Self {
        PaletteSettings {
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
            Ok(text) => match toml::from_str(&text) {
                Ok(s) => (s, None),
                Err(e) => (
                    Settings::default(),
                    Some(format!("{}: {e}", path.display())),
                ),
            },
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write then rename, so a crash never leaves half a file.
        let tmp = path.with_extension("toml.tmp");
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
            palette: palette.clone(),
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
