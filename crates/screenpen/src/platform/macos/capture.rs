//! Screen capture for freeze and snapshots, PNG files and the pasteboard.
//!
//! The capture takes every window below an overlay panel, so our own panels stay out
//! of it. It needs the Screen Recording permission; without it macOS would hand back
//! only the wallpaper, so we ask for the permission and capture nothing.

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

/// screenpen-YYYY-MM-DD-HHmmss in local time.
pub fn snapshot_stem() -> String {
    let formatter = NSDateFormatter::new();
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd-HHmmss")));
    format!("screenpen-{}", formatter.stringFromDate(&NSDate::now()))
}

/// Puts an RGBA image on the pasteboard as PNG.
pub fn copy_to_pasteboard(image: &Pixmap) -> bool {
    let Ok(png) = image.encode_png() else {
        return false;
    };
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setData_forType(Some(&NSData::from_vec(png)), unsafe { NSPasteboardTypePNG })
}
