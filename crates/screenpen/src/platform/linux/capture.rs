//! Snapshot files: $XDG_PICTURES_DIR/screenpen (usually ~/Pictures/screenpen).

use std::path::PathBuf;

use tiny_skia::Pixmap;

/// The pictures folder from user-dirs.dirs, as the desktop's file manager names it.
fn pictures_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|dirs| {
            dirs.lines().find_map(|l| {
                let value = l.strip_prefix("XDG_PICTURES_DIR=")?.trim_matches('"');
                Some(match value.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None => PathBuf::from(value),
                })
            })
        })
        .unwrap_or_else(|| home.join("Pictures"))
}

/// screenpen-YYYY-MM-DD-HHMMSS.png in local time.
fn snapshot_path() -> PathBuf {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        libc::localtime_r(&now, &mut tm);
    }
    let name = format!(
        "screenpen-{:04}-{:02}-{:02}-{:02}{:02}{:02}.png",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    );
    pictures_dir().join("screenpen").join(name)
}

/// A BGRA pixmap as RGBA, the order PNG wants.
pub fn to_rgba(bgra: &Pixmap) -> Pixmap {
    let mut rgba = bgra.clone();
    for px in rgba.data_mut().as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    rgba
}

/// Writes an RGBA pixmap as a PNG; returns the path.
pub fn save_png(rgba: &Pixmap) -> std::io::Result<PathBuf> {
    let path = snapshot_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    rgba.save_png(&path).map_err(std::io::Error::other)?;
    Ok(path)
}
