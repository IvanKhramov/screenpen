//! The brush cursor: a dot as wide as the stroke, in its color.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSCursor, NSImage};
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::NSData;
use screenpen_core::render::ChannelOrder;
use screenpen_core::{Style, ToolKind};

use crate::palette::render_cursor;

/// The last brush cursor made and what it shows, so it is rebuilt only on change.
#[derive(Default)]
pub struct BrushCursor {
    made: Option<(String, Retained<NSCursor>)>,
}

impl BrushCursor {
    /// The cursor for `tool` drawing with `style` on a screen with `scale` pixels per
    /// point, or None for tools that keep a system cursor.
    pub fn get(&mut self, tool: ToolKind, style: &Style, scale: f32) -> Option<Retained<NSCursor>> {
        let key = format!("{tool:?}{:?}{}{scale}", style.color, style.width);
        if let Some((k, cursor)) = &self.made
            && *k == key
        {
            return Some(cursor.clone());
        }
        let (pixmap, hot) = render_cursor(tool, style, scale, ChannelOrder::Rgba)?;
        let png = pixmap.encode_png().ok()?;
        let image = NSImage::initWithData(NSImage::alloc(), &NSData::from_vec(png))?;
        let s = scale as f64;
        // Pixels to points, so a Retina screen gets a sharp cursor of the right size.
        image.setSize(CGSize::new(
            pixmap.width() as f64 / s,
            pixmap.height() as f64 / s,
        ));
        let cursor = NSCursor::initWithImage_hotSpot(
            NSCursor::alloc(),
            &image,
            CGPoint::new(hot.0 as f64 / s, hot.1 as f64 / s),
        );
        self.made = Some((key, cursor.clone()));
        Some(cursor)
    }
}
