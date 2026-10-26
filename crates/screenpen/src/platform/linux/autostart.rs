//! "Start with the session": an XDG autostart entry pointing at this binary.

use std::path::PathBuf;

use super::log;

/// The desktop entry shipped next to the binary; autostart runs it with the full path.
pub const DESKTOP_ENTRY: &str = include_str!("screenpen.desktop");

fn entry_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join("screenpen.desktop"))
}

fn entry() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let exec = format!("Exec=\"{}\"", exe.display());
    Some(
        DESKTOP_ENTRY
            .lines()
            .map(|l| {
                if l.starts_with("Exec=") {
                    exec.as_str()
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
}

pub fn enabled() -> bool {
    entry_path().is_some_and(|p| p.exists())
}

pub fn set(on: bool) -> bool {
    let Some(path) = entry_path() else {
        return false;
    };
    let result = if on {
        let Some(entry) = entry() else { return false };
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, entry))
    } else {
        std::fs::remove_file(&path)
    };
    log(&format!("start with the session: {on} ({result:?})"));
    result.is_ok()
}

/// A new release usually lands in another folder; keep the entry on the binary
/// that is actually running.
pub fn refresh() {
    let (Some(path), Some(entry)) = (entry_path(), entry()) else {
        return;
    };
    if std::fs::read_to_string(&path).is_ok_and(|current| current != entry) {
        set(true);
    }
}
