#![cfg_attr(windows, windows_subsystem = "windows")]

// Platform-neutral app logic, shared by the platform backends.
#[cfg_attr(
    not(any(windows, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
mod canvas;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod clipboard;
#[cfg_attr(
    not(any(windows, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
mod config;
#[cfg_attr(
    not(any(windows, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
mod i18n;
#[cfg_attr(
    not(any(windows, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
mod palette;
mod platform;
mod settings_ui;
#[cfg_attr(
    not(any(windows, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
mod snapshots;
#[cfg_attr(
    not(any(windows, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
mod state;

fn main() {
    if std::env::args().any(|a| a == "--settings") {
        settings_ui::run();
        return;
    }
    // `--restore <snapshot>`: put a saved snapshot back in the running screenpen.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--restore") {
        match args.get(i + 1) {
            Some(stem) => match snapshots::request_restore(stem.trim_end_matches(".png")) {
                Ok(()) => platform::notify_settings_changed(),
                Err(e) => eprintln!("screenpen: {e}"),
            },
            None => eprintln!("screenpen: --restore needs a snapshot name"),
        }
        return;
    }
    if std::env::args().any(|a| a == "--toggle") {
        platform::toggle();
        return;
    }
    platform::run();
}
