//! Screen capture for freeze and snapshots, PNG files and the pasteboard.
//!
//! The capture takes every window below an overlay panel, so our own panels stay out
//! of it. It needs the Screen Recording permission; without it macOS would hand back
//! only the wallpaper, so we ask for the permission and capture nothing.

use std::path::PathBuf;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess, CGWindowImageOption,
    CGWindowListOption,
};
use objc2_foundation::{NSData, NSDate, NSDateFormatter, NSString};
use tiny_skia::Pixmap;

use super::Frame;

/// The screen at `frame` under the window `below`, as a `size` RGBA pixmap.
pub fn screen(frame: Frame, below: isize, size: (u32, u32)) -> Result<Pixmap, String> {
    if !CGPreflightScreenCaptureAccess() {
        // Shows the system prompt once; the grant applies after screenpen restarts.
        CGRequestScreenCaptureAccess();
        return Err("needs the Screen Recording permission".to_string());
    }
    // CoreGraphics global coordinates start at the primary screen's top-left, like `Frame`.
    let rect = CGRect::new(
        CGPoint::new(frame.x, frame.y),
        CGSize::new(frame.w, frame.h),
    );
    // The ScreenCaptureKit replacement is async-only; this still works on macOS 13-15.
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGWindowListCreateImage(
        rect,
        CGWindowListOption::OptionOnScreenBelowWindow,
        below as u32,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
    )
    .ok_or("capture failed")?;
    let (w, h) = size;
    let mut pixmap = Pixmap::new(w, h).ok_or("out of memory")?;
    let space = CGColorSpace::new_device_rgb().ok_or("no color space")?;
    let context = unsafe {
        CGBitmapContextCreate(
            pixmap.data_mut().as_mut_ptr().cast(),
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .ok_or("no bitmap context")?;
    let all = CGRect::new(CGPoint::ZERO, CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&context), all, Some(&image));
    drop(context);
    Ok(pixmap)
}

fn snapshot_path() -> PathBuf {
    let formatter = NSDateFormatter::new();
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd-HHmmss")));
    let stamp = formatter.stringFromDate(&NSDate::now());
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Pictures")
        .join("screenpen")
        .join(format!("screenpen-{stamp}.png"))
}

/// Writes the image to ~/Pictures/screenpen and puts it on the pasteboard.
pub fn save(image: &Pixmap) -> Result<PathBuf, String> {
    let png = image.encode_png().map_err(|e| e.to_string())?;
    let path = snapshot_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, &png).map_err(|e| e.to_string())?;
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setData_forType(Some(&NSData::from_vec(png)), unsafe { NSPasteboardTypePNG });
    Ok(path)
}
