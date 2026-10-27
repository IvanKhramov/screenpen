//! Global mouse and keyboard hooks for mouse mode: where clicks land and which keys
//! are pressed, so they can be shown on screen. Installed only while needed; the hooks
//! queue what they see and post a message, the window procedure does the drawing.

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN,
};

use screenpen_core::Chord;

use super::keys::key_from_vk;

/// Posted to the target window when something was seen.
pub const WM_INPUT_SEEN: u32 = WM_APP + 5;

pub enum Seen {
    /// Screen coordinates, and whether it was the right button.
    Click(POINT, bool),
    Keys(Chord),
}

struct Hooks {
    mouse: HHOOK,
    keyboard: HHOOK,
    target: HWND,
    seen: Vec<Seen>,
}

thread_local! {
    static HOOKS: RefCell<Hooks> = const {
        RefCell::new(Hooks {
            mouse: null_mut(),
            keyboard: null_mut(),
            target: null_mut(),
            seen: Vec::new(),
        })
    };
}

/// Installs the hooks (posting to `target`) or removes them.
pub fn set(on: bool, target: HWND) -> bool {
    HOOKS.with_borrow_mut(|h| {
        if on && h.mouse.is_null() {
            h.target = target;
            unsafe {
                let module = GetModuleHandleW(null_mut());
                h.mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), module, 0);
                h.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), module, 0);
            }
        } else if !on && !h.mouse.is_null() {
            unsafe {
                UnhookWindowsHookEx(h.mouse);
                if !h.keyboard.is_null() {
                    UnhookWindowsHookEx(h.keyboard);
                }
            }
            h.mouse = null_mut();
            h.keyboard = null_mut();
            h.seen.clear();
        }
        !h.mouse.is_null()
    })
}

/// What the hooks saw since the last call.
pub fn take() -> Vec<Seen> {
    HOOKS.with_borrow_mut(|h| std::mem::take(&mut h.seen))
}

fn push(seen: Seen) {
    HOOKS.with(|cell| {
        if let Ok(mut h) = cell.try_borrow_mut() {
            if h.seen.is_empty() {
                unsafe { PostMessageW(h.target, WM_INPUT_SEEN, 0, 0) };
            }
            h.seen.push(seen);
        }
    });
}

unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let msg = wparam as u32;
    if code >= 0 && (msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN) {
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        push(Seen::Click(info.pt, msg == WM_RBUTTONDOWN));
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let msg = wparam as u32;
    if code >= 0 && (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) {
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // The app in front gets the key; its modifiers are only known right now.
        let down = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 };
        if let Some(key) = key_from_vk(info.vkCode as u16) {
            push(Seen::Keys(Chord {
                ctrl: down(VK_CONTROL),
                alt: down(VK_MENU),
                shift: down(VK_SHIFT),
                key,
            }));
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}
