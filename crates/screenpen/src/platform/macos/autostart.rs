//! "Start with macOS": a LaunchAgent in ~/Library/LaunchAgents that runs this binary
//! at login. Works for a bare binary as well as the app bundle.

use std::path::PathBuf;

use super::log;

const LABEL: &str = "dev.screenpen";

fn agent_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
    )
}

fn plist() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe
        .display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Some(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
    ))
}

pub fn enabled() -> bool {
    agent_path().is_some_and(|p| p.exists())
}

pub fn set(on: bool) -> bool {
    let Some(path) = agent_path() else {
        return false;
    };
    let result = if on {
        let Some(text) = plist() else { return false };
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, text))
    } else {
        std::fs::remove_file(&path)
    };
    log(&format!("start with macOS: {on} ({result:?})"));
    result.is_ok()
}

/// A new release usually lands in another folder; keep the agent on the binary that
/// is actually running.
pub fn refresh() {
    let (Some(path), Some(text)) = (agent_path(), plist()) else {
        return;
    };
    if let Ok(current) = std::fs::read_to_string(&path)
        && current != text
    {
        set(true);
    }
}
