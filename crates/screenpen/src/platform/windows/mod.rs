//! Phase 0 spike: a layered overlay window per monitor, mouse drawing through
//! WM_POINTER, global hotkeys, ghost (click-through) mode, slide-key forwarding to
//! the previous foreground window and a palette window that can hide from capture.
//! Everything is logged to %TEMP%\screenpen.log for the manual checklist in PLAN.md.

mod surface;

use std::cell::RefCell;
use std::io::Write as _;
use std::ptr::{null, null_mut};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use surface::{Surface, bgra};
use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Rect, Stroke, Transform};
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
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MAPVK_VK_TO_VSC, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MapVirtualKeyW, RegisterHotKey,
    SendInput, VK_DOWN, VK_ESCAPE, VK_F8, VK_F9, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows_sys::Win32::UI::Input::Pointer::EnableMouseInPointer;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DispatchMessageW, GWL_EXSTYLE,
    GWL_STYLE, GWLP_HWNDPARENT, GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, IDC_CROSS, LoadCursorW, MA_NOACTIVATE, MSG,
    PostMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_MOUSEACTIVATE, WM_POINTERACTIVATE,
    WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE, WNDCLASSEXW, WS_CAPTION, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const HOTKEY_TOGGLE: i32 = 1; // Ctrl+Alt+Shift+A
const HOTKEY_UNGHOST: i32 = 2; // Ctrl+Alt+Shift+G
const HOTKEY_QUIT: i32 = 3; // Ctrl+Alt+Shift+Q (spike only, until the tray exists)
const PA_NOACTIVATE: LRESULT = 3; // WM_POINTERACTIVATE result
const POINTER_FLAG_FIRSTBUTTON: usize = 0x0010; // HIWORD(wParam) of WM_POINTER*

const SWATCHES: [(u8, u8, u8); 5] = [
    (0xff, 0x3b, 0x30),
    (0xff, 0xcc, 0x00),
    (0x34, 0xc7, 0x59),
    (0x0a, 0x84, 0xff),
    (0xff, 0xff, 0xff),
];

struct Overlay {
    hwnd: HWND,
    monitor: RECT,
    scale: f32,
    surface: Surface,
    strokes: Vec<(usize, Vec<(f32, f32)>)>, // (swatch, points in surface pixels)
    /// Cover the monitor's last pixel row too (only above a full-screen app).
    full_height: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Forward {
    Post,
    Input,
}

struct App {
    overlays: Vec<Overlay>,
    palette: HWND,
    palette_surface: Surface,
    palette_origin: POINT,
    palette_scale: f32,
    active: bool,
    ghost: bool,
    drawing: Option<usize>, // overlay index
    swatch: usize,
    previous: HWND,
    hide_palette_from_capture: bool,
    forward: Forward,
    rebuild: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        EnableMouseInPointer(1);
    }
    log("start");
    register_classes();
    let (palette, palette_surface, palette_scale) = create_palette();
    let app = App {
        overlays: create_overlays(),
        palette,
        palette_surface,
        palette_origin: POINT { x: 0, y: 0 },
        palette_scale,
        active: false,
        ghost: false,
        drawing: None,
        swatch: 0,
        previous: null_mut(),
        hide_palette_from_capture: false,
        forward: Forward::Post,
        rebuild: false,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    let mods = MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT;
    for (id, key) in [
        (HOTKEY_TOGGLE, b'A'),
        (HOTKEY_UNGHOST, b'G'),
        (HOTKEY_QUIT, b'Q'),
    ] {
        let ok = unsafe { RegisterHotKey(null_mut(), id, mods, key as u32) } != 0;
        log(&format!(
            "hotkey Ctrl+Alt+Shift+{} registered: {ok}",
            key as char
        ));
    }

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

// MARK: windows

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const OVERLAY_CLASS: &str = "screenpen.overlay";
const PALETTE_CLASS: &str = "screenpen.palette";

fn register_classes() {
    unsafe {
        let instance = GetModuleHandleW(null());
        for (name, cursor) in [(OVERLAY_CLASS, IDC_CROSS), (PALETTE_CLASS, null())] {
            let name = wide(name);
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: if cursor.is_null() {
                    null_mut()
                } else {
                    LoadCursorW(null_mut(), cursor)
                },
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassExW(&class);
        }
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

fn create_overlays() -> Vec<Overlay> {
    let class = wide(OVERLAY_CLASS);
    let mut out = Vec::new();
    for m in monitors() {
        let (r, scale) = monitor_info(m);
        let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                r.left,
                r.top,
                w as i32,
                h as i32 - 1,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        let Some(surface) = Surface::new(w, h) else {
            continue;
        };
        log(&format!(
            "overlay {:?} monitor {}x{} at ({}, {}) scale {scale}",
            hwnd, w, h, r.left, r.top
        ));
        out.push(Overlay {
            hwnd,
            monitor: r,
            scale,
            surface,
            strokes: Vec::new(),
            full_height: false,
        });
    }
    out
}

fn rebuild_overlays(app: &mut App) {
    log("display change: rebuilding overlays (spike drops strokes)");
    for o in app.overlays.drain(..) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(o.hwnd) };
    }
    app.overlays = create_overlays();
    if app.active {
        show(app);
    }
}

fn create_palette() -> (HWND, Surface, f32) {
    let class = wide(PALETTE_CLASS);
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let (_, scale) = monitor_info(unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) });
    let (w, h) = palette_size(scale);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            w as i32,
            h as i32,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        )
    };
    (hwnd, Surface::new(w, h).expect("palette surface"), scale)
}

/// Swatches, then "clear" and "ghost" buttons.
fn palette_size(scale: f32) -> (u32, u32) {
    let cell = 40.0 * scale;
    (
        ((SWATCHES.len() + 2) as f32 * cell + 8.0 * scale) as u32,
        (cell + 8.0 * scale) as u32,
    )
}

// MARK: show / hide / ghost

fn on_hotkey(app: &mut App, id: i32) {
    match id {
        HOTKEY_TOGGLE if app.active => hide(app),
        HOTKEY_TOGGLE => show(app),
        HOTKEY_UNGHOST if app.active && app.ghost => set_ghost(app, false),
        HOTKEY_QUIT => unsafe { PostQuitMessage(0) },
        _ => {}
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
        render(o, false);
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
}

fn hide(app: &mut App) {
    app.active = false;
    app.drawing = None;
    for o in &app.overlays {
        unsafe { ShowWindow(o.hwnd, SW_HIDE) };
    }
    unsafe { ShowWindow(app.palette, SW_HIDE) };
    let ok = unsafe { SetForegroundWindow(app.previous) } != 0;
    log(&format!("hide: focus back to {:?}: {ok}", app.previous));
}

fn set_ghost(app: &mut App, ghost: bool) {
    app.ghost = ghost;
    app.drawing = None;
    for o in &mut app.overlays {
        set_click_through(o.hwnd, ghost);
        render(o, ghost);
    }
    render_palette(app);
    if ghost {
        let ok = unsafe { SetForegroundWindow(app.previous) } != 0;
        log(&format!("ghost on: focus back to {:?}: {ok}", app.previous));
    } else {
        focus_overlay_under_cursor(app);
        log("ghost off");
    }
}

fn set_click_through(hwnd: HWND, on: bool) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = if on {
            style | WS_EX_TRANSPARENT as isize
        } else {
            style & !(WS_EX_TRANSPARENT as isize)
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

fn show_palette(app: &mut App) {
    // Top center of the primary overlay's monitor, owned by that overlay so it always
    // stays above it without re-raising.
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
            y: o.monitor.top + (16.0 * o.scale) as i32,
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

// MARK: rendering

fn stroke_paint(swatch: usize) -> Paint<'static> {
    let (r, g, b) = SWATCHES[swatch];
    let mut paint = Paint::default();
    paint.set_color(bgra(r, g, b, 255));
    paint.anti_alias = true;
    paint
}

fn pen(scale: f32) -> Stroke {
    Stroke {
        width: 4.0 * scale,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// Full redraw. While drawing, the background is black at alpha 1/255: invisible, but
/// it keeps the whole window hit-testable (alpha 0 pixels of a layered window let clicks
/// through). In ghost mode the background is fully transparent.
fn render(o: &mut Overlay, ghost: bool) {
    let stroke = pen(o.scale);
    let mut pixmap = o.surface.pixmap();
    pixmap.fill(if ghost {
        bgra(0, 0, 0, 0)
    } else {
        bgra(0, 0, 0, 1)
    });
    for (swatch, points) in &o.strokes {
        if let Some(path) = polyline(points) {
            pixmap.stroke_path(
                &path,
                &stroke_paint(*swatch),
                &stroke,
                Transform::identity(),
                None,
            );
        }
    }
    present(o);
}

/// Draws only the newest segment of the stroke in progress.
fn render_segment(o: &mut Overlay) {
    let stroke = pen(o.scale);
    let Some((swatch, points)) = o.strokes.last() else {
        return;
    };
    let tail = &points[points.len().saturating_sub(2)..];
    let Some(path) = polyline(tail) else { return };
    let paint = stroke_paint(*swatch);
    o.surface
        .pixmap()
        .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    present(o);
}

fn polyline(points: &[(f32, f32)]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (x, y) = *points.first()?;
    pb.move_to(x, y);
    if points.len() == 1 {
        pb.line_to(x + 0.01, y); // a dot
    }
    for &(x, y) in &points[1..] {
        pb.line_to(x, y);
    }
    pb.finish()
}

fn present(o: &Overlay) {
    let h = o.surface.height - if o.full_height { 0 } else { 1 };
    if !o.surface.present(o.hwnd, o.monitor.left, o.monitor.top, h) {
        log("UpdateLayeredWindow failed");
    }
}

fn render_palette(app: &mut App) {
    let s = app.palette_scale;
    let cell = 40.0 * s;
    let pad = 4.0 * s;
    let (swatch, ghost) = (app.swatch, app.ghost);
    let mut pixmap = app.palette_surface.pixmap();
    pixmap.fill(bgra(0, 0, 0, 0));
    let mut bg = Paint::default();
    bg.set_color(bgra(0x2c, 0x2c, 0x2e, 235));
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    pixmap.fill_rect(
        Rect::from_xywh(0.0, 0.0, w, h).unwrap(),
        &bg,
        Transform::identity(),
        None,
    );
    let mut fill = Paint::default();
    for i in 0..SWATCHES.len() + 2 {
        let x = pad + i as f32 * cell;
        let inset = if i == swatch { 4.0 * s } else { 9.0 * s };
        let color = match i {
            i if i < SWATCHES.len() => {
                let (r, g, b) = SWATCHES[i];
                bgra(r, g, b, 255)
            }
            i if i == SWATCHES.len() => bgra(0x8e, 0x8e, 0x93, 255), // clear
            _ if ghost => bgra(0xaf, 0x52, 0xde, 255),               // ghost on
            _ => bgra(0x48, 0x48, 0x4a, 255),                        // ghost off
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
    app.palette_surface
        .present(app.palette, x, y, app.palette_surface.height);
}

fn on_palette_click(app: &mut App, screen: POINT) {
    let s = app.palette_scale;
    let i = ((screen.x - app.palette_origin.x) as f32 - 4.0 * s) / (40.0 * s);
    if i < 0.0 {
        return;
    }
    match i as usize {
        i if i < SWATCHES.len() => {
            app.swatch = i;
            render_palette(app);
        }
        i if i == SWATCHES.len() => {
            for o in &mut app.overlays {
                o.strokes.clear();
                render(o, app.ghost);
            }
        }
        i if i == SWATCHES.len() + 1 => set_ghost(app, !app.ghost),
        _ => {}
    }
}

// MARK: keys

fn on_key(app: &mut App, index: usize, vk: u16) {
    match vk {
        VK_ESCAPE => hide(app),
        VK_F8 => {
            app.forward = if app.forward == Forward::Post {
                Forward::Input
            } else {
                Forward::Post
            };
            log(&format!("slide key forwarding: {:?}", app.forward));
        }
        VK_F9 => {
            app.hide_palette_from_capture = !app.hide_palette_from_capture;
            let affinity = if app.hide_palette_from_capture {
                WDA_EXCLUDEFROMCAPTURE
            } else {
                WDA_NONE
            };
            let ok = unsafe { SetWindowDisplayAffinity(app.palette, affinity) } != 0;
            log(&format!(
                "palette hidden from capture: {} ({ok})",
                app.hide_palette_from_capture
            ));
        }
        VK_PRIOR | VK_NEXT | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => forward_key(app, index, vk),
        k if k == b'G' as u16 => set_ghost(app, true),
        k if k == b'C' as u16 => {
            for o in &mut app.overlays {
                o.strokes.clear();
                render(o, false);
            }
        }
        k if k == b'Z' as u16 => {
            let o = &mut app.overlays[index];
            o.strokes.pop();
            render(o, false);
        }
        k if (b'1' as u16..=b'5' as u16).contains(&k) => {
            app.swatch = (k - b'1' as u16) as usize;
            render_palette(app);
        }
        _ => {}
    }
}

/// Sends a slide key to the window that was in front before we took focus.
fn forward_key(app: &mut App, index: usize, vk: u16) {
    let target = app.previous;
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) };
    match app.forward {
        Forward::Post => {
            // repeat 1, scan code, extended bit (these are all extended keys)
            let down = 1 | (scan as isize) << 16 | 1 << 24;
            let up = down | 1 << 30 | 1 << 31;
            let ok = unsafe {
                PostMessageW(target, WM_KEYDOWN, vk as WPARAM, down) != 0
                    && PostMessageW(target, WM_KEYUP, vk as WPARAM, up) != 0
            };
            log(&format!(
                "forward vk {vk:#x} by PostMessage to {target:?}: {ok}"
            ));
        }
        Forward::Input => {
            let key = |flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: scan as u16,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let inputs = [
                key(KEYEVENTF_EXTENDEDKEY),
                key(KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
            ];
            unsafe {
                let front = SetForegroundWindow(target) != 0;
                std::thread::sleep(Duration::from_millis(30));
                let sent = SendInput(
                    inputs.len() as u32,
                    inputs.as_ptr(),
                    size_of::<INPUT>() as i32,
                );
                std::thread::sleep(Duration::from_millis(30));
                let back = SetForegroundWindow(app.overlays[index].hwnd) != 0;
                log(&format!(
                    "forward vk {vk:#x} by SendInput: front {front}, sent {sent}, back {back}"
                ));
            }
        }
    }
}

// MARK: window procedure

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let mut handled: Option<LRESULT> = None;
    match msg {
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
            let p = POINT {
                x: (lparam & 0xffff) as i16 as i32,
                y: ((lparam >> 16) & 0xffff) as i16 as i32,
            };
            let first_button = (wparam >> 16) & POINTER_FLAG_FIRSTBUTTON != 0;
            with_app(|app| {
                handled = Some(0);
                if hwnd == app.palette {
                    if msg == WM_POINTERDOWN {
                        on_palette_click(app, p);
                    }
                    return;
                }
                let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) else {
                    return;
                };
                on_pointer(app, index, msg, p, first_button);
            });
        }
        WM_KEYDOWN => with_app(|app| {
            if let Some(index) = app.overlays.iter().position(|o| o.hwnd == hwnd) {
                on_key(app, index, wparam as u16);
                handled = Some(0);
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

fn on_pointer(app: &mut App, index: usize, msg: u32, screen: POINT, first_button: bool) {
    let swatch = app.swatch;
    let o = &mut app.overlays[index];
    let p = (
        (screen.x - o.monitor.left) as f32,
        (screen.y - o.monitor.top) as f32,
    );
    match msg {
        WM_POINTERDOWN if first_button && !app.ghost => {
            app.drawing = Some(index);
            o.strokes.push((swatch, vec![p]));
            render_segment(o);
        }
        WM_POINTERUPDATE if app.drawing == Some(index) => {
            if let Some((_, points)) = o.strokes.last_mut() {
                points.push(p);
            }
            render_segment(o);
        }
        WM_POINTERUP if app.drawing == Some(index) => app.drawing = None,
        _ => {}
    }
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
