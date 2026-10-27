//! Whether the system uses a light theme, for the palette's "system" theme.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Asking the system can mean running a program, so the answer is kept a while.
const KEEP: Duration = Duration::from_secs(10);

pub fn system_is_light() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, light)) = *cache
        && at.elapsed() < KEEP
    {
        return light;
    }
    let light = ask();
    *cache = Some((Instant::now(), light));
    light
}

#[cfg(windows)]
fn ask() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    status == 0 && value != 0
}

#[cfg(target_os = "macos")]
fn ask() -> bool {
    // "Dark" when dark; the key is missing in light mode.
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .is_ok_and(|o| !String::from_utf8_lossy(&o.stdout).contains("Dark"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn ask() -> bool {
    let get = |key: &str| {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
    };
    match get("color-scheme") {
        Some(s) if s.contains("dark") => false,
        Some(s) if s.contains("light") => true,
        // "default": light, unless the GTK theme itself is dark.
        Some(_) => !get("gtk-theme").is_some_and(|t| t.contains("dark")),
        // No GNOME settings: dark suits most overlays.
        None => false,
    }
}
