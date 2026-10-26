//! One screenpen at a time, across versions. The running copy holds an exclusive lock
//! on a file that names its pid and version. A second copy of the same version exits;
//! a different version (an update) asks the running one to quit and takes over.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use objc2_app_kit::NSRunningApplication;

use super::log;

/// The release tag in CI builds, the crate version otherwise.
pub const VERSION: &str = match option_env!("SCREENPEN_VERSION") {
    Some(v) => v,
    None => concat!("v", env!("CARGO_PKG_VERSION"), "-dev"),
};

pub fn title() -> String {
    format!("screenpen {VERSION}")
}

pub enum Claim {
    /// We hold the lock. `replaced` is the title of a copy we shut down.
    /// Keep `lock` open for as long as the app runs.
    Continue {
        lock: Option<File>,
        replaced: Option<String>,
    },
    /// The same version is already running.
    Exit,
}

pub fn claim(path: &Path) -> Claim {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(mut file) = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    else {
        log("instance lock file can't be opened; starting anyway");
        return Claim::Continue {
            lock: None,
            replaced: None,
        };
    };
    let mut replaced = None;
    if let Err(TryLockError::WouldBlock) = file.try_lock() {
        let mut text = String::new();
        let _ = file.read_to_string(&mut text);
        let mut lines = text.lines();
        let pid = lines.next().and_then(|p| p.trim().parse::<i32>().ok());
        let version = lines.next().unwrap_or("").trim().to_string();
        if version == VERSION {
            log("the same version is already running");
            return Claim::Exit;
        }
        log(&format!("replacing running copy {version:?} (pid {pid:?})"));
        if let Some(other) =
            pid.and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
        {
            other.terminate();
            if !wait_for_lock(&file, Duration::from_millis(1500)) {
                // Not an app that handles the quit request: stop it.
                other.forceTerminate();
                wait_for_lock(&file, Duration::from_millis(2000));
            }
        }
        replaced = Some(format!("screenpen {version}"));
    }
    // Locked by now unless the old copy refused to die; carry on either way.
    let _ = file.try_lock();
    let _ = file.set_len(0);
    let _ = file.rewind();
    let _ = write!(file, "{}\n{VERSION}\n", std::process::id());
    let _ = file.flush();
    Claim::Continue {
        lock: Some(file),
        replaced,
    }
}

fn wait_for_lock(file: &File, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        match file.try_lock() {
            Ok(()) => return true,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    false
}
