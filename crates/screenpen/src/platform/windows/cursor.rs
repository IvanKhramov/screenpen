//! The brush cursor: a dot as wide as the stroke, in its color.

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyCursor, HCURSOR, ICONINFO,
};

use screenpen_core::render::ChannelOrder;
use screenpen_core::{Style, ToolKind};

use crate::palette::render_cursor;

/// The last brush cursor made and what it shows, so it is rebuilt only on change.
#[derive(Default)]
pub struct BrushCursor {
    made: Option<(String, HCURSOR)>,
}

impl BrushCursor {
    /// The cursor for `tool` drawing with `style` at `scale`, or None for tools that
    /// keep a system cursor (or when making one fails).
    pub fn get(&mut self, tool: ToolKind, style: &Style, scale: f32) -> Option<HCURSOR> {
        let key = format!("{tool:?}{:?}{}{scale}", style.color, style.width);
        if let Some((k, cursor)) = &self.made
            && *k == key
        {
            return Some(*cursor);
        }
        let (pixmap, hot) = render_cursor(tool, style, scale, ChannelOrder::Bgra)?;
        let cursor = make(&pixmap, hot)?;
        if let Some((_, old)) = self.made.replace((key, cursor)) {
            unsafe { DestroyCursor(old) };
        }
        Some(cursor)
    }
}

/// A 32-bit cursor from a premultiplied BGRA pixmap.
fn make(pixmap: &tiny_skia::Pixmap, hot: (u32, u32)) -> Option<HCURSOR> {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        // Negative: rows top to bottom, like the pixmap.
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };
    let mut bits = std::ptr::null_mut();
    unsafe {
        let color = CreateDIBSection(
            std::ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if color.is_null() || bits.is_null() {
            return None;
        }
        // Cursors take straight alpha; the pixmap holds B, G, R in its r, g, b.
        let out = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (dst, p) in out.as_chunks_mut::<4>().0.iter_mut().zip(pixmap.pixels()) {
            let c = p.demultiply();
            *dst = [c.red(), c.green(), c.blue(), c.alpha()];
        }
        // The AND mask is unused with an alpha channel, but must exist (all zero).
        let row = ((w + 15) / 16 * 2) as usize;
        let zeros = vec![0u8; row * h as usize];
        let mask = CreateBitmap(w, h, 1, 1, zeros.as_ptr().cast());
        let icon = ICONINFO {
            fIcon: 0,
            xHotspot: hot.0,
            yHotspot: hot.1,
            hbmMask: mask,
            hbmColor: color,
        };
        let cursor = CreateIconIndirect(&icon);
        DeleteObject(color);
        DeleteObject(mask);
        (!cursor.is_null()).then_some(cursor)
    }
}
