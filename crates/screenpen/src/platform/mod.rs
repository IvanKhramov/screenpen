#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::run;

#[cfg(not(windows))]
pub fn run() {
    eprintln!("screenpen: only Windows is supported for now (see PLAN.md)");
    std::process::exit(1);
}
