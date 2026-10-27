//! Plain-text clipboard for the text tool.

use std::sync::Mutex;

use screenpen_core::Chord;

use crate::canvas::{Canvas, Repaint, TextKey};

/// One clipboard for the whole run: on Linux, copied text is served by it and
/// would vanish with it.
static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Option<T>) -> Option<T> {
    let mut guard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = arboard::Clipboard::new()
            .map_err(|e| eprintln!("clipboard unavailable: {e}"))
            .ok();
    }
    f(guard.as_mut()?)
}

pub fn set_text(s: String) {
    with(|c| {
        c.set_text(s)
            .map_err(|e| eprintln!("copy failed: {e}"))
            .ok()
    });
}

pub fn text() -> Option<String> {
    with(|c| c.get_text().ok())
}

/// Runs an editing key on the text being typed, with the clipboard behind
/// copy, cut and paste. None when the key isn't an editing key.
pub fn text_key(canvas: &mut Canvas, chord: Chord) -> Option<Repaint> {
    Some(match canvas.text_key(chord)? {
        TextKey::Handled(r) => r,
        TextKey::Copy(s) => {
            set_text(s);
            Repaint::None
        }
        TextKey::Paste => text().map_or(Repaint::None, |s| canvas.paste(&s)),
    })
}
