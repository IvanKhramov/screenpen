#![cfg_attr(windows, windows_subsystem = "windows")]

// Platform-neutral app logic; only the Windows backend uses it so far.
#[cfg_attr(not(windows), allow(dead_code))]
mod canvas;
#[cfg_attr(not(windows), allow(dead_code))]
mod config;
#[cfg_attr(not(windows), allow(dead_code))]
mod i18n;
#[cfg_attr(not(windows), allow(dead_code))]
mod palette;
mod platform;
#[cfg_attr(not(windows), allow(dead_code))]
mod state;

fn main() {
    platform::run();
}
