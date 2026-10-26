//! Snapshot files: $XDG_PICTURES_DIR/screenpen (usually ~/Pictures/screenpen).

use tiny_skia::Pixmap;

/// screenpen-YYYY-MM-DD-HHMMSS in local time.
pub fn snapshot_stem() -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        libc::localtime_r(&now, &mut tm);
    }
    format!(
        "screenpen-{:04}-{:02}-{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// A BGRA pixmap as RGBA, the order PNG wants.
pub fn to_rgba(bgra: &Pixmap) -> Pixmap {
    let mut rgba = bgra.clone();
    for px in rgba.data_mut().as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    rgba
}
