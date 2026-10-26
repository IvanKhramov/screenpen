//! The magnifier: a round lens following the pointer, drawn by the system
//! Magnification API so it can leave our own windows (overlays, palette, lens) out.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{CreateEllipticRgn, SetWindowRgn};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Magnification::{
    MAGTRANSFORM, MW_FILTERMODE_EXCLUDE, MagInitialize, MagSetWindowFilterList, MagSetWindowSource,
    MagSetWindowTransform, MagUninitialize, WC_MAGNIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, LWA_ALPHA, SW_HIDE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WS_CHILD, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_VISIBLE,
};

use super::{create_window, log};
use crate::palette::{LENS_RADIUS, LENS_ZOOM};

pub struct Magnifier {
    host: HWND,
    lens: HWND,
    pub on: bool,
    zoom: f32,
    /// Diameter the round region was made for.
    shaped: i32,
}

impl Magnifier {
    /// None when the Magnification API isn't available.
    pub fn new(class: &str) -> Option<Magnifier> {
        if unsafe { MagInitialize() } == 0 {
            log("magnifier: MagInitialize failed");
            return None;
        }
        // Click-through, never activated, above our overlays.
        let ex =
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE;
        let host = create_window(class, ex, 1, 1);
        unsafe {
            SetLayeredWindowAttributes(host, 0, 255, LWA_ALPHA);
            let lens = CreateWindowExW(
                0,
                WC_MAGNIFIER,
                null(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                1,
                1,
                host,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if lens.is_null() {
                log("magnifier: the Magnifier control was not created");
                DestroyWindow(host);
                MagUninitialize();
                return None;
            }
            Some(Magnifier {
                host,
                lens,
                on: false,
                zoom: 2.0,
                shaped: 0,
            })
        }
    }

    pub fn zoom_step(&mut self, more: bool) {
        let (min, max, step) = LENS_ZOOM;
        self.zoom = (self.zoom + if more { step } else { -step }).clamp(min, max);
    }

    /// Moves the lens to the pointer and refreshes it. `exclude` are our windows.
    pub fn update(&mut self, cursor: POINT, scale: f32, exclude: &[HWND]) {
        let d = (LENS_RADIUS * 2.0 * scale) as i32;
        let r = d / 2;
        unsafe {
            SetWindowPos(
                self.host,
                HWND_TOPMOST,
                cursor.x - r,
                cursor.y - r,
                d,
                d,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if self.shaped != d {
                SetWindowPos(self.lens, null_mut(), 0, 0, d, d, SWP_NOACTIVATE);
                // The window owns the region after this call.
                SetWindowRgn(self.host, CreateEllipticRgn(0, 0, d, d), 1);
                self.shaped = d;
            }
            let mut transform = MAGTRANSFORM {
                v: [self.zoom, 0.0, 0.0, 0.0, self.zoom, 0.0, 0.0, 0.0, 1.0],
            };
            MagSetWindowTransform(self.lens, &mut transform);
            let half = (r as f32 / self.zoom) as i32;
            let source = RECT {
                left: cursor.x - half,
                top: cursor.y - half,
                right: cursor.x + half,
                bottom: cursor.y + half,
            };
            let mut list: Vec<HWND> = exclude.iter().copied().chain([self.host]).collect();
            MagSetWindowFilterList(
                self.lens,
                MW_FILTERMODE_EXCLUDE,
                list.len() as i32,
                list.as_mut_ptr(),
            );
            MagSetWindowSource(self.lens, source);
        }
    }

    pub fn hide(&mut self) {
        self.on = false;
        unsafe { ShowWindow(self.host, SW_HIDE) };
    }
}

impl Drop for Magnifier {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.host);
            MagUninitialize();
        }
    }
}
