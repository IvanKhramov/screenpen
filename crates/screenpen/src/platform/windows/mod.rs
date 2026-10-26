//! Windows backend: a layered overlay window per monitor drawing a `Canvas`, input
//! through WM_POINTER, global hotkeys, ghost (click-through) mode, slide-key
//! forwarding to the previous foreground window, the palette and the tray icon.
//! Diagnostics go to %TEMP%\screenpen.log.

mod instance;
mod keys;
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
use tray::{Command, MenuState, Mode, Tray, WM_QUIT_REQUEST, WM_SECOND_INSTANCE, WM_TRAY};
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
use windows_sys::Win32::UI::Input::Pointer::EnableMouseInPointer;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
    GWL_STYLE, GWLP_HWNDPARENT, GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_IBEAM, KillTimer,
    LoadCursorW, MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SW_HIDE, SW_SHOWNA, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetCursor, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, TranslateMessage, WM_CHAR, WM_CONTEXTMENU, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_POINTERACTIVATE, WM_POINTERDOWN, WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE,
    WM_POINTERWHEEL, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW,
    WS_CAPTION, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::canvas::{Button, Canvas, Mods, Pointer, Repaint, Request};
use crate::config::{PaletteSettings, Settings};
use crate::i18n::{Lang, T};
use crate::palette::{Hit, Item, Look, Palette, render_cheat_sheet, render_tooltip};
use crate::state::{DIM_LEVELS, FADE_SECONDS, ToolState};

const PA_NOACTIVATE: LRESULT = 3; // WM_POINTERACTIVATE result
const POINTER_FLAG_FIRSTBUTTON: usize = 0x0010; // HIWORD(wParam) of WM_POINTER*
const POINTER_FLAG_SECONDBUTTON: usize = 0x0020;
const FADE_TIMER: usize = 1;
const GLOBAL_ACTIONS: [(i32, Action); 3] = [
    (1, Action::ToggleDrawing),
    (2, Action::ExitGhost),
    (3, Action::Quit),
];

struct Overlay {
    hwnd: HWND,
    monitor: RECT,
    surface: Surface,
    canvas: Canvas,
    /// Cover the monitor's last pixel row too (only above a full-screen app).
    full_height: bool,
}

struct App {
    overlays: Vec<Overlay>,
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
    /// Overlay index and button of the gesture in progress.
    pressed: Option<(usize, Button)>,
    last_click: Option<(Instant, POINT)>,
    previous: HWND,
    hide_palette_from_capture: bool,
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
    let palette = Palette {
        items: Item::defaults(),
        vertical: false,
        hover: None,
    };
    let sheet = ui::Popup::new(SHEET_CLASS, false);
    // The cheat sheet is for the presenter only.
    ui::set_capture_hidden(sheet.hwnd, true);
    let app = App {
        overlays: create_overlays(&state),
        state,
        palette: ui::PaletteWindow::new(PALETTE_CLASS, TOOLTIP_CLASS, palette),
        palette_placed: false,
        sheet,
        settings_path: settings_path(),
        language: "auto".to_string(),
        lang: system_lang(),
        active: false,
        ghost: false,
        pressed: None,
        last_click: None,
        previous: null_mut(),
        hide_palette_from_capture: false,
        rebuild: false,
        tray: Tray::new(tray_hwnd),
        tray_hwnd,
        taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(|app| {
        let first_run = !app.settings_path.exists();
        load_settings(app);
        if first_run {
            // Write the defaults so there's a file to look at and edit.
            save_settings(app);
        }
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

fn settings_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("screenpen").join("settings.toml")
}

fn system_lang() -> Lang {
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
        log(&format!("settings not loaded, using defaults: {e}"));
    }
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
    let p = &app.palette;
    let palette = PaletteSettings {
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

fn open_settings(app: &mut App) {
    save_settings(app);
    // Notepad rather than the .toml association, which often isn't set. The settings
    // window replaces this in phase 3b; changes apply the next time drawing starts.
    let args = wide(&format!("\"{}\"", app.settings_path.display()));
    let ok = unsafe {
        ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            wide("notepad.exe").as_ptr(),
            args.as_ptr(),
            null(),
            SW_SHOWNORMAL,
        ) as isize
    } > 32;
    log(&format!("open settings file: {ok}"));
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
        let (Some(surface), Some(mut canvas)) = (
            Surface::new(w, h),
            Canvas::new(w, h, scale, ChannelOrder::Bgra),
        ) else {
            log("out of memory for an overlay");
            continue;
        };
        canvas.set_auto_fade(state.auto_fade);
        log(&format!(
            "overlay {hwnd:?} monitor {w}x{h} at ({}, {}) scale {scale}",
            r.left, r.top
        ));
        out.push(Overlay {
            hwnd,
            monitor: r,
            surface,
            canvas,
            full_height: false,
        });
    }
    out
}

fn rebuild_overlays(app: &mut App) {
    log("display change: rebuilding overlays (drawings are kept per monitor position)");
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
        show(app);
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

fn paint(o: &mut Overlay, repaint: Repaint) {
    let mut target = o.surface.pixmap();
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
    let h = o.surface.height - if o.full_height { 0 } else { 1 };
    if !o
        .surface
        .present(o.hwnd, o.monitor.left, o.monitor.top, h, dirty)
    {
        log("UpdateLayeredWindowIndirect failed");
    }
}

fn show(app: &mut App) {
    app.previous = unsafe { GetForegroundWindow() };
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
        set_click_through(o.hwnd, false);
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
    focus_overlay_under_cursor(app);
    update_tray(app);
}

fn hide(app: &mut App) {
    app.active = false;
    app.pressed = None;
    for o in &mut app.overlays {
        o.canvas.cancel();
        unsafe { ShowWindow(o.hwnd, SW_HIDE) };
    }
    app.palette.hide();
    app.sheet.hide();
    update_fade_timer(app);
    let ok = unsafe { SetForegroundWindow(app.previous) } != 0;
    log(&format!("hide: focus back to {:?}: {ok}", app.previous));
    update_tray(app);
}

fn set_ghost(app: &mut App, ghost: bool) {
    app.ghost = ghost;
    app.pressed = None;
    for o in &mut app.overlays {
        o.canvas.cancel();
        set_click_through(o.hwnd, ghost);
    }
    refresh_all(app);
    if ghost {
        let ok = unsafe { SetForegroundWindow(app.previous) } != 0;
        log(&format!("ghost on: focus back to {:?}: {ok}", app.previous));
    } else {
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

fn update_fade_timer(app: &App) {
    let Some(first) = app.overlays.first() else {
        return;
    };
    unsafe {
        if app.active && app.state.auto_fade.is_some() {
            SetTimer(first.hwnd, FADE_TIMER, 50, None);
        } else {
            KillTimer(first.hwnd, FADE_TIMER);
        }
    }
}

/// The monitor rect if `hwnd` is a caption-less window covering a whole monitor.
fn covered_monitor(hwnd: HWND) -> Option<RECT> {
    if hwnd.is_null() {
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
        color_index: app.state.color_index,
        colors: app.state.colors,
        ghost: app.ghost,
        whiteboard: app.state.whiteboard,
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
                    let item = app.palette.palette.items[i];
                    palette_item(app, item);
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
            app.palette.palette.items[i].tooltip(app.lang, &app.state.shortcuts),
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

fn palette_item(app: &mut App, item: Item) {
    let target = focused_overlay(app);
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
        app.sheet.hide();
        return;
    }
    let Some(o) = app.overlays.get(index) else {
        return;
    };
    let (monitor, scale) = (o.monitor, o.canvas.scale());
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
    match request {
        None => {}
        Some(Request::StopDrawing) => hide(app),
        Some(Request::Ghost) => set_ghost(app, true),
        Some(Request::ToggleWhiteboard) => {
            app.state.whiteboard = !app.state.whiteboard;
            refresh_all(app);
        }
        Some(Request::Help) => toggle_sheet(app, index),
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
                    on_pointer(app, index, msg, p, flags);
                }
            });
        }
        WM_POINTERLEAVE => with_app(|app| {
            if hwnd == app.palette.hwnd && app.palette.drag.is_none() {
                clear_palette_hover(app);
            }
        }),
        WM_POINTERWHEEL => with_app(|app| {
            let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) else {
                return;
            };
            handled = Some(0);
            let (ctrl, _, _) = keys::modifiers();
            if ctrl {
                let delta = ((wparam >> 16) & 0xffff) as i16;
                let action = if delta > 0 {
                    Action::Thicker
                } else {
                    Action::Thinner
                };
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
            if app.overlays.iter().any(|o| o.hwnd == hwnd) {
                let cursor = match app.state.tool {
                    ToolKind::Select => IDC_ARROW,
                    ToolKind::Text => IDC_IBEAM,
                    _ => IDC_CROSS,
                };
                unsafe { SetCursor(LoadCursorW(null_mut(), cursor)) };
                handled = Some(1);
            }
        }),
        WM_TIMER if wparam == FADE_TIMER => with_app(|app| {
            handled = Some(0);
            for o in &mut app.overlays {
                let (repaint, _) = o.canvas.tick();
                if repaint != Repaint::None {
                    paint(o, repaint);
                }
            }
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
        WM_DPICHANGED => handled = Some(0),
        _ => {}
    }
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

fn on_pointer(app: &mut App, index: usize, msg: u32, screen: POINT, flags: usize) {
    if app.ghost {
        return;
    }
    let (ctrl, alt, shift) = keys::modifiers();
    let o = &app.overlays[index];
    let mut pointer = Pointer {
        x: (screen.x - o.monitor.left) as f32,
        y: (screen.y - o.monitor.top) as f32,
        pressure: 1.0,
        mods: Mods { ctrl, alt, shift },
        double_click: false,
    };
    let repaint = match msg {
        WM_POINTERDOWN if app.pressed.is_none() => {
            let button = if flags & POINTER_FLAG_SECONDBUTTON != 0 {
                Button::Right
            } else if flags & POINTER_FLAG_FIRSTBUTTON != 0 {
                Button::Left
            } else {
                return;
            };
            pointer.double_click = button == Button::Left && is_double_click(app, screen);
            app.pressed = Some((index, button));
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
        WM_POINTERUPDATE => match app.pressed {
            Some((i, _)) if i == index => {
                app.overlays[index].canvas.pointer_move(&app.state, pointer)
            }
            Some(_) => return,
            None => app.overlays[index].canvas.hover(&app.state, pointer),
        },
        WM_POINTERUP => match app.pressed {
            Some((i, _)) if i == index => {
                app.pressed = None;
                unsafe { ReleaseCapture() };
                app.overlays[index].canvas.pointer_up(&app.state, pointer)
            }
            _ => return,
        },
        _ => return,
    };
    paint(&mut app.overlays[index], repaint);
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
