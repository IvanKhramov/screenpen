//! One screenpen at a time, across versions. A second copy of the same version tells
//! the running one to show a notification and exits. A different version (an update,
//! or an old build that predates this check) is shut down and replaced.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowTextW, GetWindowThreadProcessId, PostMessageW,
};

use super::tray::{WM_QUIT_REQUEST, WM_SECOND_INSTANCE};
use super::{OVERLAY_CLASS, PALETTE_CLASS, TRAY_CLASS, log, wide};

/// The release tag in CI builds, the crate version otherwise.
pub const VERSION: &str = match option_env!("SCREENPEN_VERSION") {
    Some(v) => v,
    None => concat!("v", env!("CARGO_PKG_VERSION"), "-dev"),
};

/// The tray window's title; a running copy is identified by it.
pub fn title() -> String {
    format!("screenpen {VERSION}")
}

pub enum Claim {
    /// Nothing else is running (any more). `replaced` is the title of a copy we shut down.
    Continue { replaced: Option<String> },
    /// The same version is already running and has been told so.
    Exit,
}

/// Any window of a running copy. Old builds have no tray window but share the classes.
fn find_running() -> HWND {
    for class in [TRAY_CLASS, OVERLAY_CLASS, PALETTE_CLASS] {
        let hwnd = unsafe { FindWindowW(wide(class).as_ptr(), null()) };
        if !hwnd.is_null() {
            return hwnd;
        }
    }
    null_mut()
}

fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn claim() -> Claim {
    let name = wide("Local\\screenpen");
    let mutex_existed = unsafe {
        CreateMutexW(null(), 0, name.as_ptr());
        GetLastError() == ERROR_ALREADY_EXISTS
    };
    let running = find_running();
    if running.is_null() {
        if mutex_existed {
            log("mutex held but no windows found; starting anyway");
        }
        return Claim::Continue { replaced: None };
    }
    let other = window_text(running);
    if other == title() {
        unsafe { PostMessageW(running, WM_SECOND_INSTANCE, 0, 0) };
        return Claim::Exit;
    }
    log(&format!("replacing running copy {other:?}"));
    stop(running);
    Claim::Continue {
        replaced: Some(other),
    }
}

/// `screenpen --quit`: stops a running copy (the installer runs this before it
/// replaces or removes the exe). Returns whether one was running.
pub fn quit_running() -> bool {
    let running = find_running();
    if running.is_null() {
        return false;
    }
    stop(running);
    true
}

/// Asks the copy owning `running` to quit, and ends it if it doesn't.
fn stop(running: HWND) {
    let tray = unsafe { FindWindowW(wide(TRAY_CLASS).as_ptr(), null()) };
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(running, &mut pid);
        let process = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
        if !tray.is_null() {
            // Builds with this check quit cleanly on request.
            PostMessageW(tray, WM_QUIT_REQUEST, 0, 0);
        }
        if !process.is_null() {
            if WaitForSingleObject(process, 1500) != 0 {
                // Older builds don't know the request.
                TerminateProcess(process, 0);
                WaitForSingleObject(process, 2000);
                // A killed process leaves its tray icon behind until the mouse passes over it.
                if !tray.is_null() {
                    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
                    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
                    data.hWnd = tray;
                    data.uID = 1;
                    Shell_NotifyIconW(NIM_DELETE, &data);
                }
            }
            CloseHandle(process);
        }
    }
}
