//! "Start with Windows": a value under the user's Run key pointing at this exe.

use std::ptr::null_mut;

use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

use super::{log, wide};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE: &str = "screenpen";

fn command() -> Option<Vec<u16>> {
    let exe = std::env::current_exe().ok()?;
    Some(wide(&format!("\"{}\"", exe.display())))
}

/// The command currently registered, if any.
fn registered() -> Option<String> {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    (status == 0).then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
}

pub fn enabled() -> bool {
    registered().is_some()
}

pub fn set(on: bool) -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let status = unsafe {
        if on {
            let Some(data) = command() else { return false };
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr())
        }
    };
    log(&format!("start with Windows: {on} (status {status})"));
    status == 0
}

/// A new release usually lands in another folder; keep the Run entry on the exe
/// that is actually running.
pub fn refresh() {
    let (Some(current), Some(command)) = (registered(), command()) else {
        return;
    };
    let command = String::from_utf16_lossy(&command[..command.len() - 1]);
    if current != command {
        set(true);
    }
}
