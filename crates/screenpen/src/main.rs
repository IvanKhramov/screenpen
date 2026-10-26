#![cfg_attr(windows, windows_subsystem = "windows")]

// Platform-neutral app logic; only the Windows backend uses it so far.
#[cfg_attr(not(windows), allow(dead_code))]
mod canvas;
mod platform;
#[cfg_attr(not(windows), allow(dead_code))]
mod state;

fn main() {
    platform::run();
}
