//! Snapshots on disk: `<pictures>/screenpen/<stem>.png` is the screen with the ink,
//! for sharing; `.screenpen/<stem>.json` (the annotations) and
//! `.screenpen/<stem>.background.png` (the screen without ink) make it restorable and
//! exportable to PDF.

use std::path::{Path, PathBuf};

use screenpen_core::Annotation;
use screenpen_core::annotation::next_id;
use screenpen_core::pdf::{RgbImage, export};
use screenpen_core::render::ChannelOrder;
use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;

#[derive(Debug, Serialize, Deserialize)]
pub struct SnapshotData {
    /// Screen size in pixels and its scale; annotations are in points.
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub annotations: Vec<Annotation>,
}

pub struct Entry {
    pub stem: String,
    pub image: PathBuf,
    /// Has the sidecar data, so it can be restored and exported.
    pub restorable: bool,
}

/// Where snapshots go: the user's pictures folder.
pub fn dir() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let pictures = if cfg!(target_os = "linux") {
        linux_pictures(&home)
    } else {
        home.join("Pictures")
    };
    pictures.join("screenpen")
}

/// XDG_PICTURES_DIR from user-dirs.dirs, as the file manager names it.
fn linux_pictures(home: &Path) -> PathBuf {
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

fn sidecar(stem: &str, suffix: &str) -> PathBuf {
    dir().join(".screenpen").join(format!("{stem}{suffix}"))
}

/// A copy in RGBA byte order, which PNG wants. The red/blue swap works both ways, so
/// it also turns a loaded RGBA image into the canvas `order`.
pub fn to_rgba(pixmap: &Pixmap, order: ChannelOrder) -> Pixmap {
    let mut rgba = pixmap.clone();
    if order == ChannelOrder::Bgra {
        for px in rgba.data_mut().as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    rgba
}

/// Saves a snapshot; `image` and `background` are in the canvas byte `order`.
/// Returns the PNG path.
pub fn save(
    stem: &str,
    image: &Pixmap,
    background: Option<&Pixmap>,
    order: ChannelOrder,
    annotations: &[Annotation],
    scale: f32,
) -> std::io::Result<PathBuf> {
    let png = dir().join(format!("{stem}.png"));
    std::fs::create_dir_all(dir().join(".screenpen"))?;
    to_rgba(image, order)
        .save_png(&png)
        .map_err(std::io::Error::other)?;
    if let Some(bg) = background {
        to_rgba(bg, order)
            .save_png(sidecar(stem, ".background.png"))
            .map_err(std::io::Error::other)?;
    }
    let data = SnapshotData {
        width: image.width(),
        height: image.height(),
        scale,
        annotations: annotations.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&data).map_err(std::io::Error::other)?;
    std::fs::write(sidecar(stem, ".json"), json)?;
    Ok(png)
}

/// Snapshots in the folder, newest first.
pub fn list() -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut entries: Vec<(std::time::SystemTime, Entry)> = read
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "png" {
                return None;
            }
            let stem = path.file_stem()?.to_string_lossy().to_string();
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            let restorable = sidecar(&stem, ".json").exists();
            Some((
                modified,
                Entry {
                    stem,
                    image: path,
                    restorable,
                },
            ))
        })
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.0));
    entries.into_iter().map(|(_, e)| e).collect()
}

/// The annotations (with fresh ids, so they don't clash with ones on screen) and the
/// background in RGBA, if it was saved.
pub fn load(stem: &str) -> Result<(SnapshotData, Option<Pixmap>), String> {
    let json = std::fs::read(sidecar(stem, ".json")).map_err(|e| e.to_string())?;
    let mut data: SnapshotData = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
    for a in &mut data.annotations {
        a.id = next_id();
    }
    let background = Pixmap::load_png(sidecar(stem, ".background.png")).ok();
    Ok((data, background))
}

/// Writes `<stem>.pdf` next to the PNG: the background as an image, the ink as vectors.
pub fn export_pdf(stem: &str) -> Result<PathBuf, String> {
    let (data, background) = load(stem)?;
    let rgb = background.map(|bg| RgbImage {
        width: bg.width(),
        height: bg.height(),
        // Opaque screen pixels: premultiplied equals straight.
        rgb: bg
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect(),
    });
    let (w, h) = (
        data.width as f32 / data.scale,
        data.height as f32 / data.scale,
    );
    let pdf = export(&data.annotations, w, h, rgb.as_ref());
    let path = dir().join(format!("{stem}.pdf"));
    std::fs::write(&path, pdf).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn delete(stem: &str) {
    for path in [
        dir().join(format!("{stem}.png")),
        dir().join(format!("{stem}.pdf")),
        sidecar(stem, ".json"),
        sidecar(stem, ".background.png"),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// The settings window asks the running app to restore a snapshot through this file
/// (next to the settings), then sends the usual reload signal.
fn request_path() -> PathBuf {
    crate::config::default_path().with_file_name("restore.request")
}

pub fn request_restore(stem: &str) -> std::io::Result<()> {
    let path = request_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, stem)
}

/// The pending restore request, removed once read.
pub fn take_restore_request() -> Option<String> {
    let path = request_path();
    let stem = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(stem.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenpen_core::{Geometry, Point, Rgba, Style};

    #[test]
    fn save_list_load_export_delete() {
        let home = std::env::temp_dir().join(format!("screenpen-snap-{}", std::process::id()));
        // Point every per-OS lookup at the temp folder.
        // SAFETY: tests in this module run on one thread and nothing else reads these.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
            std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        }
        let mut image = Pixmap::new(80, 60).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        let a = Annotation::new(
            Geometry::Line {
                from: Point::new(1.0, 1.0),
                to: Point::new(30.0, 20.0),
            },
            Style::new(Rgba::RED, 3.0),
        );
        let png = save(
            "shot",
            &image,
            Some(&image),
            ChannelOrder::Bgra,
            std::slice::from_ref(&a),
            2.0,
        )
        .unwrap();
        assert!(png.exists());
        let entries = list();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].restorable);
        let (data, bg) = load("shot").unwrap();
        assert_eq!(data.annotations.len(), 1);
        assert_ne!(data.annotations[0].id, a.id, "restored ink gets fresh ids");
        // Saved from BGRA: blue in the canvas order is red-first on disk.
        assert_eq!(bg.unwrap().pixel(0, 0).unwrap().red(), 255);
        let pdf = export_pdf("shot").unwrap();
        assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
        request_restore("shot").unwrap();
        assert_eq!(take_restore_request().as_deref(), Some("shot"));
        assert_eq!(take_restore_request(), None);
        delete("shot");
        assert!(list().is_empty());
        std::fs::remove_dir_all(home).ok();
    }
}
