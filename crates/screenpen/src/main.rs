#![cfg_attr(windows, windows_subsystem = "windows")]

// Platform-neutral app logic, used by the Windows and Linux backends.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod canvas;
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod config;
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod i18n;
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod palette;
mod platform;
mod settings_ui;
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod state;

fn main() {
    if std::env::args().any(|a| a == "--settings") {
        settings_ui::run();
        return;
    }
    if std::env::args().any(|a| a == "--toggle") {
        platform::toggle();
        return;
    }
    platform::run();
}
