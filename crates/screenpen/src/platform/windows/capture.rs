//! Screen capture for freeze and snapshots, PNG files and the clipboard.

use std::path::PathBuf;
use std::ptr::null_mut;

use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::{GlobalFree, HWND, RECT, SYSTEMTIME};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, GetDC, ReleaseDC, SRCCOPY,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_DIB;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use super::surface::Surface;
use super::ui::set_capture_hidden;

/// The screen inside `rect` without the given windows, as opaque BGRA pixels.
/// The windows stay excluded from capture afterwards; the caller restores them.
pub fn screen(rect: RECT, exclude: &[HWND]) -> Option<Pixmap> {
    for &hwnd in exclude {
        set_capture_hidden(hwnd, true);
    }
    let (w, h) = (
        (rect.right - rect.left) as u32,
        (rect.bottom - rect.top) as u32,
    );
    let mut surface = Surface::new(w, h)?;
    unsafe {
        // Let the compositor drop the excluded windows before reading.
        DwmFlush();
        let screen = GetDC(null_mut());
        BitBlt(
            surface.dc(),
            0,
            0,
            w as i32,
            h as i32,
            screen,
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(null_mut(), screen);
    }
    let mut pixmap = Pixmap::new(w, h)?;
    pixmap
        .data_mut()
        .copy_from_slice(surface.pixmap().data_mut());
    // GDI leaves alpha at zero.
    for px in pixmap.data_mut().as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Some(pixmap)
}

/// Pictures\screenpen\screenpen-YYYY-MM-DD-HHMMSS.png
fn snapshot_path() -> PathBuf {
    let base = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    let name = format!(
        "screenpen-{:04}-{:02}-{:02}-{:02}{:02}{:02}.png",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    base.join("Pictures").join("screenpen").join(name)
}

/// Writes a BGRA pixmap as a PNG; returns the path.
pub fn save_png(bgra: &Pixmap) -> std::io::Result<PathBuf> {
    let path = snapshot_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut rgba = bgra.clone();
    for px in rgba.data_mut().as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    rgba.save_png(&path).map_err(std::io::Error::other)?;
    Ok(path)
}

/// Puts an opaque BGRA pixmap on the clipboard as a bottom-up 32-bit DIB.
pub fn copy_to_clipboard(bgra: &Pixmap, owner: HWND) -> bool {
    let (w, h) = (bgra.width() as usize, bgra.height() as usize);
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: h as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };
    let row = w * 4;
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, size_of::<BITMAPINFOHEADER>() + row * h);
        if memory.is_null() {
            return false;
        }
        let dst = GlobalLock(memory).cast::<u8>();
        if dst.is_null() {
            GlobalFree(memory);
            return false;
        }
        std::ptr::copy_nonoverlapping(
            (&header as *const BITMAPINFOHEADER).cast::<u8>(),
            dst,
            size_of::<BITMAPINFOHEADER>(),
        );
        let pixels = dst.add(size_of::<BITMAPINFOHEADER>());
        let src = bgra.data();
        for y in 0..h {
            // Bottom-up: the last row first.
            std::ptr::copy_nonoverlapping(
                src[(h - 1 - y) * row..].as_ptr(),
                pixels.add(y * row),
                row,
            );
        }
        GlobalUnlock(memory);
        // Another app may hold the clipboard for a moment.
        let mut open = false;
        for _ in 0..5 {
            if OpenClipboard(owner) != 0 {
                open = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !open {
            GlobalFree(memory);
            return false;
        }
        EmptyClipboard();
        // On success the clipboard owns the memory; otherwise it's still ours.
        let ok = !SetClipboardData(CF_DIB as u32, memory).is_null();
        if !ok {
            GlobalFree(memory);
        }
        CloseClipboard();
        ok
    }
}
