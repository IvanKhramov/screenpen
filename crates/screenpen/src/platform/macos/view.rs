//! The Objective-C classes screenpen defines: the overlay panel (a borderless panel that
//! may become key), the view that shows a tiny-skia pixmap and forwards input, and the
//! target object for menu items, the fade timer and screen-change notifications.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSGraphicsContext, NSImage, NSMenu, NSMenuDelegate,
    NSMenuItem, NSPanel, NSScreenSaverWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGBlendMode, CGColorRenderingIntent, CGColorSpace, CGContext, CGDataProvider,
    CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSRect};
use tiny_skia::{IntRect, Pixmap, PixmapMut};

use super::Input;

define_class!(
    // SAFETY: NSPanel has no subclassing requirements; no Drop.
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    pub struct OverlayPanel;

    impl OverlayPanel {
        // Borderless panels refuse key status by default; the canvas needs keys.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

#[derive(Default)]
pub struct ViewIvars {
    /// Premultiplied RGBA, in backing pixels.
    pub pixels: RefCell<Option<Pixmap>>,
    /// Backing pixels per point.
    pub scale: Cell<f64>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    pub struct PixelView;

    impl PixelView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            self.draw(dirty);
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // The first click on a panel of an inactive app should draw, not just focus.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            super::on_input(self, event, Input::LeftDown);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Drag);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Up);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            super::on_input(self, event, Input::RightDown);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Drag);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Up);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Move);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Move);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Exit);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Wheel);
        }

        #[unsafe(method(tabletProximity:))]
        fn tablet_proximity(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Proximity);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            super::on_input(self, event, Input::Key);
        }
    }
);

impl PixelView {
    pub fn new(mtm: MainThreadMarker, size: CGSize) -> Retained<PixelView> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars::default());
        let frame = CGRect::new(CGPoint::ZERO, size);
        let view: Retained<PixelView> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // Hover and the eraser ring need moves even while another app is active.
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                frame,
                options,
                Some(&view),
                None,
            )
        };
        view.addTrackingArea(&area);
        view
    }

    /// Replaces the pixels and shows all of them.
    pub fn set_pixels(&self, pixmap: Pixmap, scale: f64) {
        *self.ivars().pixels.borrow_mut() = Some(pixmap);
        self.ivars().scale.set(scale);
        self.setNeedsDisplay(true);
    }

    /// Lets `f` draw into the pixels; None if there are none yet.
    pub fn draw_into<R>(&self, f: impl FnOnce(&mut PixmapMut) -> R) -> Option<R> {
        let mut pixels = self.ivars().pixels.borrow_mut();
        Some(f(&mut pixels.as_mut()?.as_mut()))
    }

    /// Marks a rectangle of pixels, or all of them, to be shown on the next display pass.
    pub fn invalidate(&self, dirty: Option<IntRect>) {
        let Some(r) = dirty else {
            self.setNeedsDisplay(true);
            return;
        };
        let s = self.ivars().scale.get();
        let view_h = self.bounds().size.height;
        let rect = CGRect::new(
            CGPoint::new(r.left() as f64 / s, view_h - r.bottom() as f64 / s),
            CGSize::new(r.width() as f64 / s, r.height() as f64 / s),
        );
        self.setNeedsDisplayInRect(rect);
    }

    /// Copies the dirty part of the pixmap into the view. The copy keeps CoreGraphics
    /// from holding on to pixels we're about to change or free.
    fn draw(&self, dirty: NSRect) {
        let pixels = self.ivars().pixels.borrow();
        let Some(pixmap) = pixels.as_ref() else {
            return;
        };
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = context.CGContext();
        let s = self.ivars().scale.get();
        let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
        let view_h = h as f64 / s;
        // The view isn't flipped: y grows up, pixmap rows go down.
        let x0 = ((dirty.origin.x * s).floor().max(0.0) as usize).min(w);
        let x1 = (((dirty.origin.x + dirty.size.width) * s).ceil().max(0.0) as usize).min(w);
        let top = view_h - (dirty.origin.y + dirty.size.height);
        let y0 = ((top * s).floor().max(0.0) as usize).min(h);
        let y1 = (((top + dirty.size.height) * s).ceil().max(0.0) as usize).min(h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let data = pixmap.data();
        let mut region = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for y in y0..y1 {
            region.extend_from_slice(&data[(y * w + x0) * 4..(y * w + x1) * 4]);
        }
        let Some(image) = image((x1 - x0, y1 - y0), &region) else {
            return;
        };
        let rect = CGRect::new(
            CGPoint::new(x0 as f64 / s, view_h - y1 as f64 / s),
            CGSize::new((x1 - x0) as f64 / s, (y1 - y0) as f64 / s),
        );
        // Replace, don't blend: the pixmap already holds the whole picture.
        CGContext::set_blend_mode(Some(&cg), CGBlendMode::Copy);
        CGContext::draw_image(Some(&cg), rect, Some(&image));
    }
}

/// A CGImage of tightly packed premultiplied RGBA pixels.
pub fn image(
    (w, h): (usize, usize),
    rgba: &[u8],
) -> Option<objc2_core_foundation::CFRetained<CGImage>> {
    let data = CFData::from_bytes(rgba);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    unsafe {
        CGImage::new(
            w,
            h,
            8,
            32,
            w * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

/// An NSImage of a pixmap, `points` wide and high.
pub fn ns_image(pixmap: &Pixmap, points: f64) -> Option<Retained<NSImage>> {
    let cg = image(
        (pixmap.width() as usize, pixmap.height() as usize),
        pixmap.data(),
    )?;
    let size = CGSize::new(points, points);
    Some(NSImage::initWithCGImage_size(NSImage::alloc(), &cg, size))
}

/// A borderless, non-activating, transparent panel on every Space, above full-screen
/// apps. `above` raises it over the overlays (palette, tooltip, cheat sheet).
pub fn panel(mtm: MainThreadMarker, frame: CGRect, above: isize) -> Retained<OverlayPanel> {
    let this = OverlayPanel::alloc(mtm).set_ivars(());
    let panel: Retained<OverlayPanel> = unsafe {
        msg_send![
            super(this),
            initWithContentRect: frame,
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered,
            defer: false
        ]
    };
    panel.setLevel(NSScreenSaverWindowLevel + above);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setOpaque(false);
    panel.setHasShadow(false);
    // SAFETY: we own the panel through `Retained`; it must not free itself on close.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setHidesOnDeactivate(false);
    panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
    panel.setAcceptsMouseMovedEvents(true);
    panel
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        // Rebuilt on every open so checkmarks and keys are current.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            super::on_menu_open(menu);
        }
    }

    impl Target {
        #[unsafe(method(menuItem:))]
        fn menu_item(&self, item: &NSMenuItem) {
            super::on_menu_item(item.tag());
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &AnyObject) {
            super::on_tick();
        }

        #[unsafe(method(noticeDone:))]
        fn notice_done(&self, _timer: &AnyObject) {
            super::on_notice_done();
        }

        #[unsafe(method(secondInstance:))]
        fn second_instance(&self, _note: &NSNotification) {
            super::on_second_instance();
        }

        #[unsafe(method(settingsChanged:))]
        fn settings_changed(&self, _note: &NSNotification) {
            super::on_settings_changed();
        }

        #[unsafe(method(screensChanged:))]
        fn screens_changed(&self, _note: &NSNotification) {
            super::on_screens_changed();
        }
    }
);

impl Target {
    pub fn new(mtm: MainThreadMarker) -> Retained<Target> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}
