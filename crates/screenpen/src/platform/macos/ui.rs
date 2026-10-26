//! The palette panel, its tooltip and the cheat sheet. Palette layout and drawing live
//! in `crate::palette`; this is the AppKit side: panels, dragging, snapping.
//!
//! Positions here are global points with the origin at the primary screen's top-left
//! corner and y going down (`Frame`), like the palette position in the settings.

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSWindowSharingType};
use objc2_core_foundation::CGSize;
use objc2_foundation::MainThreadMarker;
use screenpen_core::Point;
use screenpen_core::render::ChannelOrder;
use tiny_skia::Pixmap;

use super::Frame;
use super::view::{OverlayPanel, PixelView, panel};
use crate::palette::{Look, Palette};

/// A never-activated panel showing a rendered pixmap (tooltip, cheat sheet).
pub struct Popup {
    pub panel: Retained<OverlayPanel>,
    pub view: Retained<PixelView>,
    pub visible: bool,
}

impl Popup {
    /// `click_through` popups never take the mouse (tooltips).
    pub fn new(mtm: MainThreadMarker, click_through: bool) -> Popup {
        let (panel, view) = small_panel(mtm, 2);
        panel.setIgnoresMouseEvents(click_through);
        Popup {
            panel,
            view,
            visible: false,
        }
    }

    /// Shows `pixmap` (drawn at `scale`) with its top-left corner at `at`.
    pub fn show(&mut self, pixmap: Pixmap, scale: f32, at: (f64, f64)) {
        let s = scale as f64;
        let frame = Frame {
            x: at.0,
            y: at.1,
            w: pixmap.width() as f64 / s,
            h: pixmap.height() as f64 / s,
        };
        self.view.set_pixels(pixmap, s);
        self.panel
            .setFrame_display(frame.to_cocoa(self.panel.mtm()), true);
        self.panel.orderFrontRegardless();
        self.visible = true;
    }

    pub fn hide(&mut self) {
        if self.visible {
            self.panel.orderOut(None);
            self.visible = false;
        }
    }
}

fn small_panel(
    mtm: MainThreadMarker,
    above: isize,
) -> (Retained<OverlayPanel>, Retained<PixelView>) {
    let frame = Frame {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    let panel = panel(mtm, frame.to_cocoa(mtm), above);
    let view = PixelView::new(mtm, CGSize::new(1.0, 1.0));
    view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    panel.setContentView(Some(&view));
    (panel, view)
}

pub fn set_capture_hidden(panel: &OverlayPanel, hidden: bool) {
    panel.setSharingType(if hidden {
        NSWindowSharingType::None
    } else {
        NSWindowSharingType::ReadOnly
    });
}

/// The screen containing `at`, or the primary one.
fn screen_at(mtm: MainThreadMarker, at: (f64, f64)) -> Option<Retained<NSScreen>> {
    let screens = NSScreen::screens(mtm);
    screens
        .iter()
        .find(|s| Frame::from_cocoa(mtm, s.frame()).contains(at))
        .or_else(|| screens.firstObject())
}

/// The part of the screen at `at` that's not the menu bar or the Dock.
pub fn work_area(mtm: MainThreadMarker, at: (f64, f64)) -> Frame {
    screen_at(mtm, at).map_or(
        Frame {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        },
        |s| Frame::from_cocoa(mtm, s.visibleFrame()),
    )
}

pub fn scale_at(mtm: MainThreadMarker, at: (f64, f64)) -> f32 {
    screen_at(mtm, at).map_or(1.0, |s| s.backingScaleFactor() as f32)
}

pub struct PaletteWindow {
    pub panel: Retained<OverlayPanel>,
    pub view: Retained<PixelView>,
    pub scale: f32,
    /// Top-left corner.
    pub origin: (f64, f64),
    pub palette: Palette,
    /// Cursor offset from the origin while dragging by the grip.
    pub drag: Option<(f64, f64)>,
    pub tooltip: Popup,
    /// The pointer is over the grip (its tooltip explains dragging and rotating).
    pub grip_hover: bool,
    pub visible: bool,
}

impl PaletteWindow {
    pub fn new(mtm: MainThreadMarker, palette: Palette) -> PaletteWindow {
        let (panel, view) = small_panel(mtm, 1);
        PaletteWindow {
            panel,
            view,
            scale: 1.0,
            origin: (0.0, 0.0),
            palette,
            drag: None,
            tooltip: Popup::new(mtm, true),
            grip_hover: false,
            visible: false,
        }
    }

    fn mtm(&self) -> MainThreadMarker {
        self.panel.mtm()
    }

    /// Size in points.
    pub fn size(&self) -> (f64, f64) {
        let (w, h) = self.palette.size_px(self.scale);
        let s = self.scale as f64;
        (w as f64 / s, h as f64 / s)
    }

    /// Screen points to palette points.
    pub fn to_local(&self, p: (f64, f64)) -> Point {
        Point::new((p.0 - self.origin.0) as f32, (p.1 - self.origin.1) as f32)
    }

    pub fn render(&mut self, look: &Look) {
        if !self.visible {
            return;
        }
        let (w, h) = self.palette.size_px(self.scale);
        let Some(mut pixmap) = Pixmap::new(w, h) else {
            return;
        };
        self.palette
            .render(&mut pixmap.as_mut(), self.scale, ChannelOrder::Rgba, look);
        self.view.set_pixels(pixmap, self.scale as f64);
        let (pw, ph) = self.size();
        let frame = Frame {
            x: self.origin.0,
            y: self.origin.1,
            w: pw,
            h: ph,
        };
        self.panel
            .setFrame_display(frame.to_cocoa(self.mtm()), true);
    }

    pub fn show(&mut self, look: &Look) {
        self.visible = true;
        self.scale = scale_at(self.mtm(), self.center());
        self.render(look);
        self.panel.orderFrontRegardless();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.drag = None;
        self.palette.hover = None;
        self.tooltip.hide();
        self.panel.orderOut(None);
    }

    pub fn center(&self) -> (f64, f64) {
        let (w, h) = self.size();
        (self.origin.0 + w / 2.0, self.origin.1 + h / 2.0)
    }

    /// Moves the top-left corner to `to`, keeping the palette on its screen's work area
    /// and snapping it to an edge when close.
    pub fn move_to(&mut self, to: (f64, f64), look: &Look) {
        let (w, h) = self.size();
        let area = work_area(self.mtm(), (to.0 + w / 2.0, to.1 + h / 2.0));
        let (margin, snap) = (8.0, 16.0);
        let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi.max(lo));
        let (right, bottom) = (area.x + area.w, area.y + area.h);
        let mut x = clamp(to.0, area.x, right - w);
        let mut y = clamp(to.1, area.y, bottom - h);
        if (x - area.x).abs() < snap {
            x = area.x + margin;
        } else if (right - w - x).abs() < snap {
            x = right - w - margin;
        }
        if (y - area.y).abs() < snap {
            y = area.y + margin;
        } else if (bottom - h - y).abs() < snap {
            y = bottom - h - margin;
        }
        self.origin = (x, y);
        self.scale = scale_at(self.mtm(), self.center());
        self.render(look);
    }

    /// Top center of the work area containing `at`.
    pub fn default_origin(&self, at: (f64, f64)) -> (f64, f64) {
        let area = work_area(self.mtm(), at);
        let (w, _) = self.palette.size_px(1.0);
        (area.x + (area.w - w as f64) / 2.0, area.y + 12.0)
    }
}
