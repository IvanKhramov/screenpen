//! Linux backend: an overlay window per monitor drawing a `Canvas`, the palette,
//! cheat sheet, tray icon, global shortcuts and ghost mode. The display server side
//! is behind `display::Display`: native Wayland where the compositor has
//! layer-shell (KDE, Sway, Hyprland), X11 otherwise (including XWayland on GNOME).
//! Everything else runs on one thread fed by `Msg`. Diagnostics go to stderr and
//! $XDG_RUNTIME_DIR/screenpen.log.

mod autostart;
mod capture;
mod display;
mod instance;
mod keys;
mod tray;
mod ui;
mod wayland;
mod x11;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use display::{Area, Cursor, Display, Event, Monitor, PointerKind, Win};
use screenpen_core::render::ChannelOrder;
use screenpen_core::{Action, Chord, Key, Rgba, ToolKind};
use tiny_skia::Pixmap;
use tray::{Command, MenuState, Mode, Tray};
use ui::{PaletteWindow, Popup};

use crate::canvas::{Button, Canvas, Mods, Pointer, Repaint, Request, typing_action};
use crate::config::{PaletteSettings, Settings};
use crate::i18n::{Lang, T};
use crate::palette::{Hit, Item, Look, Palette, render_cheat_sheet, render_tooltip};
use crate::state::{DIM_LEVELS, FADE_SECONDS, ToolState};

const GLOBAL_ACTIONS: [Action; 3] = [Action::ToggleDrawing, Action::ExitGhost, Action::Quit];
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const SLIDE_KEYS: [Key; 6] = [
    Key::PageUp,
    Key::PageDown,
    Key::Left,
    Key::Right,
    Key::Up,
    Key::Down,
];

/// Everything the main loop reacts to, from any thread.
pub enum Msg {
    X11(x11rb::protocol::Event),
    /// Wayland events were read and wait to be dispatched.
    Wayland,
    Tray(Command),
    Command(instance::Command),
    /// The display connection is gone.
    Disconnected,
}

struct Overlay {
    monitor: Monitor,
    pixmap: Pixmap,
    canvas: Canvas,
}

struct App {
    display: Box<dyn Display>,
    overlays: Vec<Overlay>,
    state: ToolState,
    palette: PaletteWindow,
    /// Whether the palette has a position (from settings or a drag) to save.
    palette_placed: bool,
    sheet: Popup,
    settings_path: PathBuf,
    /// "auto", "en" or "ru", as in the settings file.
    language: String,
    lang: Lang,
    hide_palette_from_capture: bool,
    /// The settings file didn't parse: keep the current state and don't overwrite it.
    settings_broken: bool,
    active: bool,
    ghost: bool,
    /// The screens show a captured image under the ink.
    frozen: bool,
    /// Overlay index and button of the gesture in progress.
    pressed: Option<(usize, Button)>,
    last_click: Option<(Instant, (i32, i32))>,
    /// The overlay that gets the keys.
    focus: usize,
    tray: Tray,
    settings_window: Option<std::process::Child>,
    /// When the fade and laser animation ticks next.
    next_tick: Option<Instant>,
    quit: bool,
}

pub fn run() {
    start(false);
}

/// `screenpen --toggle`: starts or stops drawing in the running copy, or starts
/// screenpen drawing. Desktop shortcuts call it where global hotkeys can't work.
pub fn toggle() {
    if instance::send(instance::Command::Toggle).is_none() {
        start(true);
    }
}

/// Called from the settings process: tells the running screenpen to reload.
pub fn notify_settings_changed() {
    instance::send(instance::Command::Reload);
}

pub fn system_lang() -> Lang {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default();
    if lang.starts_with("ru") {
        Lang::Ru
    } else {
        Lang::En
    }
}

fn open_display(tx: &Sender<Msg>) -> Option<Box<dyn Display>> {
    let forced_x11 = std::env::var("SCREENPEN_BACKEND").is_ok_and(|b| b == "x11");
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && !forced_x11 {
        match wayland::Wayland::connect(tx.clone()) {
            Ok(w) => return Some(Box::new(w)),
            Err(e) => log(&format!(
                "native Wayland overlay unavailable ({e}); using X11 through XWayland, \
                 so windows of Wayland apps may stay above the drawing"
            )),
        }
    }
    match x11::X11::connect(tx.clone()) {
        Ok(x) => Some(Box::new(x)),
        Err(e) => {
            log(&format!("no display: {e}"));
            None
        }
    }
}

fn start(draw: bool) {
    let (listener, replaced) = match instance::claim() {
        instance::Claim::Exit => return,
        instance::Claim::Continue { listener, replaced } => (listener, replaced),
    };
    log(&format!("start {}", instance::VERSION));
    let (tx, rx) = mpsc::channel();
    let Some(display) = open_display(&tx) else {
        instance::release();
        std::process::exit(1);
    };
    if let Some(listener) = listener {
        instance::listen(listener, tx.clone());
    }
    let smoke = std::env::var_os("SCREENPEN_SMOKE").map(PathBuf::from);
    let mut app = App::new(display, tx.clone());
    let first_run = !app.settings_path.exists();
    load_settings(&mut app);
    if first_run {
        // Write the defaults so there's a file to look at and edit.
        save_settings(&app);
    }
    autostart::refresh();
    let taken = register_hotkeys(&mut app);
    let l = app.lang;
    let key = if app.display.has_hotkeys() {
        chord_text(&app, Action::ToggleDrawing)
    } else {
        "screenpen --toggle".to_string()
    };
    if let Some(old) = &replaced {
        tray::notify(
            &instance::title(),
            &format!("{old} → {}", instance::VERSION),
        );
    } else if taken.is_empty() {
        tray::notify(l.t(T::Running), &l.t(T::HowToDraw).replace("{key}", &key));
    } else {
        let text = l.t(T::HotkeyTakenText).replace("{keys}", &taken.join(", "));
        tray::notify(l.t(T::HotkeyTaken), &text);
    }
    if draw {
        show(&mut app);
    }
    if let Some(dir) = smoke {
        x11::smoke(tx, dir);
    }
    app.display.flush();

    loop {
        let msg = match app.next_tick {
            Some(at) => {
                let wait = at.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        if let Some(msg) = msg {
            on_msg(&mut app, msg);
        }
        if app.next_tick.is_some_and(|at| at <= Instant::now()) {
            tick(&mut app);
        }
        app.display.flush();
        if app.quit {
            break;
        }
    }
    log("quit");
    instance::release();
}

impl App {
    fn new(mut display: Box<dyn Display>, tx: Sender<Msg>) -> App {
        let state = ToolState::default();
        let overlays = create_overlays(display.as_mut(), &state);
        let lang = system_lang();
        let menu = MenuState {
            autostart: autostart::enabled(),
            lang,
            version: instance::title(),
            mode: Mode::Idle,
            whiteboard: false,
            dim: 0,
            fade: None,
            toggle_key: String::new(),
            ghost_key: String::new(),
            quit_key: String::new(),
        };
        App {
            display,
            overlays,
            state,
            palette: PaletteWindow::new(Palette {
                items: Item::defaults(),
                vertical: false,
                hover: None,
            }),
            palette_placed: false,
            sheet: Popup::new(Win::Sheet),
            settings_path: crate::config::default_path(),
            language: "auto".to_string(),
            lang,
            hide_palette_from_capture: false,
            settings_broken: false,
            active: false,
            ghost: false,
            frozen: false,
            pressed: None,
            last_click: None,
            focus: 0,
            tray: Tray::new(tx, menu),
            settings_window: None,
            next_tick: None,
            quit: false,
        }
    }

    fn monitors(&self) -> Vec<Monitor> {
        self.overlays.iter().map(|o| o.monitor).collect()
    }
}

fn on_msg(app: &mut App, msg: Msg) {
    match msg {
        Msg::X11(_) | Msg::Wayland => {
            for event in app.display.handle(msg) {
                on_event(app, event);
            }
        }
        Msg::Tray(command) => on_tray_command(app, command),
        Msg::Command(instance::Command::Hello) => {
            let key = chord_text(app, Action::ToggleDrawing);
            let l = app.lang;
            tray::notify(
                l.t(T::AlreadyRunning),
                &l.t(T::HowToDraw).replace("{key}", &key),
            );
        }
        Msg::Command(instance::Command::Toggle) => on_hotkey(app, Action::ToggleDrawing),
        Msg::Command(instance::Command::Reload) => {
            log("settings changed by the settings window");
            reload_settings(app);
        }
        Msg::Command(instance::Command::Quit) => {
            log("quit requested by another copy");
            app.quit = true;
        }
        Msg::Disconnected => app.quit = true,
    }
}

fn on_event(app: &mut App, event: Event) {
    match event {
        Event::Pointer {
            win,
            kind,
            at,
            mods,
        } => match win {
            Win::Palette => on_palette_pointer(app, kind, at),
            Win::Sheet if matches!(kind, PointerKind::Down(_)) => {
                app.sheet.hide(app.display.as_mut())
            }
            Win::Overlay(index) if index < app.overlays.len() => {
                if app.palette.palette.hover.is_some() || app.palette.grip_hover {
                    clear_palette_hover(app);
                }
                match kind {
                    PointerKind::Wheel(up) => on_wheel(app, index, up, mods),
                    _ => on_pointer(app, index, kind, at, mods),
                }
            }
            _ => {}
        },
        Event::Key {
            win,
            key,
            mods,
            text,
        } => on_key(app, win, key, mods, text),
        Event::MonitorsChanged => rebuild_overlays(app),
    }
}

/// Binds the global shortcuts; returns the ones another app already holds.
fn register_hotkeys(app: &mut App) -> Vec<String> {
    let chords: Vec<(Action, Chord)> = GLOBAL_ACTIONS
        .iter()
        .filter_map(|&a| app.state.shortcuts.chord(a).map(|c| (a, c)))
        .collect();
    app.display.grab_hotkeys(&chords)
}

fn chord_text(app: &App, action: Action) -> String {
    app.state
        .shortcuts
        .chord(action)
        .map_or_else(|| app.lang.t(T::NoShortcut).to_string(), |c| c.to_string())
}

// MARK: settings

/// Reads the settings file into the app. Called at start and whenever drawing starts,
/// so edits made while screenpen runs are picked up.
fn load_settings(app: &mut App) {
    let (settings, error) = Settings::load(&app.settings_path);
    if let Some(e) = error {
        // Keep what we have and stop saving until the file is fixed, or the next
        // save would replace the user's file with defaults.
        log(&format!("settings not loaded, keeping current ones: {e}"));
        if !app.settings_broken {
            tray::notify(app.lang.t(T::SettingsBroken), &e);
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
        app.palette.origin = (x, y);
        app.palette_placed = true;
    }
    // X11 and Wayland have no way to keep a window out of screen sharing; the
    // setting is kept for the other platforms.
    app.hide_palette_from_capture = settings.hide_palette_from_capture;
    if app.state.shortcuts != before {
        register_hotkeys(app);
    }
    update_tray(app);
}

fn save_settings(app: &App) {
    if app.settings_broken {
        log("settings file has errors; not saving over it");
        return;
    }
    let p = &app.palette;
    let palette = PaletteSettings {
        items: p.palette.items.iter().map(|i| i.name()).collect(),
        vertical: p.palette.vertical,
        x: app.palette_placed.then_some(p.origin.0),
        y: app.palette_placed.then_some(p.origin.1),
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

fn open_settings(app: &mut App) {
    // The settings window would open under the overlay and be unreachable.
    if app.active {
        hide(app);
    }
    save_settings(app);
    if let Some(child) = &mut app.settings_window
        && matches!(child.try_wait(), Ok(None))
    {
        log("the settings window is already open");
        return;
    }
    let spawned = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn());
    match spawned {
        Ok(child) => app.settings_window = Some(child),
        Err(e) => log(&format!("settings window not started: {e}")),
    }
}

/// Applies a settings file changed by the settings window.
fn reload_settings(app: &mut App) {
    load_settings(app);
    register_hotkeys(app);
    if app.active {
        refresh_all(app);
        app.palette.hide(app.display.as_mut());
        show_palette(app);
        app.sheet.hide(app.display.as_mut());
    }
    update_tray(app);
}

// MARK: overlays

fn create_overlays(display: &mut dyn Display, state: &ToolState) -> Vec<Overlay> {
    let monitors = display.monitors();
    let mut out = Vec::new();
    for m in monitors {
        let (w, h) = (m.area.w as u32, m.area.h as u32);
        let (Some(pixmap), Some(mut canvas)) = (
            Pixmap::new(w, h),
            Canvas::new(w, h, m.scale, ChannelOrder::Bgra),
        ) else {
            log("out of memory for an overlay");
            continue;
        };
        canvas.set_auto_fade(state.auto_fade);
        log(&format!(
            "overlay monitor {w}x{h} at ({}, {}) scale {}",
            m.area.x, m.area.y, m.scale
        ));
        out.push(Overlay {
            monitor: m,
            pixmap,
            canvas,
        });
    }
    let monitors: Vec<Monitor> = out.iter().map(|o| o.monitor).collect();
    display.set_overlays(&monitors);
    out
}

fn rebuild_overlays(app: &mut App) {
    log("monitors changed: rebuilding overlays (drawings are kept per monitor position)");
    let mut old = std::mem::take(&mut app.overlays);
    app.overlays = create_overlays(app.display.as_mut(), &app.state);
    for o in &mut app.overlays {
        // Keep the drawing of a monitor that's still at the same place.
        if let Some(i) = old.iter().position(|p| p.monitor.area == o.monitor.area) {
            o.canvas.doc = old.remove(i).canvas.doc;
        }
    }
    app.focus = 0;
    if app.active {
        show(app);
    }
}

fn paint(display: &mut dyn Display, index: usize, o: &mut Overlay, repaint: Repaint) {
    let changed = o.canvas.paint(&mut o.pixmap.as_mut(), repaint);
    let dirty = match (repaint, changed) {
        (_, None) => return,
        (Repaint::Full, _) => None,
        (_, Some(r)) => Some(r),
    };
    let at = (o.monitor.area.x, o.monitor.area.y);
    display.present(Win::Overlay(index), at, o.pixmap.as_ref(), dirty);
}

fn background(app: &App) -> Rgba {
    if app.ghost {
        Rgba::BLACK.with_alpha(0.0)
    } else if app.state.whiteboard {
        Rgba::WHITE
    } else {
        Rgba::BLACK.with_alpha(app.state.dim)
    }
}

/// Repaints every overlay after a background, ghost or document-wide change.
fn refresh_all(app: &mut App) {
    let bg = background(app);
    for (i, o) in app.overlays.iter_mut().enumerate() {
        o.canvas.set_auto_fade(app.state.auto_fade);
        o.canvas.set_background(bg);
        o.canvas.rebuild_cache();
        paint(app.display.as_mut(), i, o, Repaint::Full);
    }
    update_cursor(app);
    render_palette(app);
    update_tick(app);
}

fn update_cursor(app: &mut App) {
    let cursor = match app.state.tool {
        ToolKind::Select => Cursor::Arrow,
        ToolKind::Text => Cursor::Text,
        _ => Cursor::Cross,
    };
    app.display.set_cursor(cursor);
}

// MARK: show / hide / ghost

fn on_hotkey(app: &mut App, action: Action) {
    match action {
        Action::ToggleDrawing if app.active => hide(app),
        Action::ToggleDrawing => show(app),
        Action::ExitGhost if app.active && app.ghost => set_ghost(app, false),
        Action::Quit => app.quit = true,
        _ => {}
    }
}

fn show(app: &mut App) {
    load_settings(app);
    app.active = true;
    app.ghost = false;
    app.display.set_click_through(false);
    for o in &mut app.overlays {
        if !o.canvas.wake() {
            log("out of memory waking an overlay");
        }
    }
    refresh_all(app);
    for i in 0..app.overlays.len() {
        app.display.show(Win::Overlay(i));
    }
    show_palette(app);
    focus_overlay_under_cursor(app);
    update_tray(app);
    log("show");
}

fn hide(app: &mut App) {
    app.active = false;
    app.pressed = None;
    if app.frozen {
        toggle_freeze(app);
    }
    for (i, o) in app.overlays.iter_mut().enumerate() {
        o.canvas.set_spotlight(false);
        app.display.hide(Win::Overlay(i));
        // Hidden overlays keep only their vector annotations.
        o.canvas.sleep();
    }
    app.palette.hide(app.display.as_mut());
    app.sheet.hide(app.display.as_mut());
    app.display.set_keyboard(false, app.focus);
    update_tick(app);
    update_tray(app);
    log("hide");
}

fn set_ghost(app: &mut App, ghost: bool) {
    app.ghost = ghost;
    app.pressed = None;
    for o in &mut app.overlays {
        o.canvas.cancel();
    }
    app.display.set_click_through(ghost);
    refresh_all(app);
    if ghost {
        app.display.set_keyboard(false, app.focus);
    } else {
        focus_overlay_under_cursor(app);
    }
    log(&format!("ghost: {ghost}"));
    update_tray(app);
}

fn mode(app: &App) -> Mode {
    match (app.active, app.ghost) {
        (false, _) => Mode::Idle,
        (true, false) => Mode::Drawing,
        (true, true) => Mode::Ghost,
    }
}

fn focus_overlay_under_cursor(app: &mut App) {
    let cursor = app.display.pointer_position();
    app.focus = cursor
        .and_then(|p| app.overlays.iter().position(|o| o.monitor.area.contains(p)))
        .unwrap_or(0);
    app.display.set_keyboard(true, app.focus);
}

/// One timer drives auto-fade (slow) and the laser fade-out (smooth).
fn update_tick(app: &mut App) {
    let animating = app.overlays.iter().any(|o| o.canvas.animating());
    let interval = if app.active && animating {
        Some(16)
    } else if app.active && app.state.auto_fade.is_some() {
        Some(50)
    } else {
        None
    };
    app.next_tick = interval.map(|ms| {
        let next = Instant::now() + Duration::from_millis(ms);
        app.next_tick.map_or(next, |t| t.min(next))
    });
}

fn tick(app: &mut App) {
    app.next_tick = None;
    for (i, o) in app.overlays.iter_mut().enumerate() {
        let animating = o.canvas.animating();
        let (repaint, _) = o.canvas.tick();
        // Overlays (the laser) repaint themselves in `paint`.
        if repaint != Repaint::None || animating {
            paint(app.display.as_mut(), i, o, repaint);
        }
    }
    update_tick(app);
}

fn toggle_freeze(app: &mut App) {
    app.frozen = !app.frozen;
    let images = if app.frozen {
        let areas: Vec<Area> = app.overlays.iter().map(|o| o.monitor.area).collect();
        app.display.capture(&areas)
    } else {
        app.overlays.iter().map(|_| None).collect()
    };
    for (i, (o, image)) in app.overlays.iter_mut().zip(images).enumerate() {
        let r = o.canvas.set_backdrop(image);
        paint(app.display.as_mut(), i, o, r);
    }
    log(&format!("freeze: {}", app.frozen));
    render_palette(app);
}

/// Saves the screen under the pointer with its ink to Pictures and the clipboard.
fn snapshot(app: &mut App, index: usize) {
    let Some(o) = app.overlays.get(index) else {
        return;
    };
    let screen = if o.canvas.backdrop().is_some() {
        None
    } else {
        let area = o.monitor.area;
        app.display.capture(&[area]).pop().flatten()
    };
    let result = app.overlays[index]
        .canvas
        .compose(screen)
        .ok_or_else(|| "out of memory".to_string())
        .and_then(|image| {
            let rgba = capture::to_rgba(&image);
            let copied = rgba.encode_png().is_ok_and(|png| app.display.copy_png(png));
            capture::save_png(&rgba)
                .map(|path| (path, copied))
                .map_err(|e| e.to_string())
        });
    let l = app.lang;
    match result {
        Ok((path, copied)) => {
            log(&format!(
                "snapshot saved to {} (clipboard: {copied})",
                path.display()
            ));
            tray::notify(l.t(T::SnapshotSaved), &path.display().to_string());
        }
        Err(e) => {
            log(&format!("snapshot failed: {e}"));
            tray::notify(l.t(T::SnapshotFailed), &e);
        }
    }
}

// MARK: palette and cheat sheet

fn look(app: &App) -> Look {
    Look {
        tool: app.state.tool,
        bold: app.state.text_style.bold,
        italic: app.state.text_style.italic,
        spotlight: app
            .overlays
            .first()
            .is_some_and(|o| o.canvas.spotlight_on()),
        frozen: app.frozen,
        color_index: app.state.color_index,
        colors: app.state.colors,
        ghost: app.ghost,
        whiteboard: app.state.whiteboard,
    }
}

fn render_palette(app: &mut App) {
    let look = look(app);
    app.palette.render(app.display.as_mut(), &look);
}

fn show_palette(app: &mut App) {
    let monitors = app.monitors();
    if !app.palette_placed
        || !monitors
            .iter()
            .any(|m| m.area.contains(app.palette.center()))
    {
        let primary = monitors
            .first()
            .map_or((0, 0), |m| (m.area.x + 1, m.area.y + 1));
        app.palette.origin = app.palette.default_origin(&monitors, primary);
    }
    let look = look(app);
    app.palette.show(app.display.as_mut(), &monitors, &look);
}

fn on_palette_pointer(app: &mut App, kind: PointerKind, screen: (i32, i32)) {
    let local = app.palette.to_local(screen);
    match kind {
        PointerKind::Down(1) => match app.palette.palette.hit(local) {
            Some(Hit::Grip) => {
                app.palette.tooltip.hide(app.display.as_mut());
                let double = app
                    .palette
                    .last_grip_click
                    .is_some_and(|t| t.elapsed() <= DOUBLE_CLICK);
                if double {
                    app.palette.last_grip_click = None;
                    app.palette.palette.vertical = !app.palette.palette.vertical;
                    let (origin, look, monitors) = (app.palette.origin, look(app), app.monitors());
                    app.palette
                        .move_to(app.display.as_mut(), &monitors, origin, &look);
                    app.palette_placed = true;
                    save_settings(app);
                } else {
                    app.palette.last_grip_click = Some(Instant::now());
                    let o = app.palette.origin;
                    app.palette.drag = Some((screen.0 - o.0, screen.1 - o.1));
                }
            }
            Some(Hit::Item(i)) => {
                let item = app.palette.palette.items[i];
                palette_item(app, item);
            }
            None => {}
        },
        PointerKind::Move => {
            if let Some(offset) = app.palette.drag {
                let (look, monitors) = (look(app), app.monitors());
                let to = (screen.0 - offset.0, screen.1 - offset.1);
                app.palette
                    .move_to(app.display.as_mut(), &monitors, to, &look);
                return;
            }
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
        PointerKind::Up(1) if app.palette.drag.take().is_some() => {
            app.palette_placed = true;
            save_settings(app);
        }
        PointerKind::Leave if app.palette.drag.is_none() => clear_palette_hover(app),
        PointerKind::Lost => {
            app.palette.drag = None;
            clear_palette_hover(app);
        }
        _ => {}
    }
}

fn clear_palette_hover(app: &mut App) {
    app.palette.grip_hover = false;
    if app.palette.palette.hover.take().is_some() {
        render_palette(app);
    }
    app.palette.tooltip.hide(app.display.as_mut());
}

/// The hovered item's name and shortcut, next to the palette.
fn show_tooltip(app: &mut App) {
    let (cells, grip, _) = app.palette.palette.layout();
    let (text, cell) = match app.palette.palette.hover {
        Some(i) => (
            app.palette.palette.items[i].tooltip(app.lang, &app.state.shortcuts),
            cells[i],
        ),
        None if app.palette.grip_hover => (app.lang.t(T::Grip).to_string(), grip),
        None => {
            app.palette.tooltip.hide(app.display.as_mut());
            return;
        }
    };
    let scale = app.palette.scale;
    let Some(pixmap) = render_tooltip(&text, scale, ChannelOrder::Bgra) else {
        return;
    };
    let o = app.palette.origin;
    let (pw, ph) = app.palette.size();
    let (tw, th) = (pixmap.width() as i32, pixmap.height() as i32);
    let gap = (6.0 * scale) as i32;
    let area = ui::work_area(&app.monitors(), app.palette.center());
    let at = if app.palette.palette.vertical {
        let y = o.1 + ((cell.y + cell.height / 2.0) * scale) as i32 - th / 2;
        let x = if o.0 + pw + gap + tw <= area.right() {
            o.0 + pw + gap
        } else {
            o.0 - gap - tw
        };
        (x, y)
    } else {
        let x = (o.0 + ((cell.x + cell.width / 2.0) * scale) as i32 - tw / 2)
            .clamp(area.x, (area.right() - tw).max(area.x));
        let y = if o.1 + ph + gap + th <= area.bottom() {
            o.1 + ph + gap
        } else {
            o.1 - gap - th
        };
        (x, y)
    };
    app.palette.tooltip.show(app.display.as_mut(), &pixmap, at);
}

fn palette_item(app: &mut App, item: Item) {
    let target = app.focus.min(app.overlays.len().saturating_sub(1));
    match item {
        Item::Settings => open_settings(app),
        Item::Action(Action::Ghost) => set_ghost(app, !app.ghost),
        Item::Action(Action::Help) => toggle_sheet(app, target),
        Item::Action(Action::Clear) => {
            for o in 0..app.overlays.len() {
                run_action(app, o, Action::Clear);
            }
        }
        Item::Action(action) => run_action(app, target, action),
    }
}

/// Shows or hides the shortcut cheat sheet, centered on the given screen.
fn toggle_sheet(app: &mut App, index: usize) {
    if app.sheet.visible {
        app.sheet.hide(app.display.as_mut());
        return;
    }
    let Some(o) = app.overlays.get(index) else {
        return;
    };
    let (area, scale) = (o.monitor.area, o.canvas.scale() * app.state.ui_scale);
    let Some(pixmap) =
        render_cheat_sheet(app.lang, &app.state.shortcuts, scale, ChannelOrder::Bgra)
    else {
        return;
    };
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let at = (area.x + (area.w - w) / 2, area.y + (area.h - h) / 2);
    app.sheet.show(app.display.as_mut(), &pixmap, at);
}

// MARK: actions and keys

fn run_action(app: &mut App, index: usize, action: Action) {
    let Some(o) = app.overlays.get_mut(index) else {
        return;
    };
    let (repaint, request) = o.canvas.perform(&mut app.state, action);
    paint(app.display.as_mut(), index, o, repaint);
    match request {
        None => {}
        Some(Request::StopDrawing) => hide(app),
        Some(Request::Ghost) => set_ghost(app, true),
        Some(Request::ToggleWhiteboard) => {
            app.state.whiteboard = !app.state.whiteboard;
            refresh_all(app);
            update_tray(app);
        }
        Some(Request::Help) => toggle_sheet(app, index),
        Some(Request::ToggleSpotlight) => {
            let on = !app.overlays.iter().any(|o| o.canvas.spotlight_on());
            for (i, o) in app.overlays.iter_mut().enumerate() {
                let r = o.canvas.set_spotlight(on);
                paint(app.display.as_mut(), i, o, r);
            }
            render_palette(app);
        }
        Some(Request::ToggleFreeze) => toggle_freeze(app),
        Some(Request::Snapshot) => snapshot(app, index),
        Some(Request::StateChanged) => {
            // Tool, color or width changed: other screens drop their gestures.
            for (i, o) in app.overlays.iter_mut().enumerate() {
                if i != index {
                    let r = o.canvas.cancel();
                    paint(app.display.as_mut(), i, o, r);
                }
            }
            update_cursor(app);
            render_palette(app);
            save_settings(app);
        }
    }
    update_tick(app);
}

fn on_key(app: &mut App, win: Option<Win>, key: Option<Key>, mods: Mods, text: Option<char>) {
    let chord = key.map(|key| Chord {
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
        key,
    });
    // While drawing the overlay holds the keyboard, so global shortcuts arrive here too.
    if let Some(c) = chord
        && let Some(&action) = GLOBAL_ACTIONS
            .iter()
            .find(|&&a| app.state.shortcuts.chord(a) == Some(c))
    {
        on_hotkey(app, action);
        return;
    }
    let Some(Win::Overlay(index)) = win else {
        return;
    };
    if !app.active || app.ghost || index >= app.overlays.len() {
        return;
    }
    // Esc closes the cheat sheet first.
    if app.sheet.visible && key == Some(Key::Escape) {
        app.sheet.hide(app.display.as_mut());
        return;
    }
    if app.overlays[index].canvas.is_typing() {
        if let Some(action) = chord.and_then(|c| typing_action(&app.state.shortcuts, c)) {
            return run_action(app, index, action);
        }
        // While typing, letters are text; only Esc (finish) is a shortcut.
        let typed = match key {
            Some(Key::Escape) => return run_action(app, index, Action::Escape),
            Some(Key::Enter) => Some('\r'),
            Some(Key::Backspace) => Some('\u{8}'),
            _ => text,
        };
        if let Some(c) = typed {
            let o = &mut app.overlays[index];
            let r = o.canvas.type_char(c);
            paint(app.display.as_mut(), index, o, r);
        }
        return;
    }
    let Some(chord) = chord else { return };
    if let Some(action) = app.state.shortcuts.local_action(chord) {
        run_action(app, index, action);
    } else if !chord.ctrl && !chord.alt && SLIDE_KEYS.contains(&chord.key) {
        app.display.forward_key(chord.key);
    }
}

fn on_wheel(app: &mut App, index: usize, up: bool, mods: Mods) {
    if app.ghost {
        return;
    }
    if !mods.ctrl && app.overlays[index].canvas.spotlight_on() {
        let o = &mut app.overlays[index];
        o.canvas.resize_spotlight(up);
        paint(app.display.as_mut(), index, o, Repaint::Full);
    } else if mods.ctrl {
        let action = if up { Action::Thicker } else { Action::Thinner };
        run_action(app, index, action);
    }
}

fn on_pointer(app: &mut App, index: usize, kind: PointerKind, at: (i32, i32), mods: Mods) {
    if app.ghost || !app.active {
        return;
    }
    let area = app.overlays[index].monitor.area;
    let mut pointer = Pointer {
        x: (at.0 - area.x) as f32,
        y: (at.1 - area.y) as f32,
        pressure: 1.0,
        mods,
        double_click: false,
    };
    let repaint = match kind {
        PointerKind::Down(b) if app.pressed.is_none() => {
            let button = match b {
                1 => Button::Left,
                3 => Button::Right,
                _ => return,
            };
            pointer.double_click = button == Button::Left && is_double_click(app, at);
            app.pressed = Some((index, button));
            // Keys go to the screen being drawn on.
            if app.focus != index {
                app.focus = index;
                app.display.set_keyboard(true, index);
            }
            app.overlays[index]
                .canvas
                .pointer_down(&app.state, pointer, button)
        }
        PointerKind::Move => match app.pressed {
            Some((i, _)) if i == index => {
                app.overlays[index].canvas.pointer_move(&app.state, pointer)
            }
            Some(_) => return,
            None => app.overlays[index].canvas.hover(&app.state, pointer),
        },
        // Only the button that started the gesture ends it.
        PointerKind::Up(b) => match app.pressed {
            Some((i, button)) if i == index && button_number(button) == b => {
                app.pressed = None;
                app.overlays[index].canvas.pointer_up(&app.state, pointer)
            }
            _ => return,
        },
        // Something else took the pointer mid-gesture: drop it instead of drawing on.
        PointerKind::Lost => match app.pressed {
            Some((i, _)) if i == index => {
                app.pressed = None;
                log("pointer grab lost: gesture cancelled");
                app.overlays[index].canvas.cancel()
            }
            _ => return,
        },
        _ => return,
    };
    paint(
        app.display.as_mut(),
        index,
        &mut app.overlays[index],
        repaint,
    );
    if matches!(kind, PointerKind::Up(_)) {
        update_tick(app);
    }
}

fn button_number(button: Button) -> u8 {
    match button {
        Button::Left => 1,
        Button::Right => 3,
    }
}

fn is_double_click(app: &mut App, p: (i32, i32)) -> bool {
    let now = Instant::now();
    let double = app.last_click.is_some_and(|(t, q)| {
        now.duration_since(t) <= DOUBLE_CLICK && (q.0 - p.0).abs() <= 4 && (q.1 - p.1).abs() <= 4
    });
    app.last_click = if double { None } else { Some((now, p)) };
    double
}

// MARK: tray

fn on_tray_command(app: &mut App, command: Command) {
    match command {
        Command::ToggleDrawing => on_hotkey(app, Action::ToggleDrawing),
        Command::ToggleGhost if app.active => set_ghost(app, !app.ghost),
        Command::ToggleGhost => {}
        Command::ToggleWhiteboard => {
            app.state.whiteboard = !app.state.whiteboard;
            if app.active {
                refresh_all(app);
            }
        }
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
        Command::Quit => app.quit = true,
    }
    // Every tray command changes something worth keeping, except toggling drawing.
    if !matches!(
        command,
        Command::ToggleDrawing | Command::ToggleGhost | Command::Quit
    ) {
        save_settings(app);
    }
    update_tray(app);
}

fn update_tray(app: &mut App) {
    let shown = |action| {
        if app.display.has_hotkeys() {
            chord_text(app, action)
        } else {
            String::new()
        }
    };
    let menu = MenuState {
        autostart: autostart::enabled(),
        lang: app.lang,
        version: instance::title(),
        mode: mode(app),
        whiteboard: app.state.whiteboard,
        dim: DIM_LEVELS
            .iter()
            .position(|(l, _)| *l == app.state.dim)
            .unwrap_or(0),
        fade: app
            .state
            .auto_fade
            .and_then(|s| FADE_SECONDS.iter().position(|f| *f == s)),
        toggle_key: shown(Action::ToggleDrawing),
        ghost_key: shown(Action::ExitGhost),
        quit_key: shown(Action::Quit),
    };
    app.tray.update(menu);
}

// MARK: log

fn log(line: &str) {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    eprintln!("screenpen: {line}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("screenpen.log"))
    {
        let _ = writeln!(f, "{ts} {line}");
    }
}
