//! Notification-area icon: shows that screenpen is running and in which mode, toggles
//! drawing on left click and opens the menu on right click.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use tiny_skia::{FillRule, Paint, PathBuilder, PixmapMut, Stroke, Transform};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    HICON, ICONINFO, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
    WM_NULL,
};

use super::surface::bgra;
use crate::i18n::{Lang, T};
use crate::state::{DIM_LEVELS, FADE_SECONDS};

/// Sent by the shell for clicks on the icon; `lParam` holds the mouse message.
pub const WM_TRAY: u32 = WM_APP + 1;
/// Posted by a second instance so the running one can say it's already there.
pub const WM_SECOND_INSTANCE: u32 = WM_APP + 2;
/// Posted by a newer version that is taking over: quit.
pub const WM_QUIT_REQUEST: u32 = WM_APP + 3;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Idle,
    Drawing,
    Ghost,
}

#[derive(Clone, Copy)]
pub enum Command {
    ToggleDrawing,
    ToggleGhost,
    ToggleWhiteboard,
    ToggleCaptureHiding,
    SetDim(usize),
    /// Index into `FADE_SECONDS`, or off.
    SetFade(Option<usize>),
    Settings,
    Quit,
}

/// What the menu shows checked.
pub struct MenuState {
    pub lang: Lang,
    pub version: String,
    pub mode: Mode,
    pub whiteboard: bool,
    pub hide_from_capture: bool,
    pub dim: usize,
    pub fade: Option<usize>,
    pub toggle_key: String,
    pub quit_key: String,
}

pub struct Tray {
    hwnd: HWND,
    icons: [HICON; 3],
    mode: Mode,
    lang: Lang,
    /// Chords shown in the tooltip: start drawing, leave ghost mode.
    keys: (String, String),
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let icons = [Mode::Idle, Mode::Drawing, Mode::Ghost].map(make_icon);
        let tray = Tray {
            hwnd,
            icons,
            mode: Mode::Idle,
            lang: Lang::En,
            keys: Default::default(),
        };
        tray.add();
        tray
    }

    /// Adds the icon. Called again when Explorer restarts ("TaskbarCreated").
    pub fn add(&self) -> bool {
        let mut data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP);
        data.uCallbackMessage = WM_TRAY;
        unsafe { Shell_NotifyIconW(NIM_ADD, &data) != 0 }
    }

    pub fn set_text(&mut self, lang: Lang, keys: (String, String)) {
        self.lang = lang;
        self.keys = keys;
        let data = self.data(NIF_TIP);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            let data = self.data(NIF_ICON | NIF_TIP);
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        }
    }

    pub fn notify(&self, title: &str, text: &str) {
        let mut data = self.data(NIF_INFO);
        copy(&mut data.szInfoTitle, title);
        copy(&mut data.szInfo, text);
        data.dwInfoFlags = NIIF_INFO;
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    /// Shows the context menu at the cursor and returns the picked command.
    /// Must be called with the app state released: the menu runs a modal loop.
    pub fn menu(hwnd: HWND, m: &MenuState) -> Option<Command> {
        let check = |on: bool| if on { MF_CHECKED } else { 0 };
        unsafe {
            let add = |menu, flags, id: usize, text: &str| {
                let text = super::wide(text);
                AppendMenuW(menu, flags, id, text.as_ptr());
            };
            let menu = CreatePopupMenu();
            add(menu, MF_STRING | MF_GRAYED, 0, &m.version);
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            let l = m.lang;
            add(
                menu,
                MF_STRING | check(m.mode != Mode::Idle),
                1,
                &format!("{}\t{}", l.t(T::Draw), m.toggle_key),
            );
            let ghost_flags = if m.mode == Mode::Idle { MF_GRAYED } else { 0 };
            add(
                menu,
                MF_STRING | ghost_flags | check(m.mode == Mode::Ghost),
                2,
                l.t(T::GhostMenu),
            );
            add(menu, MF_STRING | check(m.whiteboard), 3, l.t(T::Whiteboard));
            AppendMenuW(menu, MF_SEPARATOR, 0, null());

            let dim = CreatePopupMenu();
            let dim_names = [T::DimOff, T::DimSlightly, T::DimMedium, T::DimStrongly];
            for (i, name) in dim_names.iter().enumerate().take(DIM_LEVELS.len()) {
                add(dim, MF_STRING | check(m.dim == i), 100 + i, l.t(*name));
            }
            add(menu, MF_POPUP, dim as usize, l.t(T::DimMenu));
            let fade = CreatePopupMenu();
            add(
                fade,
                MF_STRING | check(m.fade.is_none()),
                200,
                l.t(T::FadeOff),
            );
            for (i, seconds) in FADE_SECONDS.iter().enumerate() {
                let text = l.t(T::FadeAfter).replace("{n}", &seconds.to_string());
                add(fade, MF_STRING | check(m.fade == Some(i)), 201 + i, &text);
            }
            add(menu, MF_POPUP, fade as usize, l.t(T::FadeMenu));
            add(
                menu,
                MF_STRING | check(m.hide_from_capture),
                4,
                l.t(T::HidePalette),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            add(menu, MF_STRING, 6, l.t(T::Settings));
            add(
                menu,
                MF_STRING,
                5,
                &format!("{}\t{}", l.t(T::Quit), m.quit_key),
            );

            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            // Without this the menu doesn't close when you click elsewhere.
            SetForegroundWindow(hwnd);
            let picked = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                p.x,
                p.y,
                0,
                hwnd,
                null(),
            );
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu); // destroys the submenus too
            match picked as usize {
                1 => Some(Command::ToggleDrawing),
                2 => Some(Command::ToggleGhost),
                3 => Some(Command::ToggleWhiteboard),
                4 => Some(Command::ToggleCaptureHiding),
                5 => Some(Command::Quit),
                6 => Some(Command::Settings),
                n @ 100..=199 => Some(Command::SetDim(n - 100)),
                200 => Some(Command::SetFade(None)),
                n @ 201..=299 => Some(Command::SetFade(Some(n - 201))),
                _ => None,
            }
        }
    }

    fn data(&self, flags: u32) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = flags;
        data.hIcon = self.icons[self.mode as usize];
        let tip = match self.mode {
            Mode::Idle => self.lang.t(T::TipIdle).replace("{key}", &self.keys.0),
            Mode::Drawing => self.lang.t(T::TipDrawing).to_string(),
            Mode::Ghost => self.lang.t(T::TipGhost).replace("{key}", &self.keys.1),
        };
        copy(&mut data.szTip, &tip);
        data
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = self.data(0);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
            for icon in self.icons {
                DestroyIcon(icon);
            }
        }
    }
}

fn copy<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (d, c) in dst
        .iter_mut()
        .zip(s.encode_utf16().take(N - 1).chain(Some(0)))
    {
        *d = c;
    }
}

/// A round badge in the mode's color with a pen stroke across it, drawn at 32x32;
/// the shell scales it down for the tray.
fn make_icon(mode: Mode) -> HICON {
    const SIZE: u32 = 32;
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE as i32,
            biHeight: -(SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut c_void = null_mut();
        let color: HBITMAP =
            CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        let bytes = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (SIZE * SIZE * 4) as usize);
        let mut pixmap = PixmapMut::from_bytes(bytes, SIZE, SIZE).expect("icon size");
        draw_icon(&mut pixmap, mode);

        let mask = CreateBitmap(SIZE as i32, SIZE as i32, 1, 1, null()); // unused with alpha
        let icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}

fn draw_icon(pixmap: &mut PixmapMut, mode: Mode) {
    let (r, g, b) = match mode {
        Mode::Idle => (0xff, 0xcc, 0x00),
        Mode::Drawing => (0xff, 0x3b, 0x30),
        Mode::Ghost => (0xaf, 0x52, 0xde),
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(bgra(r, g, b, 255));
    if let Some(circle) = PathBuilder::from_circle(16.0, 16.0, 15.0) {
        pixmap.fill_path(
            &circle,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mut pb = PathBuilder::new();
    pb.move_to(9.0, 23.0);
    pb.line_to(23.0, 9.0);
    if let Some(line) = pb.finish() {
        paint.set_color(bgra(0x1c, 0x1c, 0x1e, 255));
        let stroke = Stroke {
            width: 4.5,
            line_cap: tiny_skia::LineCap::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&line, &paint, &stroke, Transform::identity(), None);
    }
}
