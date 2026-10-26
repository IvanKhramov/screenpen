//! Tool, color and width shared by every screen and the palette.

use screenpen_core::shortcuts::{next_text_size, next_width};
use screenpen_core::{Blend, Rgba, Shortcuts, Style, TextStyle, ToolKind};

/// How much the screen behind the ink is darkened while drawing.
pub const DIM_LEVELS: [(f32, &str); 4] = [
    (0.0, "Off"),
    (0.25, "Slightly"),
    (0.45, "Medium"),
    (0.65, "Strongly"),
];

/// Auto-fade presets in seconds.
pub const FADE_SECONDS: [f32; 4] = [3.0, 5.0, 10.0, 30.0];

pub struct ToolState {
    pub tool: ToolKind,
    pub colors: [Rgba; 9],
    pub color_index: usize,
    pub width: f32,
    /// When set, each new annotation fades out after this many seconds.
    pub auto_fade: Option<f32>,
    pub dim: f32,
    pub whiteboard: bool,
    pub shortcuts: Shortcuts,
    /// Size of new text, in points.
    pub text_size: f32,
    /// Font, weight, slant, plate and outline of new text.
    pub text_style: TextStyle,
    /// Multiplier for the palette, tooltips, cheat sheet and settings window,
    /// on top of the system scale.
    pub ui_scale: f32,
}

impl Default for ToolState {
    fn default() -> Self {
        ToolState {
            tool: ToolKind::Pen,
            colors: Rgba::PALETTE,
            color_index: 0,
            width: 4.0,
            auto_fade: None,
            dim: 0.0,
            whiteboard: false,
            shortcuts: Shortcuts::default(),
            text_size: 28.0,
            text_style: TextStyle::default(),
            ui_scale: 1.0,
        }
    }
}

impl ToolState {
    pub fn color(&self) -> Rgba {
        self.colors[self.color_index.min(self.colors.len() - 1)]
    }

    pub fn font_size(&self) -> f32 {
        self.text_size
    }

    pub fn step_text_size(&mut self, bigger: bool) {
        self.text_size = next_text_size(self.text_size, bigger);
    }

    pub fn badge_radius(&self) -> f32 {
        (self.width * 2.5 + 8.0).max(13.0)
    }

    pub fn eraser_radius(&self) -> f32 {
        (self.width * 2.0).max(10.0)
    }

    pub fn style(&self, tool: ToolKind) -> Style {
        match tool {
            ToolKind::Highlighter => Style {
                color: self.color(),
                width: self.width * 4.0,
                opacity: 0.4,
                blend: Blend::Multiply,
            },
            _ => Style::new(self.color(), self.width),
        }
    }

    pub fn step_width(&mut self, thicker: bool) {
        self.width = next_width(self.width, thicker);
    }
}

/// Interface scale limits and step (Ctrl+wheel over the palette).
pub const UI_SCALE: (f32, f32, f32) = (0.75, 2.0, 0.1);

pub fn step_ui_scale(scale: f32, bigger: bool) -> f32 {
    let (min, max, step) = UI_SCALE;
    let next = if bigger { scale + step } else { scale - step };
    ((next * 20.0).round() / 20.0).clamp(min, max)
}
