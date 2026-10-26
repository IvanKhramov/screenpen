//! A top-down 32-bit DIB that tiny-skia draws into and `UpdateLayeredWindow` presents.
//!
//! The DIB is BGRA while tiny-skia writes RGBA, so callers swap red and blue in the
//! colors they paint with (`bgra`). Both are premultiplied, which is what
//! `UpdateLayeredWindow` with `AC_SRC_ALPHA` expects.

use std::ffi::c_void;
use std::ptr::null_mut;

use tiny_skia::{Color, PixmapMut};
use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush,
    HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

pub struct Surface {
    pub width: u32,
    pub height: u32,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Option<Surface> {
        let width = width.max(1);
        let height = height.max(1);
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits: *mut c_void = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            Some(Surface {
                width,
                height,
                dc,
                bitmap,
                old,
                bits: bits.cast(),
            })
        }
    }

    /// A tiny-skia view of the DIB pixels. Colors must go through `bgra`.
    pub fn pixmap(&mut self) -> PixmapMut<'_> {
        let len = self.width as usize * self.height as usize * 4;
        unsafe {
            GdiFlush();
            let bytes = std::slice::from_raw_parts_mut(self.bits, len);
            PixmapMut::from_bytes(bytes, self.width, self.height).expect("surface size")
        }
    }

    /// Pushes the pixels to a layered window at screen position `x, y`.
    /// `height` may be less than the surface height (the 1px-short trick).
    pub fn present(&self, hwnd: HWND, x: i32, y: i32, height: u32) -> bool {
        let pos = POINT { x, y };
        let size = SIZE {
            cx: self.width as i32,
            cy: height.min(self.height) as i32,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                null_mut(),
                &pos,
                &size,
                self.dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            ) != 0
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// An RGBA color with red and blue swapped, for painting into the BGRA DIB.
pub fn bgra(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(b, g, r, a)
}
