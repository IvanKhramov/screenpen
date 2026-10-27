//! The palette window, its tooltip and the cheat sheet. Palette layout and drawing
//! live in `crate::palette`; this is the Win32 side: windows, dragging, snapping.

use std::time::Instant;

use screenpen_core::Point;
use screenpen_core::render::ChannelOrder;
use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use super::surface::Surface;
use super::{create_window, log, monitor_info};
use crate::palette::{Look, Palette};

/// A layered, never-activated window showing a rendered pixmap (tooltip, cheat sheet).
pub struct Popup {
    pub hwnd: HWND,
    surface: Option<Surface>,
    pub visible: bool,
}

impl Popup {
    /// `click_through` popups never take the mouse (tooltips).
    pub fn new(class: &str, click_through: bool) -> Popup {
        let mut ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if click_through {
            ex |= WS_EX_TRANSPARENT;
        }
        Popup {
            hwnd: create_window(class, ex, 1, 1),
            surface: None,
            visible: false,
        }
    }

    pub fn show(&mut self, pixmap: &Pixmap, at: POINT) {
        let Some(surface) = Surface::from_pixmap(pixmap) else {
            return;
        };
        if !surface.present(self.hwnd, at.x, at.y, surface.height, None) {
            log("popup present failed");
        }
        self.surface = Some(surface);
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNA);
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        self.visible = true;
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.visible = false;
        }
    }
}

pub fn set_capture_hidden(hwnd: HWND, hidden: bool) -> bool {
    let affinity = if hidden {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) != 0 }
}

pub fn work_area(at: POINT) -> RECT {
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST), &mut info);
        info.rcWork
    }
}

pub fn scale_at(at: POINT) -> f32 {
    monitor_info(unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) }).1
}

pub struct PaletteWindow {
    pub hwnd: HWND,
    surface: Option<Surface>,
    /// The user's interface scale, multiplied into `scale`.
    pub ui_scale: f32,
    pub scale: f32,
    pub origin: POINT,
    pub palette: Palette,
    /// Cursor offset from the origin while dragging by the grip.
    pub drag: Option<POINT>,
    pub last_grip_click: Option<Instant>,
    pub tooltip: Popup,
    /// The pointer is over the grip (its tooltip explains dragging and rotating).
    pub grip_hover: bool,
    pub visible: bool,
}

impl PaletteWindow {
    pub fn new(class: &str, tooltip_class: &str, palette: Palette) -> PaletteWindow {
        let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        PaletteWindow {
            hwnd: create_window(class, ex, 1, 1),
            surface: None,
            scale: 1.0,
            ui_scale: 1.0,
            origin: POINT { x: 0, y: 0 },
            palette,
            drag: None,
            last_grip_click: None,
            tooltip: Popup::new(tooltip_class, true),
            grip_hover: false,
            visible: false,
        }
    }

    pub fn size(&self) -> (i32, i32) {
        let (w, h) = self.palette.size_px(self.scale);
        (w as i32, h as i32)
    }

    /// Screen pixels to palette points.
    pub fn to_local(&self, p: POINT) -> Point {
        Point::new(
            (p.x - self.origin.x) as f32 / self.scale,
            (p.y - self.origin.y) as f32 / self.scale,
        )
    }

    pub fn render(&mut self, look: &Look) {
        if !self.visible {
            return;
        }
        // The context row opens on the side facing the screen's middle, and the main
        // bar stays where it is while the window grows around it.
        let before = self.palette.main_offset();
        let c = self.center();
        let area = work_area(c);
        self.palette.context_first = if self.palette.vertical {
            c.x > (area.left + area.right) / 2
        } else {
            c.y > (area.top + area.bottom) / 2
        };
        self.palette.sync(look);
        let after = self.palette.main_offset();
        self.origin.x -= ((after.0 - before.0) * self.scale).round() as i32;
        self.origin.y -= ((after.1 - before.1) * self.scale).round() as i32;
        let (w, h) = self.palette.size_px(self.scale);
        if self
            .surface
            .as_ref()
            .is_none_or(|s| s.width != w || s.height != h)
        {
            self.surface = Surface::new(w, h);
        }
        let Some(surface) = &mut self.surface else {
            return;
        };
        self.palette
            .render(&mut surface.pixmap(), self.scale, ChannelOrder::Bgra, look);
        surface.present(self.hwnd, self.origin.x, self.origin.y, h, None);
    }

    pub fn show(&mut self, look: &Look) {
        self.visible = true;
        self.scale = scale_at(self.center()) * self.ui_scale;
        self.render(look);
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNA);
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.drag = None;
        self.palette.hover = None;
        self.tooltip.hide();
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub fn center(&self) -> POINT {
        let (w, h) = self.size();
        POINT {
            x: self.origin.x + w / 2,
            y: self.origin.y + h / 2,
        }
    }

    /// Moves the top-left corner to `to`, keeping the palette on its monitor's work area
    /// and snapping it to an edge when close.
    pub fn move_to(&mut self, to: POINT, look: &Look) {
        let (w, h) = self.size();
        let area = work_area(POINT {
            x: to.x + w / 2,
            y: to.y + h / 2,
        });
        let margin = (8.0 * self.scale) as i32;
        let snap = (16.0 * self.scale) as i32;
        let clamp = |v: i32, lo: i32, hi: i32| v.max(lo).min(hi.max(lo));
        let mut x = clamp(to.x, area.left, area.right - w);
        let mut y = clamp(to.y, area.top, area.bottom - h);
        if (x - area.left).abs() < snap {
            x = area.left + margin;
        } else if (area.right - w - x).abs() < snap {
            x = area.right - w - margin;
        }
        if (y - area.top).abs() < snap {
            y = area.top + margin;
        } else if (area.bottom - h - y).abs() < snap {
            y = area.bottom - h - margin;
        }
        self.origin = POINT { x, y };
        let scale = scale_at(self.center()) * self.ui_scale;
        if scale != self.scale {
            self.scale = scale;
        }
        self.render(look);
    }

    /// Bottom center of the work area containing `at`.
    pub fn default_origin(&self, at: POINT) -> POINT {
        let area = work_area(at);
        let scale = scale_at(at) * self.ui_scale;
        let (w, h) = self.palette.size_px(scale);
        POINT {
            x: area.left + (area.right - area.left - w as i32) / 2,
            y: area.bottom - h as i32 - (16.0 * scale) as i32,
        }
    }
}
