//! macOS backend: a non-activating panel per screen drawing a `Canvas`, mouse and
//! tablet input, Carbon global hotkeys, ghost (click-through) mode, the palette and the
//! menu-bar status item. Diagnostics go to $TMPDIR/screenpen.log.
//!
//! The panels never activate the app: the presenter's app keeps its menu bar and
//! focus, while the panel under the pointer becomes key so shortcuts reach the canvas.

mod autostart;
mod capture;
mod cursor;
mod hotkey;
mod instance;
mod keys;
mod tray;
mod ui;
mod view;

use std::cell::RefCell;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Child;
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSApplicationDidChangeScreenParametersNotification, NSCursor, NSEvent, NSEventSubtype, NSMenu,
    NSPointingDeviceType, NSRunningApplication, NSScreen,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{
    MainThreadMarker, NSDistributedNotificationCenter, NSLocale, NSNotificationCenter, NSString,
    NSTimer,
};
use screenpen_core::render::ChannelOrder;
use screenpen_core::{Action, Key, Rgba, ToolKind};
use tiny_skia::Pixmap;

use crate::canvas::{Button, Canvas, Pointer, Repaint, Request, typing_action};
use crate::config::{PaletteSettings, Settings};
use crate::i18n::{Lang, T};
use crate::palette::{
    Hit, Item, LENS_RADIUS, LENS_ZOOM, Look, Palette, Press, Theme, render_cheat_sheet,
    render_lens, render_tooltip,
};
use crate::snapshots;
use crate::state::{DIM_LEVELS, FADE_SECONDS, ToolState};
use hotkey::HotKeys;
use tray::{Command, MenuState, Mode, Tray};
use view::{OverlayPanel, PixelView, Target};

const GLOBAL_ACTIONS: [(u32, Action); 3] = [
    (1, Action::ToggleDrawing),
    (2, Action::ExitGhost),
    (3, Action::Quit),
];

/// A rectangle in global points, origin at the primary screen's top-left corner, y down.
/// Cocoa's own coordinates start at the bottom-left; `Canvas` and the settings want
/// top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    fn primary_height(mtm: MainThreadMarker) -> f64 {
        NSScreen::screens(mtm)
            .firstObject()
            .map_or(0.0, |s| s.frame().size.height)
    }

    pub fn from_cocoa(mtm: MainThreadMarker, r: CGRect) -> Frame {
        Frame {
            x: r.origin.x,
            y: Self::primary_height(mtm) - r.origin.y - r.size.height,
            w: r.size.width,
            h: r.size.height,
        }
    }

    pub fn to_cocoa(self, mtm: MainThreadMarker) -> CGRect {
        CGRect::new(
            CGPoint::new(self.x, Self::primary_height(mtm) - self.y - self.h),
            CGSize::new(self.w, self.h),
        )
    }

    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
}

/// The mouse position in `Frame` coordinates.
fn mouse(mtm: MainThreadMarker) -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    (p.x, Frame::primary_height(mtm) - p.y)
}

/// What a view received, as far as the app cares.
#[derive(Clone, Copy, PartialEq)]
pub enum Input {
    LeftDown,
    RightDown,
    Drag,
    Up,
    Move,
    Exit,
    Wheel,
    Key,
    /// A tablet pen came near or left; tells which end is down.
    Proximity,
}

struct Overlay {
    panel: Retained<OverlayPanel>,
    view: Retained<PixelView>,
    frame: Frame,
    canvas: Canvas,
}

struct App {
    mtm: MainThreadMarker,
    brush_cursor: cursor::BrushCursor,
    overlays: Vec<Overlay>,
    state: ToolState,
    palette: ui::PaletteWindow,
    /// Whether the palette has a position (from settings or a drag) to save.
    palette_placed: bool,
    sheet: ui::Popup,
    notice: ui::Popup,
    settings_path: PathBuf,
    /// "auto", "en" or "ru", as in the settings file.
    language: String,
    lang: Lang,
    active: bool,
    ghost: bool,
    /// The screens show a captured image under the ink.
    frozen: bool,
    /// The magnifier lens (made on first use), whether it's on, and its zoom.
    lens: Option<ui::Popup>,
    lens_on: bool,
    lens_zoom: f32,
    /// The pen near the tablet is turned to its eraser end.
    eraser: bool,
    /// Overlay index of the gesture in progress.
    pressed: Option<usize>,
    hide_palette_from_capture: bool,
    /// The settings file failed to parse: don't overwrite it.
    settings_broken: bool,
    tray: Tray,
    target: Retained<Target>,
    hotkeys: HotKeys,
    /// Drives auto-fade and the laser trail, with its interval in seconds.
    fade_timer: Option<(Retained<NSTimer>, f64)>,
    settings_window: Option<Child>,
    /// Held while running; see `instance`.
    _lock: Option<File>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("screenpen must start on the main thread");
    let settings_path = crate::config::default_path();
    let lock_path = settings_path.with_file_name("instance.lock");
    let (lock, replaced) = match instance::claim(&lock_path) {
        instance::Claim::Exit => {
            post(SECOND_INSTANCE);
            return;
        }
        instance::Claim::Continue { lock, replaced } => (lock, replaced),
    };
    log(&format!("start {}", instance::VERSION));
    let ns_app = NSApplication::sharedApplication(mtm);
    // Menu-bar only: no Dock icon, no main menu (LSUIElement in the bundle does the same).
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let target = Target::new(mtm);
    let state = ToolState::default();
    let palette = Palette::new(Item::defaults(), false);
    let sheet = ui::Popup::new(mtm, false);
    // The cheat sheet is for the presenter only.
    ui::set_capture_hidden(&sheet.panel, true);
    let app = App {
        mtm,
        overlays: create_overlays(mtm, &state),
        state,
        palette: ui::PaletteWindow::new(mtm, palette),
        palette_placed: false,
        sheet,
        notice: ui::Popup::new(mtm, true),
        settings_path,
        language: "auto".to_string(),
        lang: system_lang(),
        active: false,
        ghost: false,
        frozen: false,
        lens: None,
        lens_on: false,
        lens_zoom: 2.0,
        eraser: false,
        pressed: None,
        hide_palette_from_capture: false,
        settings_broken: false,
        tray: Tray::new(mtm, &target),
        target,
        hotkeys: HotKeys::new(),
        fade_timer: None,
        settings_window: None,
        _lock: lock,
        brush_cursor: cursor::BrushCursor::default(),
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(|app| {
        let first_run = !app.settings_path.exists();
        load_settings(app);
        if first_run {
            // Write the defaults so there's a file to look at and edit.
            save_settings(app);
            // And show how it works.
            if let Err(e) = crate::settings_ui::spawn(true) {
                log(&format!("welcome window not started: {e}"));
            }
        }
        autostart::refresh();
        let taken = register_hotkeys(app);
        let key = chord_text(app, Action::ToggleDrawing);
        let l = app.lang;
        if let Some(old) = &replaced {
            let text = format!("{old} → {}", instance::VERSION);
            notify(app, &instance::title(), &text);
        } else if taken.is_empty() {
            notify(
                app,
                l.t(T::Running),
                &l.t(T::HowToDraw).replace("{key}", &key),
            );
        } else {
            let text = l.t(T::HotkeyTakenText).replace("{keys}", &taken.join(", "));
            notify(app, l.t(T::HotkeyTaken), &text);
        }
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &app.target,
                sel!(screensChanged:),
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
            );
            let center = NSDistributedNotificationCenter::defaultCenter();
            for (name, selector) in [
                (SETTINGS_CHANGED, sel!(settingsChanged:)),
                (SECOND_INSTANCE, sel!(secondInstance:)),
            ] {
                center.addObserver_selector_name_object(
                    &app.target,
                    selector,
                    Some(&NSString::from_str(name)),
                    None,
                );
            }
        }
    });
    ns_app.run();
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        // A callback that AppKit runs while we're inside a handler finds the app
        // borrowed and is dropped.
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(app) = guard.as_mut()
        {
            f(app);
        }
    });
}

/// Registers the global shortcuts; returns the ones another app already holds.
fn register_hotkeys(app: &mut App) -> Vec<String> {
    app.hotkeys.clear();
    let mut taken = Vec::new();
    for (id, action) in GLOBAL_ACTIONS {
        let Some(chord) = app.state.shortcuts.chord(action) else {
            continue;
        };
        let Some(code) = keys::code_from_key(chord.key) else {
            continue;
        };
        let ok = app
            .hotkeys
            .register(id, code, keys::carbon_modifiers(chord));
        log(&format!(
            "hotkey {chord} for {} registered: {ok}",
            action.name()
        ));
        if !ok {
            taken.push(chord.to_string());
        }
    }
    taken
}

fn chord_text(app: &App, action: Action) -> String {
    app.state
        .shortcuts
        .chord(action)
        .map_or_else(|| app.lang.t(T::NoShortcut).to_string(), |c| c.to_string())
}

// MARK: settings

pub fn system_lang() -> Lang {
    let first = NSLocale::preferredLanguages().firstObject();
    if first.is_some_and(|l| l.to_string().starts_with("ru")) {
        Lang::Ru
    } else {
        Lang::En
    }
}

/// Reads the settings file into the app. Called at start and whenever drawing starts,
/// so edits made while screenpen runs are picked up.
fn load_settings(app: &mut App) {
    let (settings, error) = Settings::load(&app.settings_path);
    if let Some(e) = error {
        // Keep what we have and stop saving until the file is fixed, or the next
        // save would replace the user's file with defaults.
        log(&format!("settings not loaded, keeping current ones: {e}"));
        if !app.settings_broken {
            let l = app.lang;
            notify(app, l.t(T::SettingsBroken), &e);
        }
        app.settings_broken = true;
        return;
    }
    app.settings_broken = false;
    let before = app.state.shortcuts.clone();
    let mut state = ToolState {
        whiteboard: app.state.whiteboard,
        ..ToolState::default()
    };
    for problem in settings.apply(&mut state) {
        log(&format!("settings: {problem}"));
    }
    app.state = state;
    app.language = settings.language.clone();
    app.lang = settings.lang(system_lang());
    let items = settings.palette_items();
    app.palette.palette.items = if items.is_empty() {
        Item::defaults()
    } else {
        items
    };
    app.palette.palette.vertical = settings.palette.vertical;
    app.palette.ui_scale = app.state.ui_scale;
    if let (Some(x), Some(y)) = (settings.palette.x, settings.palette.y) {
        app.palette.origin = (x as f64, y as f64);
        app.palette_placed = true;
    }
    set_capture_hiding(app, settings.hide_palette_from_capture);
    if app.state.shortcuts != before {
        register_hotkeys(app);
    }
    let keys = (
        chord_text(app, Action::ToggleDrawing),
        chord_text(app, Action::ExitGhost),
    );
    app.tray.set_text(app.mtm, app.lang, keys);
}

fn save_settings(app: &App) {
    if app.settings_broken {
        log("settings file has errors; not saving over it");
        return;
    }
    let p = &app.palette;
    let palette = PaletteSettings {
        version: crate::config::PALETTE_VERSION,
        items: p.palette.items.iter().map(|i| i.name()).collect(),
        vertical: p.palette.vertical,
        x: app.palette_placed.then_some(p.origin.0.round() as i32),
        y: app.palette_placed.then_some(p.origin.1.round() as i32),
    };
    let settings = Settings::capture(
        &app.state,
        &palette,
        &app.language,
        app.hide_palette_from_capture,
    );
    if let Err(e) = settings.save(&app.settings_path) {
        log(&format!("settings not saved: {e}"));
    }
}

/// Opens the settings window, a separate process of the same binary, or brings the
/// one already open to the front.
fn open_settings(app: &mut App) {
    // A normal window would open under the screen-saver-level overlay and be unreachable.
    if app.active {
        hide(app);
    }
    save_settings(app);
    if let Some(child) = &mut app.settings_window
        && let Ok(None) = child.try_wait()
    {
        if let Some(running) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(child.id() as i32)
        {
            #[allow(deprecated)] // the replacement needs macOS 14
            running.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        }
        return;
    }
    let spawned = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn());
    match spawned {
        Ok(child) => app.settings_window = Some(child),
        Err(e) => log(&format!("settings window not started: {e}")),
    }
}

/// Posted by the settings process after it saved the file.
const SETTINGS_CHANGED: &str = "dev.screenpen.settingsChanged";
/// Posted by a second copy of the same version before it exits.
const SECOND_INSTANCE: &str = "dev.screenpen.secondInstance";

/// Tells the running screenpen (another process) about something.
fn post(name: &str) {
    let name = NSString::from_str(name);
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true);
    }
}

/// Called from the settings process: tells the running screenpen to reload.
pub fn notify_settings_changed() {
    post(SETTINGS_CHANGED);
}

fn on_second_instance() {
    with_app(|app| {
        let key = chord_text(app, Action::ToggleDrawing);
        let l = app.lang;
        notify(
            app,
            l.t(T::AlreadyRunning),
            &l.t(T::HowToDraw).replace("{key}", &key),
        );
    });
}

/// A short notice under the status item, in place of the Windows tray balloon:
/// system notifications would need a permission prompt.
fn notify(app: &mut App, title: &str, text: &str) {
    log(&format!("notice: {title}: {text}"));
    let Some(anchor) = app.tray.anchor(app.mtm) else {
        return;
    };
    let icon = Frame::from_cocoa(app.mtm, anchor);
    let scale = ui::scale_at(app.mtm, (icon.x, icon.y));
    let Some(pixmap) = render_tooltip(&format!("{title}. {text}"), scale, ChannelOrder::Rgba)
    else {
        return;
    };
    let w = pixmap.width() as f64 / scale as f64;
    let area = ui::work_area(app.mtm, (icon.x, icon.y));
    let x = icon.x.min(area.x + area.w - w - 8.0).max(area.x);
    app.notice.show(pixmap, scale, (x, icon.y + icon.h + 4.0));
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            6.0,
            &app.target,
            sel!(noticeDone:),
            None,
            false,
        );
    }
}

fn on_notice_done() {
    with_app(|app| app.notice.hide());
}

fn on_settings_changed() {
    with_app(|app| {
        log("settings changed by the settings window");
        load_settings(app);
        register_hotkeys(app);
        if app.active {
            refresh_all(app);
            app.palette.hide();
            show_palette(app);
            app.sheet.hide();
        }
        update_tray(app);
        if let Some(stem) = snapshots::take_restore_request() {
            restore_snapshot(app, &stem);
        }
    });
}

/// Puts a snapshot back on the screen it fits (same size), frozen, with its ink editable.
fn restore_snapshot(app: &mut App, stem: &str) {
    let (data, background) = match snapshots::load(stem) {
        Ok(loaded) => loaded,
        Err(e) => {
            log(&format!("snapshot {stem} not restored: {e}"));
            return;
        }
    };
    if !app.active {
        show(app);
    }
    let fits = |o: &Overlay| o.canvas.width() == data.width && o.canvas.height() == data.height;
    let index = app
        .overlays
        .iter()
        .position(fits)
        .unwrap_or_else(|| focused_overlay(app));
    let Some(o) = app.overlays.get_mut(index) else {
        return;
    };
    let r = o.canvas.restore(data.annotations, background);
    paint(o, r);
    app.frozen = app.overlays.iter().any(|o| o.canvas.backdrop().is_some());
    render_palette(app);
    log(&format!("snapshot {stem} restored on screen {index}"));
}

// MARK: overlays

fn create_overlays(mtm: MainThreadMarker, state: &ToolState) -> Vec<Overlay> {
    let mut out = Vec::new();
    for screen in NSScreen::screens(mtm).iter() {
        let rect = screen.frame();
        let scale = screen.backingScaleFactor();
        let w = (rect.size.width * scale).round() as u32;
        let h = (rect.size.height * scale).round() as u32;
        let (Some(pixels), Some(mut canvas)) = (
            Pixmap::new(w, h),
            Canvas::new(w, h, scale as f32, ChannelOrder::Rgba),
        ) else {
            log("out of memory for an overlay");
            continue;
        };
        canvas.set_auto_fade(state.auto_fade);
        let panel = view::panel(mtm, rect, 0);
        let view = PixelView::new(mtm, rect.size);
        view.set_pixels(pixels, scale);
        panel.setContentView(Some(&view));
        panel.makeFirstResponder(Some(&view));
        panel.setFrame_display(rect, false);
        let frame = Frame::from_cocoa(mtm, rect);
        log(&format!("overlay {w}x{h} px, {frame:?} pt, scale {scale}"));
        out.push(Overlay {
            panel,
            view,
            frame,
            canvas,
        });
    }
    out
}

fn on_screens_changed() {
    with_app(|app| {
        log("screens changed: rebuilding overlays (drawings are kept per screen frame)");
        let mut old = std::mem::take(&mut app.overlays);
        app.pressed = None;
        app.overlays = create_overlays(app.mtm, &app.state);
        for o in &mut app.overlays {
            // Keep the drawing of a screen that's still at the same place.
            if let Some(i) = old.iter().position(|p| p.frame == o.frame) {
                o.canvas.doc = old.remove(i).canvas.doc;
            }
        }
        for o in &old {
            o.panel.orderOut(None);
            o.panel.close();
        }
        if app.active {
            show(app);
        }
    });
}

// MARK: show / hide / ghost

fn on_hotkey(id: u32) {
    with_app(|app| {
        let Some(&(_, action)) = GLOBAL_ACTIONS.iter().find(|(i, _)| *i == id) else {
            return;
        };
        match action {
            Action::ToggleDrawing if app.active => hide(app),
            Action::ToggleDrawing => show(app),
            Action::ExitGhost if app.active && app.ghost => set_ghost(app, false),
            Action::Quit => quit(app),
            _ => {}
        }
    });
}

fn quit(app: &App) {
    log("quit");
    NSApplication::sharedApplication(app.mtm).terminate(None);
}

fn background(app: &App) -> Rgba {
    if app.ghost {
        Rgba::BLACK.with_alpha(0.0)
    } else if app.state.whiteboard {
        Rgba::WHITE
    } else {
        // Same as on Windows: invisible, and the panels take the mouse anyway.
        Rgba::BLACK.with_alpha(app.state.dim.max(1.0 / 255.0))
    }
}

/// Repaints every overlay after a background, ghost or document-wide change.
fn refresh_all(app: &mut App) {
    let bg = background(app);
    for o in &mut app.overlays {
        o.canvas.set_auto_fade(app.state.auto_fade);
        o.canvas.set_background(bg);
        o.canvas.rebuild_cache();
        paint(o, Repaint::Full);
    }
    render_palette(app);
    update_fade_timer(app);
}

fn paint(o: &mut Overlay, repaint: Repaint) {
    let canvas = &mut o.canvas;
    let Some(changed) = o.view.draw_into(|target| canvas.paint(target, repaint)) else {
        return;
    };
    match (repaint, changed) {
        (_, None) => {}
        (Repaint::Full, _) => o.view.invalidate(None),
        (_, Some(r)) => o.view.invalidate(Some(r)),
    }
}

fn show(app: &mut App) {
    load_settings(app);
    app.active = true;
    app.ghost = false;
    for o in &mut app.overlays {
        o.panel.setIgnoresMouseEvents(false);
        if !o.canvas.wake() {
            log("out of memory waking an overlay");
        }
    }
    refresh_all(app);
    for o in &app.overlays {
        o.panel.orderFrontRegardless();
    }
    show_palette(app);
    focus_overlay_under_cursor(app);
    update_tray(app);
    log("show");
}

fn hide(app: &mut App) {
    app.lens_on = false;
    if let Some(lens) = &mut app.lens {
        lens.hide();
    }
    app.active = false;
    app.pressed = None;
    if app.frozen {
        toggle_freeze(app);
    }
    for o in &mut app.overlays {
        o.canvas.set_spotlight(false);
    }
    for o in &mut app.overlays {
        // Ordering out the key panel hands the keyboard back to the active app.
        o.panel.orderOut(None);
        // Hidden overlays keep only their vector annotations.
        o.canvas.sleep();
    }
    app.palette.hide();
    app.sheet.hide();
    update_fade_timer(app);
    update_tray(app);
    log("hide");
}

fn set_ghost(app: &mut App, ghost: bool) {
    app.ghost = ghost;
    app.pressed = None;
    for o in &mut app.overlays {
        o.canvas.cancel();
        o.panel.setIgnoresMouseEvents(ghost);
    }
    refresh_all(app);
    if ghost {
        // Give the keyboard back too: a panel stops being key when it's ordered out.
        for o in &app.overlays {
            if o.panel.isKeyWindow() {
                o.panel.orderOut(None);
                o.panel.orderFrontRegardless();
            }
        }
        // The palette stays clickable so ghost mode can be left from it.
        app.palette.panel.orderFrontRegardless();
    } else {
        focus_overlay_under_cursor(app);
    }
    log(&format!("ghost {ghost}"));
    update_tray(app);
}

fn mode(app: &App) -> Mode {
    match (app.active, app.ghost) {
        (false, _) => Mode::Idle,
        (true, false) => Mode::Drawing,
        (true, true) => Mode::Ghost,
    }
}

fn update_tray(app: &mut App) {
    let mode = mode(app);
    app.tray.set_mode(app.mtm, mode);
}

fn set_capture_hiding(app: &mut App, on: bool) {
    app.hide_palette_from_capture = on;
    ui::set_capture_hidden(&app.palette.panel, on);
    ui::set_capture_hidden(&app.palette.tooltip.panel, on);
}

fn overlay_at(app: &App, p: (f64, f64)) -> Option<usize> {
    app.overlays.iter().position(|o| o.frame.contains(p))
}

/// Makes the panel under the pointer key without activating the app, so shortcuts
/// reach its canvas while the presenter's app stays in front.
fn focus_overlay_under_cursor(app: &App) {
    let index = overlay_at(app, mouse(app.mtm)).unwrap_or(0);
    if let Some(o) = app.overlays.get(index) {
        o.panel.makeKeyWindow();
    }
}

/// One timer drives auto-fade (slow) and the laser fade-out (smooth).
fn update_fade_timer(app: &mut App) {
    let animating = app.overlays.iter().any(|o| o.canvas.animating()) || app.lens_on;
    let wanted = match (app.active, animating, app.state.auto_fade) {
        (false, _, _) => None,
        (true, true, _) => Some(0.016),
        (true, false, Some(_)) => Some(0.05),
        (true, false, None) => None,
    };
    if app.fade_timer.as_ref().map(|(_, i)| *i) == wanted {
        return;
    }
    if let Some((timer, _)) = app.fade_timer.take() {
        timer.invalidate();
    }
    app.fade_timer = wanted.map(|interval| {
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                interval,
                &app.target,
                sel!(tick:),
                None,
                true,
            )
        };
        (timer, interval)
    });
}

fn on_tick() {
    with_app(|app| {
        update_magnifier(app);
        for o in &mut app.overlays {
            let animating = o.canvas.animating();
            let (repaint, _) = o.canvas.tick();
            // Overlays (the laser) repaint themselves in `paint`.
            if repaint != Repaint::None || animating {
                paint(o, repaint);
            }
        }
        update_fade_timer(app);
    });
}

fn toggle_magnifier(app: &mut App) {
    app.lens_on = !app.lens_on;
    if app.lens_on {
        update_magnifier(app);
    } else if let Some(lens) = &mut app.lens {
        lens.hide();
    }
    update_fade_timer(app);
    render_palette(app);
}

/// Captures the screen around the pointer below our overlay and shows it enlarged in
/// a round lens; called from the animation timer while the lens is on.
fn update_magnifier(app: &mut App) {
    if !app.lens_on {
        return;
    }
    let at = mouse(app.mtm);
    let Some(o) = app.overlays.iter().find(|o| o.frame.contains(at)) else {
        return;
    };
    let scale = ui::scale_at(app.mtm, at);
    let half = (LENS_RADIUS / app.lens_zoom) as f64;
    let source = Frame {
        x: at.0 - half,
        y: at.1 - half,
        w: half * 2.0,
        h: half * 2.0,
    };
    let px = ((half * 2.0) as f32 * scale).max(1.0) as u32;
    let image = match capture::screen(source, o.panel.windowNumber(), (px, px)) {
        Ok(image) => image,
        Err(e) => {
            // Usually the Screen Recording permission; turn off rather than retry 60 times a second.
            log(&format!("magnifier: {e}"));
            app.lens_on = false;
            let l = app.lang;
            notify(app, l.t(T::MagnifierUnavailable), &e);
            render_palette(app);
            return;
        }
    };
    let diameter = (LENS_RADIUS * 2.0 * scale) as u32;
    let Some(lens_image) = render_lens(&image, diameter) else {
        return;
    };
    let mtm = app.mtm;
    let lens = app.lens.get_or_insert_with(|| ui::Popup::new(mtm, true));
    let r = LENS_RADIUS as f64;
    lens.show(lens_image, scale, (at.0 - r, at.1 - r));
}

fn toggle_freeze(app: &mut App) {
    app.frozen = !app.frozen;
    for o in &mut app.overlays {
        let image = if app.frozen {
            let size = (o.canvas.width(), o.canvas.height());
            capture::screen(o.frame, o.panel.windowNumber(), size)
                .map_err(|e| log(&format!("freeze: no capture: {e}")))
                .ok()
        } else {
            None
        };
        let r = o.canvas.set_backdrop(image);
        paint(o, r);
    }
    log(&format!("freeze: {}", app.frozen));
    render_palette(app);
}

/// Saves the screen under the pointer with its ink to Pictures and the pasteboard.
fn snapshot(app: &mut App, index: usize) {
    let Some(o) = app.overlays.get(index) else {
        return;
    };
    let screen = if o.canvas.backdrop().is_some() {
        Ok(None)
    } else {
        let size = (o.canvas.width(), o.canvas.height());
        capture::screen(o.frame, o.panel.windowNumber(), size).map(Some)
    };
    let result = screen.and_then(|screen| {
        // The clean screen goes into the snapshot too, so it can be restored and exported.
        let background = o.canvas.backdrop().cloned().or_else(|| screen.clone());
        let annotations: Vec<_> = o
            .canvas
            .doc
            .annotations()
            .iter()
            .map(|a| (**a).clone())
            .collect();
        let image = o.canvas.compose(screen).ok_or("out of memory")?;
        capture::copy_to_pasteboard(&image);
        snapshots::save(
            &capture::snapshot_stem(),
            &image,
            background.as_ref(),
            ChannelOrder::Rgba,
            &annotations,
            o.canvas.scale(),
        )
        .map_err(|e| e.to_string())
    });
    let l = app.lang;
    match result {
        Ok(path) => {
            log(&format!("snapshot saved to {}", path.display()));
            notify(app, l.t(T::SnapshotSaved), &path.display().to_string());
        }
        Err(e) => {
            log(&format!("snapshot failed: {e}"));
            notify(app, l.t(T::SnapshotFailed), &e);
        }
    }
}

// MARK: palette and cheat sheet

fn look(app: &App) -> Look {
    Look {
        tool: app.state.tool,
        bold: app.state.text_style.bold,
        italic: app.state.text_style.italic,
        magnifier: app.lens_on,
        spotlight: app
            .overlays
            .first()
            .is_some_and(|o| o.canvas.spotlight_on()),
        frozen: app.frozen,
        color_index: app.state.color_index,
        colors: app.state.colors,
        ghost: app.ghost,
        whiteboard: app.state.whiteboard,
        width: app.state.width,
        text_size: app.state.text_size,
        theme: Theme::from_setting(&app.state.theme),
        badges: app.state.key_badges.then(|| app.state.shortcuts.clone()),
    }
}

fn render_palette(app: &mut App) {
    let look = look(app);
    app.palette.render(&look);
}

fn show_palette(app: &mut App) {
    if !app.palette_placed || overlay_at(app, app.palette.center()).is_none() {
        let primary = app
            .overlays
            .iter()
            .find(|o| o.frame.x == 0.0 && o.frame.y == 0.0)
            .or(app.overlays.first())
            .map_or((0.0, 0.0), |o| (o.frame.x + 1.0, o.frame.y + 1.0));
        app.palette.origin = app.palette.default_origin(primary);
    }
    let look = look(app);
    app.palette.show(&look);
}

fn on_palette_input(app: &mut App, input: Input, event: &NSEvent) {
    let screen = mouse(app.mtm);
    let local = app.palette.to_local(screen);
    match input {
        Input::LeftDown => match app.palette.palette.hit(local) {
            Some(Hit::Grip) => {
                app.palette.tooltip.hide();
                if event.clickCount() >= 2 {
                    app.palette.palette.vertical = !app.palette.palette.vertical;
                    let (origin, look) = (app.palette.origin, look(app));
                    app.palette.move_to(origin, &look);
                    app.palette_placed = true;
                    save_settings(app);
                } else {
                    let o = app.palette.origin;
                    app.palette.drag = Some((screen.0 - o.0, screen.1 - o.1));
                }
            }
            Some(Hit::Item(i)) => {
                let look = look(app);
                match app.palette.palette.press(i, &look) {
                    Press::Run(action) => palette_action(app, action),
                    Press::Settings => open_settings(app),
                    Press::None => {}
                }
                render_palette(app);
            }
            None => {}
        },
        Input::Drag => {
            if let Some(offset) = app.palette.drag {
                let look = look(app);
                app.palette
                    .move_to((screen.0 - offset.0, screen.1 - offset.1), &look);
            }
        }
        Input::Move => {
            let hit = app.palette.palette.hit(local);
            let hover = match hit {
                Some(Hit::Item(i)) => Some(i),
                _ => None,
            };
            let grip = hit == Some(Hit::Grip);
            if hover != app.palette.palette.hover || grip != app.palette.grip_hover {
                app.palette.palette.hover = hover;
                app.palette.grip_hover = grip;
                render_palette(app);
                show_tooltip(app);
            }
        }
        Input::Up if app.palette.drag.take().is_some() => {
            app.palette_placed = true;
            save_settings(app);
        }
        Input::Exit if app.palette.drag.is_none() => clear_palette_hover(app),
        _ => {}
    }
}

fn clear_palette_hover(app: &mut App) {
    app.palette.grip_hover = false;
    if app.palette.palette.hover.take().is_some() {
        render_palette(app);
    }
    app.palette.tooltip.hide();
}

/// The hovered item's name and shortcut, next to the palette.
fn show_tooltip(app: &mut App) {
    let (cells, grip, _) = app.palette.palette.layout();
    let (text, cell) = match app.palette.palette.hover {
        Some(i) => (
            app.palette
                .palette
                .item(i)
                .tooltip(app.lang, &app.state.shortcuts),
            cells[i],
        ),
        None if app.palette.grip_hover => (app.lang.t(T::Grip).to_string(), grip),
        None => {
            app.palette.tooltip.hide();
            return;
        }
    };
    let scale = app.palette.scale;
    let Some(pixmap) = render_tooltip(&text, scale, ChannelOrder::Rgba) else {
        return;
    };
    let o = app.palette.origin;
    let (pw, ph) = app.palette.size();
    let s = app.palette.backing();
    let (tw, th) = (pixmap.width() as f64 / s, pixmap.height() as f64 / s);
    let gap = 6.0;
    let area = ui::work_area(app.mtm, app.palette.center());
    let u = app.palette.ui_scale as f64;
    let (cx, cy, cw, ch) = (
        cell.x as f64 * u,
        cell.y as f64 * u,
        cell.width as f64 * u,
        cell.height as f64 * u,
    );
    let at = if app.palette.palette.vertical {
        let y = o.1 + cy + ch / 2.0 - th / 2.0;
        let x = if o.0 + pw + gap + tw <= area.x + area.w {
            o.0 + pw + gap
        } else {
            o.0 - gap - tw
        };
        (x, y)
    } else {
        let x = (o.0 + cx + cw / 2.0 - tw / 2.0).clamp(area.x, (area.x + area.w - tw).max(area.x));
        let y = if o.1 + ph + gap + th <= area.y + area.h {
            o.1 + ph + gap
        } else {
            o.1 - gap - th
        };
        (x, y)
    };
    app.palette.tooltip.show(pixmap, s as f32, at);
}

fn palette_action(app: &mut App, action: Action) {
    let target = focused_overlay(app);
    match action {
        Action::Ghost => set_ghost(app, !app.ghost),
        Action::Help => toggle_sheet(app, target),
        Action::ToggleDrawing => hide(app),
        Action::Clear => {
            for o in 0..app.overlays.len() {
                run_action(app, o, Action::Clear);
            }
        }
        action => run_action(app, target, action),
    }
}

/// Shows or hides the shortcut cheat sheet, centered on the given screen.
fn toggle_sheet(app: &mut App, index: usize) {
    if app.sheet.visible {
        app.sheet.hide();
        return;
    }
    let Some(o) = app.overlays.get(index) else {
        return;
    };
    let (frame, scale) = (o.frame, o.canvas.scale() * app.state.ui_scale);
    let Some(pixmap) =
        render_cheat_sheet(app.lang, &app.state.shortcuts, scale, ChannelOrder::Rgba)
    else {
        return;
    };
    let s = scale as f64;
    let (w, h) = (pixmap.width() as f64 / s, pixmap.height() as f64 / s);
    let at = (
        frame.x + ((frame.w - w) / 2.0).round(),
        frame.y + ((frame.h - h) / 2.0).round(),
    );
    app.sheet.show(pixmap, scale, at);
}

/// The overlay that has keyboard focus, or the one under the pointer.
fn focused_overlay(app: &App) -> usize {
    app.overlays
        .iter()
        .position(|o| o.panel.isKeyWindow())
        .or_else(|| overlay_at(app, mouse(app.mtm)))
        .unwrap_or(0)
}

// MARK: actions and keys

fn run_action(app: &mut App, index: usize, action: Action) {
    let Some(o) = app.overlays.get_mut(index) else {
        return;
    };
    let (repaint, request) = o.canvas.perform(&mut app.state, action);
    paint(o, repaint);
    match request {
        None => {}
        Some(Request::StopDrawing) => hide(app),
        Some(Request::Ghost) => set_ghost(app, true),
        Some(Request::ToggleWhiteboard) => {
            app.state.whiteboard = !app.state.whiteboard;
            refresh_all(app);
        }
        Some(Request::Help) => toggle_sheet(app, index),
        Some(Request::ToggleSpotlight) => {
            let on = !app.overlays.iter().any(|o| o.canvas.spotlight_on());
            for o in &mut app.overlays {
                let r = o.canvas.set_spotlight(on);
                paint(o, r);
            }
            render_palette(app);
        }
        Some(Request::ToggleFreeze) => toggle_freeze(app),
        Some(Request::ToggleMagnifier) => toggle_magnifier(app),
        Some(Request::Snapshot) => snapshot(app, index),
        Some(Request::StateChanged) => {
            // Tool, color or width changed: other screens drop their gestures.
            for (i, o) in app.overlays.iter_mut().enumerate() {
                if i != index {
                    let r = o.canvas.cancel();
                    paint(o, r);
                }
            }
            render_palette(app);
            save_settings(app);
        }
    }
}

fn on_key(app: &mut App, index: usize, event: &NSEvent) {
    let chord = keys::chord(event);
    let escape = chord.is_some_and(|c| c.key == Key::Escape);
    // Esc closes the cheat sheet first.
    if app.sheet.visible && escape {
        app.sheet.hide();
        return;
    }
    if app.overlays[index].canvas.is_typing() {
        let o = &mut app.overlays[index];
        let edit = chord.map(|c| keys::edit_chord(event, c));
        if let Some(r) = edit.and_then(|c| crate::clipboard::text_key(&mut o.canvas, c)) {
            paint(o, r);
            return;
        }
        if let Some(action) = chord.and_then(|c| typing_action(&app.state.shortcuts, c)) {
            run_action(app, index, action);
            return;
        }
        // While typing, keys are text; only Esc (finish) is a shortcut.
        if escape {
            run_action(app, index, Action::Escape);
            return;
        }
        let text = event
            .characters()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let o = &mut app.overlays[index];
        for c in text.chars() {
            let r = o.canvas.type_char(c);
            paint(o, r);
        }
        return;
    }
    if let Some(action) = chord.and_then(|c| app.state.shortcuts.local_action(c)) {
        run_action(app, index, action);
    }
}

// MARK: status item

fn on_tray_command(app: &mut App, command: Command) {
    match command {
        Command::ToggleDrawing if app.active => hide(app),
        Command::ToggleDrawing => show(app),
        Command::ToggleGhost if app.active => set_ghost(app, !app.ghost),
        Command::ToggleGhost => {}
        Command::ToggleWhiteboard => {
            app.state.whiteboard = !app.state.whiteboard;
            if app.active {
                refresh_all(app);
            }
        }
        Command::ToggleCaptureHiding => set_capture_hiding(app, !app.hide_palette_from_capture),
        Command::SetDim(i) => {
            app.state.dim = DIM_LEVELS.get(i).map_or(0.0, |(level, _)| *level);
            if app.active {
                refresh_all(app);
            }
        }
        Command::SetFade(i) => {
            app.state.auto_fade = i.and_then(|i| FADE_SECONDS.get(i).copied());
            if app.active {
                refresh_all(app);
            }
        }
        Command::Settings => open_settings(app),
        Command::ToggleAutostart => {
            autostart::set(!autostart::enabled());
        }
        Command::Quit => quit(app),
    }
    // Every menu command changes something worth keeping, except toggling drawing.
    if !matches!(
        command,
        Command::ToggleDrawing | Command::ToggleGhost | Command::Quit
    ) {
        save_settings(app);
    }
}

fn menu_state(app: &App) -> MenuState {
    MenuState {
        autostart: autostart::enabled(),
        lang: app.lang,
        version: instance::title(),
        mode: mode(app),
        whiteboard: app.state.whiteboard,
        hide_from_capture: app.hide_palette_from_capture,
        dim: DIM_LEVELS
            .iter()
            .position(|(l, _)| *l == app.state.dim)
            .unwrap_or(0),
        fade: app
            .state
            .auto_fade
            .and_then(|s| FADE_SECONDS.iter().position(|f| *f == s)),
        toggle_key: chord_text(app, Action::ToggleDrawing),
        quit_key: chord_text(app, Action::Quit),
    }
}

fn on_menu_open(menu: &NSMenu) {
    with_app(|app| tray::fill_menu(app.mtm, menu, &app.target, &menu_state(app)));
}

fn on_menu_item(tag: isize) {
    if let Some(command) = Command::from_tag(tag) {
        with_app(|app| on_tray_command(app, command));
    }
}

// MARK: input

fn on_input(view: &PixelView, event: &NSEvent, input: Input) {
    with_app(|app| {
        if std::ptr::eq(view, &*app.palette.view) {
            on_palette_input(app, input, event);
            return;
        }
        if std::ptr::eq(view, &*app.sheet.view) {
            if matches!(input, Input::LeftDown | Input::RightDown) {
                app.sheet.hide();
            }
            return;
        }
        let Some(index) = app
            .overlays
            .iter()
            .position(|o| std::ptr::eq(view, &*o.view))
        else {
            return;
        };
        match input {
            Input::Key => on_key(app, index, event),
            Input::Wheel => {
                let dy = event.scrollingDeltaY();
                let ctrl = keys::mods(event).ctrl;
                if !ctrl && dy != 0.0 && app.lens_on {
                    let (min, max, step) = LENS_ZOOM;
                    let next = app.lens_zoom + if dy > 0.0 { step } else { -step };
                    app.lens_zoom = next.clamp(min, max);
                    update_magnifier(app);
                } else if !ctrl && dy != 0.0 && app.overlays[index].canvas.spotlight_on() {
                    let o = &mut app.overlays[index];
                    o.canvas.resize_spotlight(dy > 0.0);
                    paint(o, Repaint::Full);
                } else if ctrl && dy != 0.0 {
                    let action = if dy > 0.0 {
                        Action::Thicker
                    } else {
                        Action::Thinner
                    };
                    run_action(app, index, action);
                }
            }
            Input::Exit => {}
            Input::Proximity => track_pen(app, event),
            _ => {
                if event.subtype() == NSEventSubtype::TabletProximity {
                    track_pen(app, event);
                }
                on_pointer(app, index, input, event);
            }
        }
    });
}

fn on_pointer(app: &mut App, index: usize, input: Input, event: &NSEvent) {
    if app.ghost {
        return;
    }
    if input == Input::Move {
        if app.palette.palette.hover.is_some() || app.palette.grip_hover {
            clear_palette_hover(app);
        }
        set_cursor(app, index);
    }
    let o = &app.overlays[index];
    let at = o.view.convertPoint_fromView(event.locationInWindow(), None);
    let scale = o.canvas.scale() as f64;
    // Only tablet points carry pressure; a mouse draws at full width.
    let tablet = event.subtype() == NSEventSubtype::TabletPoint;
    let pointer = Pointer {
        x: (at.x * scale) as f32,
        y: ((o.frame.h - at.y) * scale) as f32,
        pressure: if tablet { event.pressure() } else { 1.0 },
        mods: keys::mods(event),
        double_click: input == Input::LeftDown && event.clickCount() >= 2,
    };
    let repaint = match input {
        Input::LeftDown | Input::RightDown if app.pressed.is_none() => {
            // The pen's eraser end erases like the right mouse button.
            let button = if input == Input::RightDown || (tablet && app.eraser) {
                Button::Right
            } else {
                Button::Left
            };
            app.pressed = Some(index);
            // Keys go to the screen being drawn on.
            if !o.panel.isKeyWindow() {
                o.panel.makeKeyWindow();
            }
            app.overlays[index]
                .canvas
                .pointer_down(&app.state, pointer, button)
        }
        Input::Drag if app.pressed == Some(index) => {
            app.overlays[index].canvas.pointer_move(&app.state, pointer)
        }
        Input::Move if app.pressed.is_none() => {
            app.overlays[index].canvas.hover(&app.state, pointer)
        }
        Input::Up if app.pressed == Some(index) => {
            app.pressed = None;
            app.overlays[index].canvas.pointer_up(&app.state, pointer)
        }
        _ => return,
    };
    paint(&mut app.overlays[index], repaint);
    if input == Input::Up {
        update_fade_timer(app);
    }
}

/// Remembers which end of the pen is near the tablet.
fn track_pen(app: &mut App, event: &NSEvent) {
    app.eraser =
        event.isEnteringProximity() && event.pointingDeviceType() == NSPointingDeviceType::Eraser;
}

/// The cursor over a drawing screen: the brush for drawing tools, else a system one.
fn set_cursor(app: &mut App, index: usize) {
    let tool = app.state.tool;
    let scale = app.overlays[index].canvas.scale();
    let cursor = app
        .brush_cursor
        .get(tool, &app.state.style(tool), scale)
        .unwrap_or_else(|| match tool {
            ToolKind::Select => NSCursor::arrowCursor(),
            ToolKind::Text => NSCursor::IBeamCursor(),
            _ => NSCursor::crosshairCursor(),
        });
    cursor.set();
}

// MARK: log

fn log(line: &str) {
    let path = std::env::temp_dir().join("screenpen.log");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{ts} {line}");
    }
}
