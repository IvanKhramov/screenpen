//! What the app needs from a display server. X11 and Wayland implement it; the app
//! logic in `mod.rs` only sees screen pixels, window roles and input events.

use screenpen_core::{Action, Chord, Key};
use tiny_skia::{IntRect, Pixmap, PixmapRef};

use crate::canvas::Mods;

/// A rectangle in global screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub area: Area,
    /// Pixels per point.
    pub scale: f32,
}

/// Our windows, by role. The backend decides what each role needs (input, focus).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Win {
    /// The drawing surface covering monitor `n` of the last `set_overlays`.
    Overlay(usize),
    Palette,
    /// Never takes input.
    Tooltip,
    Sheet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Cross,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// 1 left, 2 middle, 3 right (X11 numbering).
    Down(u8),
    Move,
    Up(u8),
    Leave,
    /// The pointer was taken away mid-gesture (another client grabbed it).
    Lost,
    /// Wheel step; true is up (away from the user).
    Wheel(bool),
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// Position in global screen pixels.
    Pointer {
        win: Win,
        kind: PointerKind,
        at: (i32, i32),
        mods: Mods,
    },
    /// A key press. `win` is None for a global hotkey caught outside our windows.
    Key {
        win: Option<Win>,
        key: Option<Key>,
        mods: Mods,
        /// The character the active layout types, for text input.
        text: Option<char>,
    },
    MonitorsChanged,
    /// A click in another app, seen while watching input (global pixels).
    SeenClick {
        at: (i32, i32),
        right: bool,
    },
    /// A key pressed in another app, with the pointer's position then.
    SeenKeys {
        at: (i32, i32),
        chord: Chord,
    },
}

pub trait Display {
    /// The connected monitors; the first is the primary one.
    fn monitors(&mut self) -> Vec<Monitor>;
    /// Gives overlay `n` the area of `monitors[n]`, dropping windows beyond.
    fn set_overlays(&mut self, monitors: &[Monitor]);
    /// Puts `pixels` (premultiplied BGRA) in `win` with its top-left at `at`, resizing
    /// it to the pixmap. `dirty` limits the upload to the part that changed.
    fn present(&mut self, win: Win, at: (i32, i32), pixels: PixmapRef, dirty: Option<IntRect>);
    /// Maps `win` above everything else.
    fn show(&mut self, win: Win);
    fn hide(&mut self, win: Win);
    /// Ghost mode: overlays let pointer input through to the windows below.
    fn set_click_through(&mut self, on: bool);
    /// Keys go to overlay `overlay` (true) or back to other apps (false).
    fn set_keyboard(&mut self, grab: bool, overlay: usize);
    fn set_cursor(&mut self, cursor: Cursor);
    /// Binds the global shortcuts; returns the chords another app already holds.
    fn grab_hotkeys(&mut self, chords: &[(Action, Chord)]) -> Vec<String>;
    /// The screen inside each area without our windows, as opaque BGRA pixels.
    fn capture(&mut self, areas: &[Area]) -> Vec<Option<Pixmap>>;
    /// Offers a PNG on the clipboard.
    fn copy_png(&mut self, png: Vec<u8>) -> bool;
    /// Sends a slide key (PageUp, arrows) to the app behind the overlay.
    fn forward_key(&mut self, key: Key);
    fn pointer_position(&mut self) -> Option<(i32, i32)>;
    /// Turns a message from the backend's reader thread into events.
    fn handle(&mut self, msg: super::Msg) -> Vec<Event>;
    /// Sends buffered requests to the server.
    fn flush(&mut self);
    /// Global hotkeys work (they don't on Wayland).
    fn has_hotkeys(&self) -> bool;
    /// Reports clicks and keys in other apps as `Seen*` events, or stops. Returns
    /// whether it's watching.
    fn watch_input(&mut self, on: bool) -> bool;
}
