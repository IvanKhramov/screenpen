use serde::{Deserialize, Serialize};

/// An sRGB color with components in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

impl Rgba {
    pub const fn rgb(red: f32, green: f32, blue: f32) -> Rgba {
        Rgba {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    pub const RED: Rgba = Rgba::rgb(0.96, 0.23, 0.21);
    pub const ORANGE: Rgba = Rgba::rgb(1.0, 0.58, 0.0);
    pub const YELLOW: Rgba = Rgba::rgb(1.0, 0.84, 0.0);
    pub const GREEN: Rgba = Rgba::rgb(0.2, 0.78, 0.35);
    pub const BLUE: Rgba = Rgba::rgb(0.0, 0.48, 1.0);
    pub const WHITE: Rgba = Rgba::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Rgba = Rgba::rgb(0.0, 0.0, 0.0);
    pub const PURPLE: Rgba = Rgba::rgb(0.69, 0.32, 0.87);
    pub const PINK: Rgba = Rgba::rgb(1.0, 0.18, 0.57);

    /// The default swatches, bound to keys 1-9.
    /// The first six are on the palette.
    pub const PALETTE: [Rgba; 9] = [
        Rgba::RED,
        Rgba::YELLOW,
        Rgba::GREEN,
        Rgba::BLUE,
        Rgba::WHITE,
        Rgba::BLACK,
        Rgba::ORANGE,
        Rgba::PURPLE,
        Rgba::PINK,
    ];

    /// White on dark colors, black on light ones (badge numbers).
    pub fn contrasting(self) -> Rgba {
        let luminance = 0.2126 * self.red + 0.7152 * self.green + 0.0722 * self.blue;
        if luminance > 0.6 {
            Rgba::BLACK
        } else {
            Rgba::WHITE
        }
    }

    pub fn with_alpha(self, alpha: f32) -> Rgba {
        Rgba { alpha, ..self }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub color: Rgba,
    /// Base line width in points; pressure scales it per segment.
    pub width: f32,
    /// Applied to the whole annotation, so overlapping segments don't darken.
    pub opacity: f32,
    pub blend: Blend,
}

impl Style {
    pub const fn new(color: Rgba, width: f32) -> Style {
        Style {
            color,
            width,
            opacity: 1.0,
            blend: Blend::Normal,
        }
    }

    pub const DEFAULT_PEN: Style = Style::new(Rgba::RED, 4.0);
}
