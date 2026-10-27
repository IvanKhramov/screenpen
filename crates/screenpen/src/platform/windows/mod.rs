//! Windows backend: a layered overlay window per monitor drawing a `Canvas`, input
//! through WM_POINTER, global hotkeys, ghost (click-through) mode, slide-key
//! forwarding to the previous foreground window, the palette and the tray icon.
//! Diagnostics go to %TEMP%\screenpen.log.

mod autostart;
mod capture;
mod cursor;
mod instance;
mod keys;
mod magnifier;
mod surface;
mod tray;
mod ui;

use std::cell::RefCell;
use std::io::Write as _;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use screenpen_core::render::ChannelOrder;
use screenpen_core::{Action, Rgba, ToolKind};
use surface::Surface;
use tray::{
    Command, MenuState, Mode, Tray, WM_QUIT_REQUEST, WM_RELOAD_SETTINGS, WM_SECOND_INSTANCE,
    WM_TRAY,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, MAPVK_VK_TO_VSC, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    MapVirtualKeyW, RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, VK_DOWN, VK_LEFT,
    VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows_sys::Win32::UI::Input::Pointer::{
    EnableMouseInPointer, GetPointerPenInfo, GetPointerType, POINTER_PEN_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW,
    GWL_EXSTYLE, GWL_STYLE, GWLP_HWNDPARENT, GetClassNameW, GetCursorPos, GetForegroundWindow,
    GetMessageW, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_IBEAM,
    KillTimer, LoadCursorW, MA_NOACTIVATE, MSG, PEN_FLAG_ERASER, PEN_FLAG_INVERTED,
    PEN_MASK_PRESSURE, PT_PEN, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetCursor,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, WM_CAPTURECHANGED, WM_CHAR, WM_CONTEXTMENU, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_POINTERACTIVATE,
    WM_POINTERDOWN, WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE, WM_POINTERWHEEL, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::canvas::{Button, Canvas, Mods, Pointer, Repaint, Request, typing_action};
use crate::config::{PaletteSettings, Settings};
use crate::i18n::{Lang, T};
use crate::palette::{Hit, Item, Look, Palette, Press, Theme, render_cheat_sheet, render_tooltip};
use crate::snapshots;
use crate::state::{DIM_LEVELS, FADE_SECONDS, ToolState, step_ui_scale};

const PA_NOACTIVATE: LRESULT = 3; // WM_POINTERACTIVATE result
const POINTER_FLAG_FIRSTBUTTON: usize = 0x0010; // HIWORD(wParam) of WM_POINTER*
const POINTER_FLAG_SECONDBUTTON: usize = 0x0020;
const POINTER_FLAG_CANCELED: usize = 0x8000;
const WM_POINTERCAPTURECHANGED: u32 = 0x024C;
const FADE_TIMER: usize = 1;
const GLOBAL_ACTIONS: [(i32, Action); 3] = [
    (1, Action::ToggleDrawing),
    (2, Action::ExitGhost),
    (3, Action::Quit),
];

struct Overlay {
    hwnd: HWND,
    monitor: RECT,
    /// The window's pixels; only while drawing is on (see `Canvas::sleep`).
    surface: Option<Surface>,
    canvas: Canvas,
    /// Cover the monitor's last pixel row too (only above a full-screen app).
    full_height: bool,
}

struct App {
    overlays: Vec<Overlay>,
    brush_cursor: cursor::BrushCursor,
    state: ToolState,
    palette: ui::PaletteWindow,
    /// Whether the palette has a position (from settings or a drag) to save.
    palette_placed: bool,
    sheet: ui::Popup,
    settings_path: PathBuf,
    /// "auto", "en" or "ru", as in the settings file.
    language: String,
    lang: Lang,
    active: bool,
    ghost: bool,
    /// The screens show a captured image under the ink.
    frozen: bool,
    /// None when the system has no Magnification API.
    magnifier: Option<magnifier::Magnifier>,
    /// Overlay index, button and pointer id of the gesture in progress.
    pressed: Option<(usize, Button, u32)>,
    last_click: Option<(Instant, POINT)>,
    previous: HWND,
    hide_palette_from_capture: bool,
    /// The settings file failed to parse: don't overwrite it.
    settings_broken: bool,
    rebuild: bool,
    tray: Tray,
    tray_hwnd: HWND,
    /// Broadcast when Explorer restarts; the tray icon has to be added again.
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn run() {
    let replaced = match instance::claim() {
        instance::Claim::Exit => return,
        instance::Claim::Continue { replaced } => replaced,
    };
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        EnableMouseInPointer(1);
    }
    log(&format!("start {}", instance::VERSION));
    register_classes();
    let tray_hwnd = create_window(TRAY_CLASS, WS_EX_TOOLWINDOW, 0, 0);
    unsafe { SetWindowTextW(tray_hwnd, wide(&instance::title()).as_ptr()) };
    let state = ToolState::default();
    let palette = Palette::new(Item::defaults(), false);
    let sheet = ui::Popup::new(SHEET_CLASS, false);
    // The cheat sheet is for the presenter only.
    ui::set_capture_hidden(sheet.hwnd, true);
    let app = App {
        overlays: create_overlays(&state),
        state,
        palette: ui::PaletteWindow::new(PALETTE_CLASS, TOOLTIP_CLASS, palette),
        palette_placed: false,
        sheet,
        settings_path: crate::config::default_path(),
        language: "auto".to_string(),
        lang: system_lang(),
        active: false,
        ghost: false,
        frozen: false,
        magnifier: None,
        pressed: None,
        last_click: None,
        previous: null_mut(),
        hide_palette_from_capture: false,
        settings_broken: false,
        rebuild: false,
        tray: Tray::new(tray_hwnd),
        tray_hwnd,
        taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
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
            app.tray.notify(&instance::title(), &text);
        } else if taken.is_empty() {
            app.tray
                .notify(l.t(T::Running), &l.t(T::HowToDraw).replace("{key}", &key));
        } else {
            let text = l.t(T::HotkeyTakenText).replace("{keys}", &taken.join(", "));
            app.tray.notify(l.t(T::HotkeyTaken), &text);
        }
    });

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        if msg.message == WM_HOTKEY && msg.hwnd.is_null() {
            with_app(|app| on_hotkey(app, msg.wParam as i32));
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        with_app(|app| {
            if app.rebuild {
                app.rebuild = false;
                rebuild_overlays(app);
            }
        });
    }
    log("quit");
    // Drop the app now: thread-locals of the main thread aren't dropped at exit, and
    // the tray icon would linger until the mouse passes over it.
    APP.with(|cell| cell.borrow_mut().take());
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        // A nested message (sent while we're inside a handler) finds the app borrowed
        // and falls through to DefWindowProc.
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(app) = guard.as_mut()
        {
            f(app);
        }
    });
}

/// Registers the global shortcuts; returns the ones another app already holds.
fn register_hotkeys(app: &App) -> Vec<String> {
    let mut taken = Vec::new();
    for (id, action) in GLOBAL_ACTIONS {
        unsafe { UnregisterHotKey(null_mut(), id) };
        let Some(chord) = app.state.shortcuts.chord(action) else {
            continue;
        };
        let Some(vk) = keys::vk_from_key(chord.key) else {
            continue;
        };
        let mut mods = MOD_NOREPEAT;
        if chord.ctrl {
            mods |= MOD_CONTROL;
        }
        if chord.alt {
            mods |= MOD_ALT;
        }
        if chord.shift {
            mods |= MOD_SHIFT;
        }
        let ok = unsafe { RegisterHotKey(null_mut(), id, mods, vk as u32) } != 0;
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

pub fn quit_running() -> bool {
    instance::quit_running()
}

pub fn system_lang() -> Lang {
    // Primary language id 0x19 is Russian.
    if unsafe { GetUserDefaultUILanguage() } & 0x3ff == 0x19 {
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
            app.tray.notify(l.t(T::SettingsBroken), &e);
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
    app.state.lang = app.lang;
    let items = settings.palette_items();
    app.palette.palette.items = if items.is_empty() {
        Item::defaults()
    } else {
        items
    };
    app.palette.palette.vertical = settings.palette.vertical;
    app.palette.ui_scale = app.state.ui_scale;
    if let (Some(x), Some(y)) = (settings.palette.x, settings.palette.y) {
        app.palette.origin = POINT { x, y };
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
    app.tray.set_text(app.lang, keys);
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
        x: app.palette_placed.then_some(p.origin.x),
        y: app.palette_placed.then_some(p.origin.y),
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

/// Title of the settings window, so a second "Settings…" brings it forward.
pub const SETTINGS_TITLE: &str = "screenpen settings";

fn open_settings(app: &mut App) {
    // A normal window would open under the topmost overlay and be unreachable.
    if app.active {
        hide(app);
    }
    save_settings(app);
    let existing = unsafe { FindWindowW(null(), wide(SETTINGS_TITLE).as_ptr()) };
    if !existing.is_null() {
        unsafe { SetForegroundWindow(existing) };
        return;
    }
    let spawned = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn());
    if let Err(e) = spawned {
        log(&format!("settings window not started: {e}"));
    }
}

/// Called from the settings process: tells the running screenpen to reload.
pub fn notify_settings_changed() {
    let running = unsafe { FindWindowW(wide(TRAY_CLASS).as_ptr(), null()) };
    if !running.is_null() {
        unsafe { PostMessageW(running, WM_RELOAD_SETTINGS, 0, 0) };
    }
}

/// Applies a settings file changed by the settings window.
fn reload_settings(app: &mut App) {
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
    let backdrop = background.map(|bg| snapshots::to_rgba(&bg, ChannelOrder::Bgra));
    let Some(o) = app.overlays.get_mut(index) else {
        return;
    };
    let r = o.canvas.restore(data.annotations, backdrop);
    paint(o, r);
    app.frozen = app.overlays.iter().any(|o| o.canvas.backdrop().is_some());
    render_palette(app);
    log(&format!("snapshot {stem} restored on screen {index}"));
}

// MARK: windows

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const OVERLAY_CLASS: &str = "screenpen.overlay";
const PALETTE_CLASS: &str = "screenpen.palette";
const TRAY_CLASS: &str = "screenpen.tray";
const TOOLTIP_CLASS: &str = "screenpen.tooltip";
const SHEET_CLASS: &str = "screenpen.sheet";
const LENS_CLASS: &str = "screenpen.lens";

fn register_classes() {
    unsafe {
        let instance = GetModuleHandleW(null());
        for name in [
            OVERLAY_CLASS,
            PALETTE_CLASS,
            TRAY_CLASS,
            TOOLTIP_CLASS,
            SHEET_CLASS,
        ] {
            let name = wide(name);
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassExW(&class);
        }
    }
}

fn create_window(class: &str, ex_style: u32, width: i32, height: i32) -> HWND {
    let class = wide(class);
    unsafe {
        CreateWindowExW(
            ex_style,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            width,
            height,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        )
    }
}

fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
        unsafe { (*(data as *mut Vec<HMONITOR>)).push(m) };
        1
    }
    let mut list: Vec<HMONITOR> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            &mut list as *mut _ as LPARAM,
        )
    };
    list
}

fn monitor_info(m: HMONITOR) -> (RECT, f32) {
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(m, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (info.rcMonitor, dx as f32 / 96.0)
    }
}

fn create_overlays(state: &ToolState) -> Vec<Overlay> {
    let mut out = Vec::new();
    for m in monitors() {
        let (r, scale) = monitor_info(m);
        let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        let hwnd = create_window(
            OVERLAY_CLASS,
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w as i32,
            h as i32 - 1,
        );
        let Some(mut canvas) = Canvas::new(w, h, scale, ChannelOrder::Bgra) else {
            log("out of memory for an overlay");
            continue;
        };
        // Drawing starts off: no full-screen buffers until it's turned on.
        canvas.sleep();
        canvas.set_auto_fade(state.auto_fade);
        log(&format!(
            "overlay {hwnd:?} monitor {w}x{h} at ({}, {}) scale {scale}",
            r.left, r.top
        ));
        out.push(Overlay {
            hwnd,
            monitor: r,
            surface: None,
            canvas,
            full_height: false,
        });
    }
    out
}

fn rebuild_overlays(app: &mut App) {
    log("display change: rebuilding overlays (drawings are kept per monitor position)");
    // The palette is owned by an overlay, and destroying a window destroys the
    // windows it owns: detach it first.
    unsafe { SetWindowLongPtrW(app.palette.hwnd, GWLP_HWNDPARENT, 0) };
    app.pressed = None;
    // The frozen images belong to the old monitor layout.
    app.frozen = false;
    let mut old = std::mem::take(&mut app.overlays);
    app.overlays = create_overlays(&app.state);
    for o in &mut app.overlays {
        // Keep the drawing of a monitor that's still at the same place.
        if let Some(i) = old.iter().position(|p| same_rect(p.monitor, o.monitor)) {
            let previous = old.remove(i);
            o.canvas.doc = previous.canvas.doc;
            unsafe { DestroyWindow(previous.hwnd) };
        }
    }
    for o in old {
        unsafe { DestroyWindow(o.hwnd) };
    }
    if app.active {
        // Not `show`: that would forget the window behind us and leave ghost mode.
        reshow(app);
    }
}

// MARK: show / hide / ghost

fn on_hotkey(app: &mut App, id: i32) {
    let Some(&(_, action)) = GLOBAL_ACTIONS.iter().find(|(i, _)| *i == id) else {
        return;
    };
    match action {
        Action::ToggleDrawing if app.active => hide(app),
        Action::ToggleDrawing => show(app),
        Action::ExitGhost if app.active && app.ghost => set_ghost(app, false),
        Action::Quit => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

fn background(app: &App) -> Rgba {
    if app.ghost {
        Rgba::BLACK.with_alpha(0.0)
    } else if app.state.whiteboard {
        Rgba::WHITE
    } else {
        // Alpha 0 pixels of a layered window let clicks through; 1/255 is invisible
        // but keeps the whole window hit-testable.
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

fn paint(o: &mut Overlay, mut repaint: Repaint) {
    if o.surface.is_none() {
        o.surface = Surface::new(o.canvas.width(), o.canvas.height());
        // A new surface is empty: send all of it.
        repaint = Repaint::Full;
    }
    let Some(surface) = &mut o.surface else {
        log("out of memory for an overlay surface");
        return;
    };
    let mut target = surface.pixmap();
    let changed = o.canvas.paint(&mut target, repaint);
    // A full repaint also resizes the window (the 1px-short trick), so it sends everything.
    let dirty = match (repaint, changed) {
        (_, None) => return,
        (Repaint::Full, _) => None,
        (_, Some(r)) => Some(RECT {
            left: r.left(),
            top: r.top(),
            right: r.right(),
            bottom: r.bottom(),
        }),
    };
    let h = surface.height - if o.full_height { 0 } else { 1 };
    if !surface.present(o.hwnd, o.monitor.left, o.monitor.top, h, dirty) {
        log("UpdateLayeredWindowIndirect failed");
    }
}

fn show(app: &mut App) {
    let front = unsafe { GetForegroundWindow() };
    // Started from the tray or with our own window in front: keep the last real app,
    // so slide keys and the focus on hide go back to it.
    if !is_ours_or_shell(app, front) {
        app.previous = front;
    }
    let fullscreen = covered_monitor(app.previous);
    log(&format!(
        "show: previous {:?} class {:?}, covers a monitor: {}",
        app.previous,
        class_name(app.previous),
        fullscreen.is_some()
    ));
    load_settings(app);
    app.active = true;
    app.ghost = false;
    for o in &mut app.overlays {
        o.full_height = fullscreen.is_some_and(|r| same_rect(r, o.monitor));
    }
    reshow(app);
    focus_overlay_under_cursor(app);
    update_tray(app);
}

/// Shows every overlay and the palette in the current mode (drawing or ghost).
fn reshow(app: &mut App) {
    for o in &mut app.overlays {
        set_click_through(o.hwnd, app.ghost);
        // The screen behind may have changed while hidden.
        o.canvas.forget_screen();
        if !o.canvas.wake() {
            log("out of memory waking an overlay");
        }
    }
    refresh_all(app);
    for o in &app.overlays {
        unsafe {
            ShowWindow(o.hwnd, SW_SHOWNA);
            SetWindowPos(
                o.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    show_palette(app);
}

/// Our windows and the taskbar/tray: not something to hand the focus back to.
fn is_ours_or_shell(app: &App, hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return true;
    }
    let ours = app.overlays.iter().any(|o| o.hwnd == hwnd)
        || [
            app.palette.hwnd,
            app.palette.tooltip.hwnd,
            app.sheet.hwnd,
            app.tray_hwnd,
        ]
        .contains(&hwnd);
    let shell = [
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "NotifyIconOverflowWindow",
        "TopLevelWindowForOverflowXamlIsland",
    ];
    ours || shell.contains(&class_name(hwnd).as_str())
}

fn hide(app: &mut App) {
    if let Some(lens) = &mut app.magnifier {
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
        unsafe { ShowWindow(o.hwnd, SW_HIDE) };
        // Hidden overlays keep only their vector annotations.
        o.canvas.sleep();
        o.surface = None;
    }
    app.palette.hide();
    app.sheet.hide();
    update_fade_timer(app);
    let ok = unsafe { SetForegroundWindow(app.previous) } != 0;
    log(&format!("hide: focus back to {:?}: {ok}", app.previous));
    update_tray(app);
}

fn set_ghost(app: &mut App, ghost: bool) {
    if ghost && app.frozen {
        toggle_freeze(app);
    }
    app.ghost = ghost;
    app.pressed = None;
    for o in &mut app.overlays {
        o.canvas.cancel();
        // Back from mouse mode the screen behind is likely different.
        o.canvas.forget_screen();
        set_click_through(o.hwnd, ghost);
    }
    refresh_all(app);
    if ghost {
        let ok = unsafe { SetForegroundWindow(app.previous) } != 0;
        log(&format!("ghost on: focus back to {:?}: {ok}", app.previous));
    } else {
        // While in ghost mode the user may have switched apps: slide keys go there now.
        let front = unsafe { GetForegroundWindow() };
        if !is_ours_or_shell(app, front) {
            app.previous = front;
        }
        focus_overlay_under_cursor(app);
        log("ghost off");
    }
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
    app.tray.set_mode(mode);
}

fn set_capture_hiding(app: &mut App, on: bool) {
    app.hide_palette_from_capture = on;
    let ok = ui::set_capture_hidden(app.palette.hwnd, on)
        && ui::set_capture_hidden(app.palette.tooltip.hwnd, on);
    log(&format!("palette hidden from capture: {on} ({ok})"));
}

fn set_click_through(hwnd: HWND, on: bool) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let transparent = WS_EX_TRANSPARENT as isize;
        let style = if on {
            style | transparent
        } else {
            style & !transparent
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    }
}

fn focus_overlay_under_cursor(app: &App) {
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let target = app
        .overlays
        .iter()
        .find(|o| contains(o.monitor, cursor))
        .or(app.overlays.first());
    if let Some(o) = target {
        let ok = unsafe { SetForegroundWindow(o.hwnd) } != 0;
        log(&format!("focus overlay {:?}: {ok}", o.hwnd));
    }
}

/// One timer drives auto-fade (slow) and the laser fade-out (smooth).
fn update_fade_timer(app: &App) {
    let Some(first) = app.overlays.first() else {
        return;
    };
    let animating = app.overlays.iter().any(|o| o.canvas.animating())
        || app.magnifier.as_ref().is_some_and(|l| l.on);
    unsafe {
        if app.active && animating {
            SetTimer(first.hwnd, FADE_TIMER, 16, None);
        } else if app.active && app.state.auto_fade.is_some() {
            SetTimer(first.hwnd, FADE_TIMER, 50, None);
        } else {
            KillTimer(first.hwnd, FADE_TIMER);
        }
    }
}

fn toggle_magnifier(app: &mut App) {
    if app.magnifier.is_none() {
        app.magnifier = magnifier::Magnifier::new(LENS_CLASS);
    }
    let Some(lens) = &mut app.magnifier else {
        let l = app.lang;
        app.tray.notify(l.t(T::MagnifierUnavailable), "");
        return;
    };
    if lens.on {
        lens.hide();
    } else {
        lens.on = true;
        update_magnifier(app);
    }
    update_fade_timer(app);
    render_palette(app);
}

/// Follows the pointer; called from the animation timer while the lens is on.
fn update_magnifier(app: &mut App) {
    let exclude = own_windows(app);
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let scale = ui::scale_at(cursor);
    if let Some(lens) = app.magnifier.as_mut().filter(|l| l.on) {
        lens.update(cursor, scale, &exclude);
    }
}

/// Our windows that must not end up in a capture of the screen behind them.
fn own_windows(app: &App) -> Vec<HWND> {
    let mut windows: Vec<HWND> = app.overlays.iter().map(|o| o.hwnd).collect();
    windows.extend([app.palette.hwnd, app.palette.tooltip.hwnd]);
    windows
}

/// Puts capture visibility back after a capture: overlays visible, palette per setting.
fn restore_capture_visibility(app: &mut App) {
    for o in &app.overlays {
        ui::set_capture_hidden(o.hwnd, false);
    }
    let hide = app.hide_palette_from_capture;
    set_capture_hiding(app, hide);
}

/// Captures the screen behind an overlay when the magnet needs its edges.
fn feed_magnet(app: &mut App, index: usize, mods: Mods) {
    if !app.overlays[index].canvas.needs_screen(&app.state, mods) {
        return;
    }
    let image = capture::screen(app.overlays[index].monitor, &own_windows(app));
    restore_capture_visibility(app);
    if image.is_none() {
        log("magnet: no screen capture");
    }
    app.overlays[index].canvas.set_screen(image);
}

fn toggle_freeze(app: &mut App) {
    app.frozen = !app.frozen;
    let images: Vec<Option<tiny_skia::Pixmap>> = if app.frozen {
        let exclude = own_windows(app);
        let images = app
            .overlays
            .iter()
            .map(|o| capture::screen(o.monitor, &exclude))
            .collect();
        restore_capture_visibility(app);
        images
    } else {
        app.overlays.iter().map(|_| None).collect()
    };
    for (o, image) in app.overlays.iter_mut().zip(images) {
        let r = o.canvas.set_backdrop(image);
        paint(o, r);
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
        let exclude = own_windows(app);
        let image = capture::screen(o.monitor, &exclude);
        restore_capture_visibility(app);
        image
    };
    let o = &app.overlays[index];
    // The clean screen goes into the snapshot too, so it can be restored and exported.
    let background = o.canvas.backdrop().cloned().or_else(|| screen.clone());
    let annotations: Vec<_> = o
        .canvas
        .doc
        .annotations()
        .iter()
        .map(|a| (**a).clone())
        .collect();
    let result = o
        .canvas
        .compose(screen)
        .ok_or_else(|| "out of memory".to_string())
        .and_then(|image| {
            let copied = capture::copy_to_clipboard(&image, o.hwnd);
            let stem = capture::snapshot_stem();
            snapshots::save(
                &stem,
                &image,
                background.as_ref(),
                ChannelOrder::Bgra,
                &annotations,
                o.canvas.scale(),
            )
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
            app.tray
                .notify(l.t(T::SnapshotSaved), &path.display().to_string());
        }
        Err(e) => {
            log(&format!("snapshot failed: {e}"));
            app.tray.notify(l.t(T::SnapshotFailed), &e);
        }
    }
}

/// The monitor rect if `hwnd` is a caption-less window covering a whole monitor.
fn covered_monitor(hwnd: HWND) -> Option<RECT> {
    // The desktop and taskbar cover the monitor without a caption too.
    if hwnd.is_null()
        || ["Progman", "WorkerW", "Shell_TrayWnd"].contains(&class_name(hwnd).as_str())
    {
        return None;
    }
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if style & WS_CAPTION == WS_CAPTION {
            return None;
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let (m, _) = monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
        (r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom)
            .then_some(m)
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn same_rect(a: RECT, b: RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

fn contains(r: RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

// MARK: palette and cheat sheet

fn look(app: &App) -> Look {
    Look {
        tool: app.state.tool,
        bold: app.state.text_style.bold,
        italic: app.state.text_style.italic,
        magnifier: app.magnifier.as_ref().is_some_and(|l| l.on),
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
    if !app.palette_placed
        || !app
            .overlays
            .iter()
            .any(|o| contains(o.monitor, app.palette.center()))
    {
        let primary = app
            .overlays
            .iter()
            .find(|o| o.monitor.left == 0 && o.monitor.top == 0)
            .or(app.overlays.first())
            .map_or(POINT { x: 0, y: 0 }, |o| POINT {
                x: o.monitor.left + 1,
                y: o.monitor.top + 1,
            });
        app.palette.origin = app.palette.default_origin(primary);
    }
    let look = look(app);
    app.palette.show(&look);
    set_palette_owner(app);
}

/// The palette is owned by the overlay it sits on, so it always stays above it
/// without being raised again.
fn set_palette_owner(app: &App) {
    let center = app.palette.center();
    if let Some(o) = app
        .overlays
        .iter()
        .find(|o| contains(o.monitor, center))
        .or(app.overlays.first())
    {
        unsafe { SetWindowLongPtrW(app.palette.hwnd, GWLP_HWNDPARENT, o.hwnd as isize) };
    }
}

fn on_palette_pointer(app: &mut App, msg: u32, screen: POINT, flags: usize) {
    let local = app.palette.to_local(screen);
    match msg {
        WM_POINTERDOWN if flags & POINTER_FLAG_FIRSTBUTTON != 0 => {
            match app.palette.palette.hit(local) {
                Some(Hit::Grip) => {
                    app.palette.tooltip.hide();
                    let limit =
                        std::time::Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);
                    let double = app
                        .palette
                        .last_grip_click
                        .is_some_and(|t| t.elapsed() <= limit);
                    if double {
                        app.palette.last_grip_click = None;
                        app.palette.palette.vertical = !app.palette.palette.vertical;
                        let (origin, look) = (app.palette.origin, look(app));
                        app.palette.move_to(origin, &look);
                        app.palette_placed = true;
                        save_settings(app);
                    } else {
                        app.palette.last_grip_click = Some(Instant::now());
                        let o = app.palette.origin;
                        app.palette.drag = Some(POINT {
                            x: screen.x - o.x,
                            y: screen.y - o.y,
                        });
                        unsafe { SetCapture(app.palette.hwnd) };
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
            }
        }
        WM_POINTERUPDATE => {
            if let Some(offset) = app.palette.drag {
                let look = look(app);
                app.palette.move_to(
                    POINT {
                        x: screen.x - offset.x,
                        y: screen.y - offset.y,
                    },
                    &look,
                );
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
        WM_POINTERUP if app.palette.drag.take().is_some() => {
            unsafe { ReleaseCapture() };
            app.palette_placed = true;
            set_palette_owner(app);
            save_settings(app);
        }
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
    let Some(pixmap) = render_tooltip(&text, scale, ChannelOrder::Bgra) else {
        return;
    };
    let o = app.palette.origin;
    let (pw, ph) = app.palette.size();
    let (tw, th) = (pixmap.width() as i32, pixmap.height() as i32);
    let gap = (6.0 * scale) as i32;
    let area = ui::work_area(app.palette.center());
    let at = if app.palette.palette.vertical {
        let y = o.y + ((cell.y + cell.height / 2.0) * scale) as i32 - th / 2;
        let x = if o.x + pw + gap + tw <= area.right {
            o.x + pw + gap
        } else {
            o.x - gap - tw
        };
        POINT { x, y }
    } else {
        let x = (o.x + ((cell.x + cell.width / 2.0) * scale) as i32 - tw / 2)
            .clamp(area.left, area.right - tw);
        let y = if o.y + ph + gap + th <= area.bottom {
            o.y + ph + gap
        } else {
            o.y - gap - th
        };
        POINT { x, y }
    };
    app.palette.tooltip.show(&pixmap, at);
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
    let (monitor, scale) = (o.monitor, o.canvas.scale() * app.state.ui_scale);
    let Some(pixmap) =
        render_cheat_sheet(app.lang, &app.state.shortcuts, scale, ChannelOrder::Bgra)
    else {
        return;
    };
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let at = POINT {
        x: monitor.left + (monitor.right - monitor.left - w) / 2,
        y: monitor.top + (monitor.bottom - monitor.top - h) / 2,
    };
    app.sheet.show(&pixmap, at);
}

/// The overlay that has keyboard focus, or the first one.
fn focused_overlay(app: &App) -> usize {
    let front = unsafe { GetForegroundWindow() };
    app.overlays
        .iter()
        .position(|o| o.hwnd == front)
        .unwrap_or(0)
}

// MARK: actions and keys

fn run_action(app: &mut App, index: usize, action: Action) {
    let Some(o) = app.overlays.get_mut(index) else {
        return;
    };
    let (repaint, request) = o.canvas.perform(&mut app.state, action);
    paint(o, repaint);
    // Toasts and the timer animate.
    update_fade_timer(app);
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
            // Show the new color or width at once, not on the next mouse move.
            let mut at = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut at) };
            if app.active
                && !app.ghost
                && let Some(i) = app.overlays.iter().position(|o| contains(o.monitor, at))
            {
                set_cursor(app, i);
            }
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

/// The cursor over a drawing screen: the brush for drawing tools, else a system one.
fn set_cursor(app: &mut App, index: usize) {
    let scale = app.overlays[index].canvas.scale();
    let tool = app.state.tool;
    let brush = app.brush_cursor.get(tool, &app.state.style(tool), scale);
    let cursor = brush.unwrap_or_else(|| {
        let id = match tool {
            ToolKind::Select => IDC_ARROW,
            ToolKind::Text => IDC_IBEAM,
            _ => IDC_CROSS,
        };
        unsafe { LoadCursorW(null_mut(), id) }
    });
    unsafe { SetCursor(cursor) };
}

fn on_key(app: &mut App, index: usize, vk: u16) {
    let typing = app.overlays[index].canvas.is_typing();
    let Some(chord) = keys::chord_from_vk(vk) else {
        return;
    };
    // Esc closes the cheat sheet first.
    if app.sheet.visible && chord.key == screenpen_core::Key::Escape {
        app.sheet.hide();
        return;
    }
    if typing {
        let o = &mut app.overlays[index];
        if let Some(r) = crate::clipboard::text_key(&mut o.canvas, chord) {
            paint(o, r);
            return;
        }
        if let Some(action) = typing_action(&app.state.shortcuts, chord) {
            run_action(app, index, action);
            return;
        }
        // While typing, letters are text; only Esc (finish) is a shortcut.
        if chord.key == screenpen_core::Key::Escape {
            run_action(app, index, Action::Escape);
        }
        return;
    }
    if let Some(action) = app.state.shortcuts.local_action(chord) {
        run_action(app, index, action);
    } else if !chord.ctrl
        && !chord.alt
        && [VK_PRIOR, VK_NEXT, VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN].contains(&vk)
    {
        forward_key(app, vk);
    }
}

/// Sends a slide key to the window that was in front before we took focus.
fn forward_key(app: &App, vk: u16) {
    let target = app.previous;
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) };
    // repeat 1, scan code, extended bit (these are all extended keys)
    let down = 1 | (scan as isize) << 16 | 1 << 24;
    let up = down | 1 << 30 | 1 << 31;
    let ok = unsafe {
        PostMessageW(target, WM_KEYDOWN, vk as WPARAM, down) != 0
            && PostMessageW(target, WM_KEYUP, vk as WPARAM, up) != 0
    };
    log(&format!("forward vk {vk:#x} to {target:?}: {ok}"));
}

// MARK: tray

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
        Command::Quit => unsafe { PostQuitMessage(0) },
    }
    // Every tray command changes something worth keeping, except toggling drawing.
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

/// Tray window messages. The menu runs a modal loop, so it's shown with the app
/// state released and the picked command is applied afterwards.
fn on_tray_message(hwnd: HWND, msg: u32, lparam: LPARAM) -> bool {
    let mut menu = None;
    let mut taskbar_created = 0;
    with_app(|app| {
        if hwnd == app.tray_hwnd {
            menu = Some(menu_state(app));
            taskbar_created = app.taskbar_created;
        }
    });
    let Some(menu) = menu else { return false };
    match msg {
        WM_TRAY => match lparam as u32 {
            WM_LBUTTONUP => with_app(|app| on_tray_command(app, Command::ToggleDrawing)),
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                if let Some(command) = Tray::menu(hwnd, &menu) {
                    with_app(|app| on_tray_command(app, command));
                }
            }
            _ => {}
        },
        WM_RELOAD_SETTINGS => with_app(|app| {
            log("settings changed by the settings window");
            reload_settings(app);
        }),
        WM_QUIT_REQUEST => {
            log("quit requested by a newer version");
            unsafe { PostQuitMessage(0) };
        }
        WM_SECOND_INSTANCE => with_app(|app| {
            let key = chord_text(app, Action::ToggleDrawing);
            let l = app.lang;
            app.tray.notify(
                l.t(T::AlreadyRunning),
                &l.t(T::HowToDraw).replace("{key}", &key),
            );
        }),
        m if m == taskbar_created => with_app(|app| {
            let ok = app.tray.add();
            log(&format!("explorer restarted, tray icon re-added: {ok}"));
        }),
        _ => return false,
    }
    true
}

// MARK: window procedure

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if on_tray_message(hwnd, msg, lparam) {
        return 0;
    }
    let mut handled: Option<LRESULT> = None;
    match msg {
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
            let p = POINT {
                x: (lparam & 0xffff) as i16 as i32,
                y: ((lparam >> 16) & 0xffff) as i16 as i32,
            };
            let flags = (wparam >> 16) & 0xffff;
            with_app(|app| {
                handled = Some(0);
                if hwnd == app.palette.hwnd {
                    on_palette_pointer(app, msg, p, flags);
                    return;
                }
                if hwnd == app.sheet.hwnd {
                    if msg == WM_POINTERDOWN {
                        app.sheet.hide();
                    }
                    return;
                }
                if let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) {
                    if app.palette.palette.hover.is_some() || app.palette.grip_hover {
                        clear_palette_hover(app);
                    }
                    let id = (wparam & 0xffff) as u32;
                    let pen = pen_info(id);
                    on_pointer(app, index, msg, p, flags, id, pen);
                }
            });
        }
        // The gesture's pointer or mouse capture went elsewhere (lock screen, UAC, a
        // cancelled pen): drop the gesture, or every later press would be ignored.
        WM_POINTERCAPTURECHANGED | WM_CAPTURECHANGED => with_app(|app| {
            if let Some((i, _, _)) = app.pressed
                && app.overlays.get(i).is_some_and(|o| o.hwnd == hwnd)
            {
                app.pressed = None;
                let o = &mut app.overlays[i];
                let r = o.canvas.cancel();
                paint(o, r);
            }
        }),
        WM_POINTERLEAVE => with_app(|app| {
            if hwnd == app.palette.hwnd && app.palette.drag.is_none() {
                clear_palette_hover(app);
            }
        }),
        WM_POINTERWHEEL => with_app(|app| {
            let (ctrl, _, _) = keys::modifiers();
            let up = ((wparam >> 16) & 0xffff) as i16 > 0;
            // Ctrl+wheel over the palette scales the interface.
            if hwnd == app.palette.hwnd {
                handled = Some(0);
                if ctrl {
                    app.state.ui_scale = step_ui_scale(app.state.ui_scale, up);
                    app.palette.ui_scale = app.state.ui_scale;
                    app.palette.tooltip.hide();
                    let (origin, look) = (app.palette.origin, look(app));
                    app.palette.move_to(origin, &look);
                    save_settings(app);
                }
                return;
            }
            let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) else {
                return;
            };
            handled = Some(0);
            if let Some(lens) = app.magnifier.as_mut().filter(|l| l.on && !ctrl) {
                lens.zoom_step(up);
                update_magnifier(app);
            } else if !ctrl && app.overlays[index].canvas.spotlight_on() {
                let o = &mut app.overlays[index];
                o.canvas.resize_spotlight(up);
                paint(o, Repaint::Full);
            } else if ctrl && app.overlays[index].canvas.is_typing() {
                // While typing, Ctrl+wheel sizes the text.
                let action = if up {
                    Action::TextBigger
                } else {
                    Action::TextSmaller
                };
                run_action(app, index, action);
            } else if ctrl {
                let action = if up { Action::Thicker } else { Action::Thinner };
                run_action(app, index, action);
            }
        }),
        WM_KEYDOWN | WM_SYSKEYDOWN => with_app(|app| {
            if let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) {
                on_key(app, index, wparam as u16);
                // Swallow Alt combos too, so they don't beep or open a system menu.
                handled = Some(0);
            }
        }),
        WM_CHAR | WM_SYSCHAR => with_app(|app| {
            let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) else {
                return;
            };
            handled = Some(0);
            let o = &mut app.overlays[index];
            if o.canvas.is_typing()
                && let Some(c) = char::from_u32(wparam as u32)
            {
                let r = o.canvas.type_char(c);
                paint(o, r);
            }
        }),
        WM_SETCURSOR => with_app(|app| {
            if let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) {
                set_cursor(app, index);
                handled = Some(1);
            }
        }),
        WM_TIMER if wparam == FADE_TIMER => with_app(|app| {
            handled = Some(0);
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
        }),
        WM_MOUSEACTIVATE => with_app(|app| {
            if hwnd == app.palette.hwnd || hwnd == app.sheet.hwnd {
                handled = Some(MA_NOACTIVATE as LRESULT);
            }
        }),
        WM_POINTERACTIVATE => with_app(|app| {
            if hwnd == app.palette.hwnd || hwnd == app.sheet.hwnd {
                handled = Some(PA_NOACTIVATE);
            }
        }),
        WM_DISPLAYCHANGE => with_app(|app| app.rebuild = true),
        // Our windows already match their monitor; don't take the suggested rect.
        WM_DPICHANGED => with_app(|app| {
            handled = Some(0);
            if app.overlays.iter().any(|o| o.hwnd == hwnd) {
                app.rebuild = true;
            }
        }),
        _ => {}
    }
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

/// Pressure (0..=1) and whether the eraser end touches, for a pen; None for mouse and touch.
fn pen_info(pointer_id: u32) -> Option<(f32, bool)> {
    unsafe {
        let mut kind = 0;
        if GetPointerType(pointer_id, &mut kind) == 0 || kind != PT_PEN {
            return None;
        }
        let mut info: POINTER_PEN_INFO = std::mem::zeroed();
        if GetPointerPenInfo(pointer_id, &mut info) == 0 {
            return None;
        }
        // Pens without pressure report 0; draw those at full width.
        let pressure = if info.penMask & PEN_MASK_PRESSURE != 0 {
            info.pressure as f32 / 1024.0
        } else {
            1.0
        };
        let eraser = info.penFlags & (PEN_FLAG_ERASER | PEN_FLAG_INVERTED) != 0;
        Some((pressure, eraser))
    }
}

fn on_pointer(
    app: &mut App,
    index: usize,
    msg: u32,
    screen: POINT,
    flags: usize,
    id: u32,
    pen: Option<(f32, bool)>,
) {
    if app.ghost {
        return;
    }
    let (ctrl, alt, shift) = keys::modifiers();
    let o = &app.overlays[index];
    let mut pointer = Pointer {
        x: (screen.x - o.monitor.left) as f32,
        y: (screen.y - o.monitor.top) as f32,
        pressure: pen.map_or(1.0, |(p, _)| p),
        mods: Mods { ctrl, alt, shift },
        double_click: false,
    };
    feed_magnet(app, index, pointer.mods);
    let repaint = match msg {
        WM_POINTERDOWN if app.pressed.is_none() => {
            // The pen's eraser end erases like the right mouse button.
            let button = if flags & POINTER_FLAG_SECONDBUTTON != 0
                || pen.is_some_and(|(_, eraser)| eraser)
            {
                Button::Right
            } else if flags & POINTER_FLAG_FIRSTBUTTON != 0 {
                Button::Left
            } else {
                return;
            };
            pointer.double_click = button == Button::Left && is_double_click(app, screen);
            app.pressed = Some((index, button, id));
            let hwnd = app.overlays[index].hwnd;
            unsafe {
                SetCapture(hwnd);
                // Keys go to the screen being drawn on.
                if GetForegroundWindow() != hwnd {
                    SetForegroundWindow(hwnd);
                }
            }
            app.overlays[index]
                .canvas
                .pointer_down(&app.state, pointer, button)
        }
        // Only the pointer that started the gesture moves it: a palm or a second
        // finger on the same screen doesn't.
        WM_POINTERUPDATE => match app.pressed {
            Some((i, _, p)) if i == index && p == id => {
                app.overlays[index].canvas.pointer_move(&app.state, pointer)
            }
            Some(_) => return,
            None => app.overlays[index].canvas.hover(&app.state, pointer),
        },
        WM_POINTERUP => match app.pressed {
            Some((i, _, p)) if i == index && p == id => {
                app.pressed = None;
                unsafe { ReleaseCapture() };
                let canvas = &mut app.overlays[index].canvas;
                if flags & POINTER_FLAG_CANCELED != 0 {
                    canvas.cancel()
                } else {
                    canvas.pointer_up(&app.state, pointer)
                }
            }
            _ => return,
        },
        _ => return,
    };
    paint(&mut app.overlays[index], repaint);
    // Down starts the ticks a held stroke needs to straighten.
    if msg == WM_POINTERUP || msg == WM_POINTERDOWN {
        update_fade_timer(app);
    }
}

fn is_double_click(app: &mut App, p: POINT) -> bool {
    let now = Instant::now();
    let limit = std::time::Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);
    let double = app.last_click.is_some_and(|(t, q)| {
        now.duration_since(t) <= limit && (q.x - p.x).abs() <= 4 && (q.y - p.y).abs() <= 4
    });
    app.last_click = if double { None } else { Some((now, p)) };
    double
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
