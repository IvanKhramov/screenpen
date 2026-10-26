//! Windows backend: a layered overlay window per monitor drawing a `Canvas`, input
//! through WM_POINTER, global hotkeys, ghost (click-through) mode, slide-key
//! forwarding to the previous foreground window, the palette and the tray icon.
//! Diagnostics go to %TEMP%\screenpen.log.

mod instance;
mod keys;
mod surface;
mod tray;

use std::cell::RefCell;
use std::io::Write as _;
use std::ptr::{null, null_mut};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use screenpen_core::render::ChannelOrder;
use screenpen_core::{Action, Rgba, ToolKind};
use surface::{Surface, bgra};
use tiny_skia::{Paint, Rect, Transform};
use tray::{Command, MenuState, Mode, Tray, WM_QUIT_REQUEST, WM_SECOND_INSTANCE, WM_TRAY};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow,
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
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
    GWL_STYLE, GWLP_HWNDPARENT, GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_IBEAM, KillTimer,
    LoadCursorW, MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetCursor,
    SetForegroundWindow, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_CHAR,
    WM_CONTEXTMENU, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_POINTERACTIVATE, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE,
    WM_POINTERWHEEL, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW,
    WS_CAPTION, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::canvas::{Button, Canvas, Mods, Pointer, Repaint, Request};
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
    palette: HWND,
    palette_surface: Surface,
    palette_origin: POINT,
    palette_scale: f32,
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
    let (palette, palette_surface, palette_scale) = create_palette();
    let state = ToolState::default();
    let app = App {
        overlays: create_overlays(&state),
        state,
        palette,
        palette_surface,
        palette_origin: POINT { x: 0, y: 0 },
        palette_scale,
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
        let taken = register_hotkeys(app);
        let key = chord_text(app, Action::ToggleDrawing);
        if let Some(old) = &replaced {
            let text = format!("Replaced the running copy ({old}).");
            app.tray.notify(&instance::title(), &text);
        } else if taken.is_empty() {
            app.tray.notify(
                "screenpen is running",
                &format!("{key} or click the tray icon to draw."),
            );
        } else {
            let text = format!(
                "Taken by another app: {}. Use the tray icon.",
                taken.join(", ")
            );
            app.tray.notify("screenpen: hotkey unavailable", &text);
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
        .map_or_else(|| "(no shortcut)".to_string(), |c| c.to_string())
}

// MARK: windows

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const OVERLAY_CLASS: &str = "screenpen.overlay";
const PALETTE_CLASS: &str = "screenpen.palette";
const TRAY_CLASS: &str = "screenpen.tray";

fn register_classes() {
    unsafe {
        let instance = GetModuleHandleW(null());
        for name in [OVERLAY_CLASS, PALETTE_CLASS, TRAY_CLASS] {
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

fn create_palette() -> (HWND, Surface, f32) {
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let (_, scale) = monitor_info(unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) });
    let (w, h) = palette_size(scale);
    let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let hwnd = create_window(PALETTE_CLASS, ex, w as i32, h as i32);
    (hwnd, Surface::new(w, h).expect("palette surface"), scale)
}

/// Nine swatches, then "clear" and "ghost" buttons. Replaced by the real palette in phase 3.
const PALETTE_CELLS: usize = 11;

fn palette_size(scale: f32) -> (u32, u32) {
    let cell = 36.0 * scale;
    (
        (PALETTE_CELLS as f32 * cell + 8.0 * scale) as u32,
        (cell + 8.0 * scale) as u32,
    )
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
    unsafe { ShowWindow(app.palette, SW_HIDE) };
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
    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    let ok = unsafe { SetWindowDisplayAffinity(app.palette, affinity) } != 0;
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

// MARK: palette (temporary until phase 3)

fn show_palette(app: &mut App) {
    // Top center of the primary monitor, owned by its overlay so it always stays above
    // it without re-raising.
    if let Some(o) = app
        .overlays
        .iter()
        .find(|o| o.monitor.left == 0 && o.monitor.top == 0)
        .or(app.overlays.first())
    {
        unsafe { SetWindowLongPtrW(app.palette, GWLP_HWNDPARENT, o.hwnd as isize) };
        let width = (o.monitor.right - o.monitor.left) - app.palette_surface.width as i32;
        app.palette_origin = POINT {
            x: o.monitor.left + width / 2,
            y: o.monitor.top + (16.0 * o.canvas.scale()) as i32,
        };
    }
    render_palette(app);
    unsafe {
        ShowWindow(app.palette, SW_SHOWNA);
        SetWindowPos(
            app.palette,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

fn render_palette(app: &mut App) {
    if !app.active {
        return;
    }
    let s = app.palette_scale;
    let cell = 36.0 * s;
    let pad = 4.0 * s;
    let (selected, ghost) = (app.state.color_index, app.ghost);
    let colors = app.state.colors;
    let mut pixmap = app.palette_surface.pixmap();
    pixmap.fill(bgra(0, 0, 0, 0));
    let mut bg = Paint::default();
    bg.set_color(bgra(0x2c, 0x2c, 0x2e, 235));
    let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
    pixmap.fill_rect(
        Rect::from_xywh(0.0, 0.0, w, h).unwrap(),
        &bg,
        Transform::identity(),
        None,
    );
    let mut fill = Paint::default();
    for i in 0..PALETTE_CELLS {
        let x = pad + i as f32 * cell;
        let inset = if i == selected { 3.0 * s } else { 8.0 * s };
        let color = match i {
            i if i < colors.len() => {
                let c = colors[i];
                bgra(
                    (c.red * 255.0) as u8,
                    (c.green * 255.0) as u8,
                    (c.blue * 255.0) as u8,
                    255,
                )
            }
            9 => bgra(0x8e, 0x8e, 0x93, 255),          // clear
            _ if ghost => bgra(0xaf, 0x52, 0xde, 255), // ghost on
            _ => bgra(0x48, 0x48, 0x4a, 255),          // ghost off
        };
        fill.set_color(color);
        let r = Rect::from_xywh(
            x + inset,
            pad + inset,
            cell - 2.0 * inset,
            cell - 2.0 * inset,
        )
        .unwrap();
        pixmap.fill_rect(r, &fill, Transform::identity(), None);
    }
    let POINT { x, y } = app.palette_origin;
    let height = app.palette_surface.height;
    app.palette_surface.present(app.palette, x, y, height, None);
}

fn on_palette_click(app: &mut App, screen: POINT) {
    let s = app.palette_scale;
    let i = ((screen.x - app.palette_origin.x) as f32 - 4.0 * s) / (36.0 * s);
    if i < 0.0 {
        return;
    }
    let target = focused_overlay(app);
    match i as usize {
        i if i < 9 => run_action(app, target, Action::Color(i as u8)),
        9 => {
            for o in 0..app.overlays.len() {
                run_action(app, o, Action::Clear);
            }
        }
        10 => set_ghost(app, !app.ghost),
        _ => {}
    }
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
        Some(Request::Help) => log("help: arrives with the phase 3 cheat sheet"),
        Some(Request::StateChanged) => {
            // Tool, color or width changed: other screens drop their gestures.
            for (i, o) in app.overlays.iter_mut().enumerate() {
                if i != index {
                    let r = o.canvas.cancel();
                    paint(o, r);
                }
            }
            render_palette(app);
        }
    }
}

fn on_key(app: &mut App, index: usize, vk: u16) {
    let typing = app.overlays[index].canvas.is_typing();
    let Some(chord) = keys::chord_from_vk(vk) else {
        return;
    };
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
        Command::Quit => unsafe { PostQuitMessage(0) },
    }
}

fn menu_state(app: &App) -> MenuState {
    MenuState {
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
            app.tray.notify(
                "screenpen is already running",
                &format!("{key} or click the tray icon to draw."),
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
                if hwnd == app.palette {
                    if msg == WM_POINTERDOWN {
                        on_palette_click(app, p);
                    }
                    return;
                }
                if let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) {
                    on_pointer(app, index, msg, p, flags);
                }
            });
        }
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
            if hwnd == app.palette {
                handled = Some(MA_NOACTIVATE as LRESULT);
            }
        }),
        WM_POINTERACTIVATE => with_app(|app| {
            if hwnd == app.palette {
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
