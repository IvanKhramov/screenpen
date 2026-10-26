#![cfg_attr(windows, windows_subsystem = "windows")]

mod platform;

fn main() {
    platform::run();
}
