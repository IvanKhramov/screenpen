//! One screenpen at a time, and the commands other processes send it, over a Unix
//! socket in $XDG_RUNTIME_DIR. A second copy of the same version tells the running
//! one to say so and exits; a different version is asked to quit and replaced.
//! `screenpen --toggle` (bound to a desktop shortcut on Wayland) and the settings
//! window use the same socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use super::{Msg, log};

/// The release tag in CI builds, the crate version otherwise.
pub const VERSION: &str = match option_env!("SCREENPEN_VERSION") {
    Some(v) => v,
    None => concat!("v", env!("CARGO_PKG_VERSION"), "-dev"),
};

pub fn title() -> String {
    format!("screenpen {VERSION}")
}

/// A request from another process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// A second copy of this version was started.
    Hello,
    Toggle,
    Reload,
    Quit,
}

impl Command {
    fn word(self) -> &'static str {
        match self {
            Command::Hello => "hello",
            Command::Toggle => "toggle",
            Command::Reload => "reload",
            Command::Quit => "quit",
        }
    }

    fn parse(word: &str) -> Option<Command> {
        [
            Command::Hello,
            Command::Toggle,
            Command::Reload,
            Command::Quit,
        ]
        .into_iter()
        .find(|c| c.word() == word)
    }
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // The display keeps sessions of one user on different displays apart.
    let display = std::env::var("WAYLAND_DISPLAY")
        .or_else(|_| std::env::var("DISPLAY"))
        .unwrap_or_default()
        .replace(['/', ':'], "");
    dir.join(format!("screenpen-{display}.sock"))
}

/// Sends `command` to the running copy; returns its title, or None when none runs.
pub fn send(command: Command) -> Option<String> {
    request(command.word())
}

/// The running copy's title. Any unknown word is answered without side effects.
fn running() -> Option<String> {
    request("version")
}

fn request(word: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket_path()).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    writeln!(stream, "{word}").ok()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).ok()?;
    Some(reply.trim().to_string())
}

pub enum Claim {
    /// We own the socket now (unless it couldn't be created). `replaced` is the
    /// title of a copy we shut down.
    Continue {
        listener: Option<UnixListener>,
        replaced: Option<String>,
    },
    /// The same version is already running and has been told so.
    Exit,
}

pub fn claim() -> Claim {
    let mut replaced = None;
    if let Some(other) = running() {
        if other == title() {
            send(Command::Hello);
            return Claim::Exit;
        }
        log(&format!("replacing running copy {other:?}"));
        send(Command::Quit);
        let deadline = Instant::now() + Duration::from_secs(2);
        while running().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        replaced = Some(other);
    }
    let path = socket_path();
    // Nobody answers on it: a crashed copy left it behind.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .inspect_err(|e| log(&format!("socket {} not bound: {e}", path.display())))
        .ok();
    Claim::Continue { listener, replaced }
}

/// Answers every connection with our title and passes its command to the app.
pub fn listen(listener: UnixListener, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let _ = writeln!(&stream, "{}", title());
            if let Some(command) = Command::parse(line.trim())
                && tx.send(Msg::Command(command)).is_err()
            {
                break;
            }
        }
    });
}

/// Removes the socket on quit, so the next start doesn't wait for an answer.
pub fn release() {
    let _ = std::fs::remove_file(socket_path());
}
