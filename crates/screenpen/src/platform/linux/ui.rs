//! The palette window, its tooltip and the cheat sheet. Palette layout and drawing
//! live in `crate::palette`; this is the window side: dragging, snapping, placement.

use std::time::Instant;

use screenpen_core::Point;
use screenpen_core::render::ChannelOrder;
use tiny_skia::Pixmap;

use super::display::{Area, Display, Monitor, Win};
use crate::palette::{Look, Palette};

/// A window showing a rendered pixmap (tooltip, cheat sheet).
pub struct Popup {
    win: Win,
    pub visible: bool,
}

impl Popup {
    pub fn new(win: Win) -> Popup {
        Popup {
            win,
            visible: false,
        }
    }

    pub fn show(&mut self, display: &mut dyn Display, pixmap: &Pixmap, at: (i32, i32)) {
        display.present(self.win, at, pixmap.as_ref(), None);
        display.show(self.win);
        self.visible = true;
    }

    pub fn hide(&mut self, display: &mut dyn Display) {
        if self.visible {
            display.hide(self.win);
            self.visible = false;
        }
    }
}

/// The monitor containing `at`, or the nearest one.
pub fn monitor_at(monitors: &[Monitor], at: (i32, i32)) -> Option<Monitor> {
    let distance = |a: &Area| {
        let dx = (a.x - at.0).max(at.0 - a.right() + 1).max(0) as i64;
        let dy = (a.y - at.1).max(at.1 - a.bottom() + 1).max(0) as i64;
        dx * dx + dy * dy
    };
    monitors.iter().copied().min_by_key(|m| distance(&m.area))
}

pub fn work_area(monitors: &[Monitor], at: (i32, i32)) -> Area {
    monitor_at(monitors, at).map_or(
        Area {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        },
        |m| m.area,
    )
}

fn scale_at(monitors: &[Monitor], at: (i32, i32)) -> f32 {
    monitor_at(monitors, at).map_or(1.0, |m| m.scale)
}

pub struct PaletteWindow {
    pixmap: Option<Pixmap>,
    pub scale: f32,
    pub origin: (i32, i32),
    pub palette: Palette,
    /// Cursor offset from the origin while dragging by the grip.
    pub drag: Option<(i32, i32)>,
    pub last_grip_click: Option<Instant>,
    pub tooltip: Popup,
    /// The pointer is over the grip (its tooltip explains dragging and rotating).
    pub grip_hover: bool,
    pub visible: bool,
}

impl PaletteWindow {
    pub fn new(palette: Palette) -> PaletteWindow {
        PaletteWindow {
            pixmap: None,
            scale: 1.0,
            origin: (0, 0),
            palette,
            drag: None,
            last_grip_click: None,
            tooltip: Popup::new(Win::Tooltip),
            grip_hover: false,
            visible: false,
        }
    }

    pub fn size(&self) -> (i32, i32) {
        let (w, h) = self.palette.size_px(self.scale);
        (w as i32, h as i32)
    }

    /// Screen pixels to palette points.
    pub fn to_local(&self, p: (i32, i32)) -> Point {
        Point::new(
            (p.0 - self.origin.0) as f32 / self.scale,
            (p.1 - self.origin.1) as f32 / self.scale,
        )
    }

    pub fn render(&mut self, display: &mut dyn Display, look: &Look) {
        if !self.visible {
            return;
        }
        let (w, h) = self.palette.size_px(self.scale);
        if self
            .pixmap
            .as_ref()
            .is_none_or(|p| p.width() != w || p.height() != h)
        {
            self.pixmap = Pixmap::new(w, h);
        }
        let Some(pixmap) = &mut self.pixmap else {
            return;
        };
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        self.palette
            .render(&mut pixmap.as_mut(), self.scale, ChannelOrder::Bgra, look);
        display.present(Win::Palette, self.origin, pixmap.as_ref(), None);
    }

    pub fn show(&mut self, display: &mut dyn Display, monitors: &[Monitor], look: &Look) {
        self.visible = true;
        self.scale = scale_at(monitors, self.center());
        self.render(display, look);
        display.show(Win::Palette);
    }

    pub fn hide(&mut self, display: &mut dyn Display) {
        self.visible = false;
        self.drag = None;
        self.palette.hover = None;
        self.tooltip.hide(display);
        display.hide(Win::Palette);
    }

    pub fn center(&self) -> (i32, i32) {
        let (w, h) = self.size();
        (self.origin.0 + w / 2, self.origin.1 + h / 2)
    }

    /// Moves the top-left corner to `to`, keeping the palette on its monitor and
    /// snapping it to an edge when close.
    pub fn move_to(
        &mut self,
        display: &mut dyn Display,
        monitors: &[Monitor],
        to: (i32, i32),
        look: &Look,
    ) {
        let (w, h) = self.size();
        let area = work_area(monitors, (to.0 + w / 2, to.1 + h / 2));
        let margin = (8.0 * self.scale) as i32;
        let snap = (16.0 * self.scale) as i32;
        let clamp = |v: i32, lo: i32, hi: i32| v.max(lo).min(hi.max(lo));
        let mut x = clamp(to.0, area.x, area.right() - w);
        let mut y = clamp(to.1, area.y, area.bottom() - h);
        if (x - area.x).abs() < snap {
            x = area.x + margin;
        } else if (area.right() - w - x).abs() < snap {
            x = area.right() - w - margin;
        }
        if (y - area.y).abs() < snap {
            y = area.y + margin;
        } else if (area.bottom() - h - y).abs() < snap {
            y = area.bottom() - h - margin;
        }
        self.origin = (x, y);
        self.scale = scale_at(monitors, self.center());
        self.render(display, look);
    }

    /// Top center of the monitor containing `at`.
    pub fn default_origin(&self, monitors: &[Monitor], at: (i32, i32)) -> (i32, i32) {
        let area = work_area(monitors, at);
        let scale = scale_at(monitors, at);
        let (w, _) = self.palette.size_px(scale);
        (
            area.x + (area.w - w as i32) / 2,
            area.y + (12.0 * scale) as i32,
        )
    }
}
