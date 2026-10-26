#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{notify_settings_changed, run, system_lang};

#[cfg(not(windows))]
pub fn run() {
    eprintln!("screenpen: only Windows is supported for now (see PLAN.md)");
    std::process::exit(1);
}

/// The UI language from the environment, for platforms without a native lookup yet.
#[cfg(not(windows))]
pub fn system_lang() -> crate::i18n::Lang {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if lang.starts_with("ru") {
        crate::i18n::Lang::Ru
    } else {
        crate::i18n::Lang::En
    }
}

/// Tells a running screenpen to reload its settings file. Elsewhere it reloads
/// the file the next time drawing starts.
#[cfg(not(windows))]
pub fn notify_settings_changed() {}
