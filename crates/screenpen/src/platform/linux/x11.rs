//! X11 backend (also used under XWayland): override-redirect 32-bit ARGB windows,
//! one per RandR monitor. Pixels go into a server-side pixmap set as the window
//! background, so the server repaints exposures itself. Ghost mode empties the XShape
//! input region; drawing grabs the keyboard so single-letter shortcuts work; global
//! shortcuts are passive key grabs on the root window.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use screenpen_core::{Action, Chord, Key};
use tiny_skia::{IntRect, Pixmap, PixmapRef};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyOrIdError};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ButtonPressEvent, ChangeWindowAttributesAux, ClipOrdering, ConfigureWindowAux,
    ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, GrabMode, GrabStatus, ImageFormat,
    KeyPressEvent, ModMask, PropMode, SelectionNotifyEvent, SelectionRequestEvent, StackMode,
    VisualClass, Window, WindowClass,
};
use x11rb::protocol::{Event as XEvent, randr, shape, xkb, xtest};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use super::display::{Area, Cursor, Display, Event, Monitor, PointerKind, Win};
use super::keys::{code_from_key, key_from_code};
use super::{Msg, log};
use crate::canvas::Mods;

/// X keycodes are evdev codes shifted by 8.
const EVDEV_OFFSET: u8 = 8;
/// Glyphs of the core cursor font: left_ptr, crosshair, xterm.
const CURSOR_GLYPHS: [u16; 3] = [68, 34, 152];

struct Native {
    id: Window,
    /// The window background, holding what was last presented.
    pixmap: u32,
    gc: u32,
    size: (u16, u16),
    at: (i32, i32),
    mapped: bool,
}

/// The core keyboard mapping, for the characters typed into text annotations.
struct Keymap {
    min: u8,
    per: usize,
    syms: Vec<u32>,
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    png: Atom,
}

pub struct X11 {
    conn: Arc<RustConnection>,
    root: Window,
    root_depth: u8,
    visual: u32,
    colormap: u32,
    windows: HashMap<Win, Native>,
    overlays: Vec<Area>,
    keymap: Keymap,
    /// Passive grabs on the root window: keycode and modifiers.
    hotkeys: Vec<(u8, u16)>,
    cursors: [u32; 3],
    cursor: Cursor,
    click_through: bool,
    /// The overlay holding the keyboard grab.
    grab: Option<usize>,
    /// Owns the clipboard selection.
    helper: Window,
    atoms: Atoms,
    clipboard: Option<Vec<u8>>,
    composited: bool,
}

fn mods(state: u16) -> Mods {
    Mods {
        shift: state & u16::from(ModMask::SHIFT) != 0,
        ctrl: state & u16::from(ModMask::CONTROL) != 0,
        alt: state & u16::from(ModMask::M1) != 0,
    }
}

impl X11 {
    pub fn connect(tx: Sender<Msg>) -> Result<X11, String> {
        let (conn, screen_num) = RustConnection::connect(None).map_err(|e| e.to_string())?;
        let conn = Arc::new(conn);
        let x = X11::setup(&conn, screen_num).map_err(|e| e.to_string())?;
        let reader = conn.clone();
        std::thread::spawn(move || {
            loop {
                let msg = match reader.wait_for_event() {
                    Ok(event) => Msg::X11(event),
                    Err(e) => {
                        log(&format!("X connection lost: {e}"));
                        Msg::Disconnected
                    }
                };
                let last = matches!(msg, Msg::Disconnected);
                if tx.send(msg).is_err() || last {
                    break;
                }
            }
        });
        Ok(x)
    }

    fn setup(conn: &Arc<RustConnection>, screen_num: usize) -> Result<X11, ReplyOrIdError> {
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
            .ok_or(ConnectionError::UnknownError)?;
        let colormap = conn.generate_id()?;
        conn.create_colormap(
            x11rb::protocol::xproto::ColormapAlloc::NONE,
            colormap,
            root,
            visual,
        )?;

        // Group (layout) bits in key event state need an XKB-aware client.
        if conn
            .extension_information(xkb::X11_EXTENSION_NAME)?
            .is_some()
        {
            xkb::use_extension(conn.as_ref(), 1, 0)?.reply()?;
        }
        randr::select_input(conn.as_ref(), root, randr::NotifyMask::SCREEN_CHANGE)?;

        let font = conn.generate_id()?;
        conn.open_font(font, b"cursor")?;
        let mut cursors = [0; 3];
        for (c, glyph) in cursors.iter_mut().zip(CURSOR_GLYPHS) {
            *c = conn.generate_id()?;
            conn.create_glyph_cursor(
                *c,
                font,
                font,
                glyph,
                glyph + 1,
                0,
                0,
                0,
                0xffff,
                0xffff,
                0xffff,
            )?;
        }
        conn.close_font(font)?;

        let helper = conn.generate_id()?;
        conn.create_window(
            0,
            helper,
            root,
            -10,
            -10,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().override_redirect(1),
        )?;
        let atom = |name: &str| -> Result<Atom, ReplyOrIdError> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let atoms = Atoms {
            clipboard: atom("CLIPBOARD")?,
            targets: atom("TARGETS")?,
            png: atom("image/png")?,
        };
        let cm = atom(&format!("_NET_WM_CM_S{screen_num}"))?;
        let composited = conn.get_selection_owner(cm)?.reply()?.owner != NONE;
        if !composited {
            log(
                "no compositing manager: the overlay can't be see-through, so drawing \
                 covers the screen (dimming and the whiteboard still work)",
            );
        }
        let mut x = X11 {
            conn: conn.clone(),
            root,
            root_depth: screen.root_depth,
            visual,
            colormap,
            windows: HashMap::new(),
            overlays: Vec::new(),
            keymap: Keymap {
                min: 8,
                per: 0,
                syms: Vec::new(),
            },
            hotkeys: Vec::new(),
            cursors,
            cursor: Cursor::Cross,
            click_through: false,
            grab: None,
            helper,
            atoms,
            clipboard: None,
            composited,
        };
        x.load_keymap()?;
        conn.flush()?;
        Ok(x)
    }

    fn load_keymap(&mut self) -> Result<(), ReplyOrIdError> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)?
            .reply()?;
        self.keymap = Keymap {
            min,
            per: map.keysyms_per_keycode as usize,
            syms: map.keysyms,
        };
        Ok(())
    }

    /// The character a key types with the given state: group from bits 13-14,
    /// Shift picks the level, Caps Lock flips letters.
    fn text(&self, keycode: u8, state: u16) -> Option<char> {
        let k = &self.keymap;
        let start = keycode.checked_sub(k.min)? as usize * k.per;
        let syms = k.syms.get(start..start + k.per)?;
        let shift = state & u16::from(ModMask::SHIFT) != 0;
        let lock = state & u16::from(ModMask::LOCK) != 0;
        let group = ((state >> 13) & 3) as usize * 2;
        let pick = |i: usize| syms.get(i).copied().filter(|&s| s != 0);
        let sym = pick(group + shift as usize)
            .or_else(|| pick(group))
            .or_else(|| pick(shift as usize))
            .or_else(|| pick(0))?;
        let c = xkeysym::Keysym::new(sym).key_char()?;
        if c.is_control() {
            return None;
        }
        Some(match (lock && !shift, lock && shift) {
            (true, _) => c.to_uppercase().next().unwrap_or(c),
            (_, true) => c.to_lowercase().next().unwrap_or(c),
            _ => c,
        })
    }

    fn win_of(&self, id: Window) -> Option<Win> {
        self.windows
            .iter()
            .find(|(_, n)| n.id == id)
            .map(|(w, _)| *w)
    }

    fn create(&mut self, win: Win, at: (i32, i32), size: (u16, u16)) -> Result<(), ReplyOrIdError> {
        let c = &self.conn;
        let (id, pixmap, gc) = (c.generate_id()?, c.generate_id()?, c.generate_id()?);
        c.create_pixmap(32, pixmap, self.root, size.0, size.1)?;
        c.create_gc(gc, pixmap, &CreateGCAux::new().graphics_exposures(0))?;
        let mut events = EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::LEAVE_WINDOW;
        if matches!(win, Win::Overlay(_)) {
            events |= EventMask::KEY_PRESS;
        }
        let mut aux = CreateWindowAux::new()
            .background_pixmap(pixmap)
            .border_pixel(0)
            .colormap(self.colormap)
            .override_redirect(1)
            .event_mask(events);
        if matches!(win, Win::Overlay(_)) {
            aux = aux.cursor(self.cursors[self.cursor as usize]);
        }
        c.create_window(
            32,
            id,
            self.root,
            at.0 as i16,
            at.1 as i16,
            size.0,
            size.1,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &aux,
        )?;
        let name = format!("screenpen {win:?}");
        c.change_property8(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            name.as_bytes(),
        )?;
        c.change_property8(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"screenpen\0screenpen\0",
        )?;
        self.windows.insert(
            win,
            Native {
                id,
                pixmap,
                gc,
                size,
                at,
                mapped: false,
            },
        );
        match win {
            Win::Tooltip => self.set_input(win, false),
            Win::Overlay(_) if self.click_through => self.set_input(win, false),
            _ => {}
        }
        Ok(())
    }

    /// A new background pixmap of the new size; the old content is replaced next.
    fn resize(&mut self, win: Win, size: (u16, u16)) -> Result<(), ReplyOrIdError> {
        let c = &self.conn;
        let Some(n) = self.windows.get_mut(&win) else {
            return Ok(());
        };
        let pixmap = c.generate_id()?;
        c.create_pixmap(32, pixmap, self.root, size.0, size.1)?;
        let aux = ChangeWindowAttributesAux::new().background_pixmap(pixmap);
        c.change_window_attributes(n.id, &aux)?;
        let aux = ConfigureWindowAux::new()
            .width(size.0 as u32)
            .height(size.1 as u32);
        c.configure_window(n.id, &aux)?;
        c.free_pixmap(n.pixmap)?;
        n.pixmap = pixmap;
        n.size = size;
        Ok(())
    }

    fn destroy(&mut self, win: Win) {
        if let Some(n) = self.windows.remove(&win) {
            let _ = self.conn.destroy_window(n.id);
            let _ = self.conn.free_pixmap(n.pixmap);
            let _ = self.conn.free_gc(n.gc);
        }
    }

    /// Whether the window takes pointer input (an empty input shape lets it through).
    fn set_input(&self, win: Win, on: bool) {
        let Some(n) = self.windows.get(&win) else {
            return;
        };
        let _ = if on {
            shape::mask(
                self.conn.as_ref(),
                shape::SO::SET,
                shape::SK::INPUT,
                n.id,
                0,
                0,
                NONE,
            )
        } else {
            shape::rectangles(
                self.conn.as_ref(),
                shape::SO::SET,
                shape::SK::INPUT,
                ClipOrdering::UNSORTED,
                n.id,
                0,
                0,
                &[],
            )
        };
    }

    fn upload(&self, n: &Native, pixels: PixmapRef, r: IntRect) -> Result<(), ConnectionError> {
        let stride = pixels.width() as usize * 4;
        let row_bytes = r.width() as usize * 4;
        let max = self.conn.maximum_request_bytes().saturating_sub(64);
        let rows_per = (max / row_bytes).max(1) as u32;
        let data = pixels.data();
        let mut y = r.top() as u32;
        while y < r.bottom() as u32 {
            let rows = rows_per.min(r.bottom() as u32 - y);
            let mut chunk = Vec::with_capacity(rows as usize * row_bytes);
            for row in y..y + rows {
                let start = row as usize * stride + r.left() as usize * 4;
                chunk.extend_from_slice(&data[start..start + row_bytes]);
            }
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                n.pixmap,
                n.gc,
                r.width() as u16,
                rows as u16,
                r.left() as i16,
                y as i16,
                0,
                32,
                &chunk,
            )?;
            y += rows;
        }
        // Repaint the window from its background pixmap.
        self.conn.clear_area(
            false,
            n.id,
            r.left() as i16,
            r.top() as i16,
            r.width() as u16,
            r.height() as u16,
        )?;
        Ok(())
    }

    fn grab_keyboard(&self, overlay: usize) -> bool {
        let Some(n) = self.windows.get(&Win::Overlay(overlay)) else {
            return false;
        };
        // Right after a tray click the panel may still hold the keyboard.
        for _ in 0..20 {
            let status = self
                .conn
                .grab_keyboard(false, n.id, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.status);
            if status == Some(GrabStatus::SUCCESS) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        log("keyboard not grabbed: shortcuts won't work until drawing restarts");
        false
    }

    fn sync(&self) {
        let _ = self.conn.get_input_focus().map(|c| c.reply());
    }

    fn pointer_event(&self, e: &ButtonPressEvent, kind: PointerKind) -> Option<Event> {
        Some(Event::Pointer {
            win: self.win_of(e.event)?,
            kind,
            at: (e.root_x as i32, e.root_y as i32),
            mods: mods(u16::from(e.state)),
        })
    }

    fn key_event(&self, e: &KeyPressEvent) -> Event {
        let state = u16::from(e.state);
        let m = mods(state);
        Event::Key {
            win: self.win_of(e.event),
            key: key_from_code(e.detail.saturating_sub(EVDEV_OFFSET) as u32),
            mods: m,
            text: if m.ctrl || m.alt {
                None
            } else {
                self.text(e.detail, state)
            },
        }
    }

    fn on_selection_request(&self, e: &SelectionRequestEvent) {
        let c = &self.conn;
        let mut property = e.property;
        let ok = match (&self.clipboard, e.target) {
            (Some(_), t) if t == self.atoms.targets => c
                .change_property32(
                    PropMode::REPLACE,
                    e.requestor,
                    e.property,
                    AtomEnum::ATOM,
                    &[self.atoms.targets, self.atoms.png],
                )
                .is_ok(),
            (Some(png), t) if t == self.atoms.png && png.len() + 64 < c.maximum_request_bytes() => {
                c.change_property8(PropMode::REPLACE, e.requestor, e.property, t, png)
                    .is_ok()
            }
            _ => false,
        };
        if !ok {
            property = NONE;
        }
        let reply = SelectionNotifyEvent {
            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property,
        };
        let _ = c.send_event(false, e.requestor, EventMask::NO_EVENT, reply);
    }
}

impl Display for X11 {
    fn monitors(&mut self) -> Vec<Monitor> {
        let scale = xft_scale(&self.conn, self.root);
        let mut list: Vec<(bool, Area)> = randr::get_monitors(self.conn.as_ref(), self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .map(|m| {
                        let area = Area {
                            x: m.x as i32,
                            y: m.y as i32,
                            w: m.width as i32,
                            h: m.height as i32,
                        };
                        (m.primary, area)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if list.is_empty() {
            let s = self.conn.setup().roots.iter().find(|s| s.root == self.root);
            if let Some(s) = s {
                let area = Area {
                    x: 0,
                    y: 0,
                    w: s.width_in_pixels as i32,
                    h: s.height_in_pixels as i32,
                };
                list.push((true, area));
            }
        }
        list.sort_by_key(|(primary, _)| !primary);
        list.into_iter()
            .map(|(_, area)| Monitor { area, scale })
            .collect()
    }

    fn set_overlays(&mut self, monitors: &[Monitor]) {
        let old: Vec<Win> = self
            .windows
            .keys()
            .copied()
            .filter(|w| matches!(w, Win::Overlay(_)))
            .collect();
        for w in old {
            self.destroy(w);
        }
        self.grab = None;
        self.overlays = monitors.iter().map(|m| m.area).collect();
    }

    fn present(&mut self, win: Win, at: (i32, i32), pixels: PixmapRef, dirty: Option<IntRect>) {
        let size = (pixels.width() as u16, pixels.height() as u16);
        let ready = match self.windows.get(&win) {
            None => self.create(win, at, size),
            Some(n) if n.size != size => self.resize(win, size),
            Some(_) => Ok(()),
        };
        if let Err(e) = ready {
            log(&format!("window {win:?} not ready: {e}"));
            return;
        }
        let Some(n) = self.windows.get_mut(&win) else {
            return;
        };
        if n.at != at {
            n.at = at;
            let aux = ConfigureWindowAux::new().x(at.0).y(at.1);
            let _ = self.conn.configure_window(n.id, &aux);
        }
        let full = IntRect::from_xywh(0, 0, pixels.width(), pixels.height());
        let Some(r) = dirty.or(full) else { return };
        let n = &self.windows[&win];
        if let Err(e) = self.upload(n, pixels, r) {
            log(&format!("put image failed: {e}"));
        }
    }

    fn show(&mut self, win: Win) {
        let Some(n) = self.windows.get_mut(&win) else {
            return;
        };
        n.mapped = true;
        let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        let _ = self.conn.configure_window(n.id, &aux);
        let _ = self.conn.map_window(n.id);
    }

    fn hide(&mut self, win: Win) {
        if let Some(n) = self.windows.get_mut(&win) {
            n.mapped = false;
            let _ = self.conn.unmap_window(n.id);
        }
    }

    fn set_click_through(&mut self, on: bool) {
        self.click_through = on;
        for i in 0..self.overlays.len() {
            self.set_input(Win::Overlay(i), !on);
        }
    }

    fn set_keyboard(&mut self, grab: bool, overlay: usize) {
        if grab {
            // Viewable windows only: map before grabbing.
            self.sync();
            self.grab = self.grab_keyboard(overlay).then_some(overlay);
        } else if self.grab.take().is_some() {
            let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        }
    }

    fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
        let aux = ChangeWindowAttributesAux::new().cursor(self.cursors[cursor as usize]);
        for (w, n) in &self.windows {
            if matches!(w, Win::Overlay(_)) {
                let _ = self.conn.change_window_attributes(n.id, &aux);
            }
        }
    }

    fn grab_hotkeys(&mut self, chords: &[(Action, Chord)]) -> Vec<String> {
        for (keycode, mods) in self.hotkeys.drain(..) {
            let _ = self
                .conn
                .ungrab_key(keycode, self.root, ModMask::from(mods));
        }
        let mut taken = Vec::new();
        // Caps Lock and Num Lock change the state, so grab every combination of them.
        let locks = [
            0,
            u16::from(ModMask::LOCK),
            u16::from(ModMask::M2),
            u16::from(ModMask::LOCK | ModMask::M2),
        ];
        for (action, chord) in chords {
            let Some(code) = code_from_key(chord.key) else {
                continue;
            };
            let keycode = code as u8 + EVDEV_OFFSET;
            let mut mods = 0u16;
            for (on, m) in [
                (chord.shift, ModMask::SHIFT),
                (chord.ctrl, ModMask::CONTROL),
                (chord.alt, ModMask::M1),
            ] {
                if on {
                    mods |= u16::from(m);
                }
            }
            let mut ok = true;
            for lock in locks {
                let grabbed = self
                    .conn
                    .grab_key(
                        true,
                        self.root,
                        ModMask::from(mods | lock),
                        keycode,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )
                    .map_err(|e| e.to_string())
                    .and_then(|c| c.check().map_err(|e| format!("{e:?}")));
                match grabbed {
                    Ok(()) => self.hotkeys.push((keycode, mods | lock)),
                    Err(_) => ok = false,
                }
            }
            log(&format!(
                "hotkey {chord} for {} grabbed: {ok}",
                action.name()
            ));
            if !ok {
                taken.push(chord.to_string());
            }
        }
        taken
    }

    fn capture(&mut self, areas: &[Area]) -> Vec<Option<Pixmap>> {
        let mapped: Vec<Win> = self
            .windows
            .iter()
            .filter(|(_, n)| n.mapped)
            .map(|(w, _)| *w)
            .collect();
        for w in &mapped {
            let _ = self.conn.unmap_window(self.windows[w].id);
        }
        self.sync();
        // Give the windows below (or the compositor) time to repaint.
        std::thread::sleep(Duration::from_millis(if self.composited {
            150
        } else {
            60
        }));
        let images = areas
            .iter()
            .map(|a| {
                let image = self
                    .conn
                    .get_image(
                        ImageFormat::Z_PIXMAP,
                        self.root,
                        a.x as i16,
                        a.y as i16,
                        a.w as u16,
                        a.h as u16,
                        !0,
                    )
                    .ok()?
                    .reply()
                    .inspect_err(|e| log(&format!("screen capture failed: {e}")))
                    .ok()?;
                // 24-bit roots come as 32 bits per pixel, BGRX on little-endian.
                if !matches!(self.root_depth, 24 | 32)
                    || image.data.len() != (a.w * a.h * 4) as usize
                {
                    log("screen capture: unsupported pixel format");
                    return None;
                }
                let mut pixmap = Pixmap::new(a.w as u32, a.h as u32)?;
                pixmap.data_mut().copy_from_slice(&image.data);
                for px in pixmap.data_mut().as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
                Some(pixmap)
            })
            .collect();
        // Back in stacking order: overlays, palette, popups.
        let mut order = mapped;
        order.sort_by_key(|w| match w {
            Win::Overlay(i) => *i,
            Win::Palette => 1000,
            Win::Tooltip => 1001,
            Win::Sheet => 1002,
        });
        for w in order {
            self.show(w);
        }
        // An unmapped window loses the keyboard grab.
        if let Some(i) = self.grab {
            self.set_keyboard(true, i);
        }
        images
    }

    fn copy_png(&mut self, png: Vec<u8>) -> bool {
        self.clipboard = Some(png);
        let c = &self.conn;
        let _ = c.set_selection_owner(self.helper, self.atoms.clipboard, CURRENT_TIME);
        c.get_selection_owner(self.atoms.clipboard)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.owner == self.helper)
    }

    fn forward_key(&mut self, key: Key) {
        let Some(code) = code_from_key(key) else {
            return;
        };
        let keycode = code as u8 + EVDEV_OFFSET;
        // Our grab would catch the fake key: let it go to the focused app meanwhile.
        let grab = self.grab;
        self.set_keyboard(false, 0);
        for kind in [
            x11rb::protocol::xproto::KEY_PRESS_EVENT,
            x11rb::protocol::xproto::KEY_RELEASE_EVENT,
        ] {
            let _ = xtest::fake_input(
                self.conn.as_ref(),
                kind,
                keycode,
                CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            );
        }
        self.sync();
        std::thread::sleep(Duration::from_millis(20));
        if let Some(i) = grab {
            self.set_keyboard(true, i);
        }
        log(&format!("forwarded {key}"));
    }

    fn pointer_position(&mut self) -> Option<(i32, i32)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x as i32, r.root_y as i32))
    }

    fn handle(&mut self, msg: Msg) -> Vec<Event> {
        let Msg::X11(event) = msg else {
            return Vec::new();
        };
        let event = match event {
            XEvent::ButtonPress(e) => {
                let kind = match e.detail {
                    4 => PointerKind::Wheel(true),
                    5 => PointerKind::Wheel(false),
                    b @ 1..=3 => PointerKind::Down(b),
                    _ => return Vec::new(),
                };
                self.pointer_event(&e, kind)
            }
            XEvent::ButtonRelease(e) => match e.detail {
                b @ 1..=3 => self.pointer_event(&e, PointerKind::Up(b)),
                _ => None,
            },
            XEvent::MotionNotify(e) => self.win_of(e.event).map(|win| Event::Pointer {
                win,
                kind: PointerKind::Move,
                at: (e.root_x as i32, e.root_y as i32),
                mods: mods(u16::from(e.state)),
            }),
            XEvent::LeaveNotify(e) => self.win_of(e.event).map(|win| Event::Pointer {
                win,
                kind: PointerKind::Leave,
                at: (e.root_x as i32, e.root_y as i32),
                mods: mods(u16::from(e.state)),
            }),
            XEvent::KeyPress(e) => Some(self.key_event(&e)),
            XEvent::MappingNotify(_) => {
                if let Err(e) = self.load_keymap() {
                    log(&format!("keymap not reloaded: {e}"));
                }
                None
            }
            XEvent::RandrScreenChangeNotify(_) => Some(Event::MonitorsChanged),
            XEvent::SelectionRequest(e) => {
                self.on_selection_request(&e);
                None
            }
            XEvent::SelectionClear(_) => {
                self.clipboard = None;
                None
            }
            XEvent::Error(e) => {
                log(&format!("X error: {e:?}"));
                None
            }
            _ => None,
        };
        event.into_iter().collect()
    }

    fn flush(&mut self) {
        let _ = self.conn.flush();
    }

    fn has_hotkeys(&self) -> bool {
        true
    }
}

/// Xft.dpi from the resource database, as GNOME, KDE and xrandr --dpi set it.
fn xft_scale(conn: &RustConnection, root: Window) -> f32 {
    let resources = conn
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            1 << 20,
        )
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| String::from_utf8_lossy(&r.value).into_owned())
        .unwrap_or_default();
    resources
        .lines()
        .find_map(|l| l.strip_prefix("Xft.dpi:"))
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map_or(1.0, |dpi| (dpi / 96.0).max(1.0))
}

/// Headless check (`SCREENPEN_SMOKE=<dir>`, e.g. under xvfb-run): plays a user through
/// XTEST on its own connection (the drawing hotkey, a pen stroke, a rectangle, typed
/// text, a snapshot), saves what the screen shows to `<dir>/screen.png` and quits.
pub fn smoke(tx: Sender<Msg>, dir: std::path::PathBuf) {
    std::thread::spawn(move || {
        let result = smoke_steps(&dir);
        log(&format!("smoke: {result:?}"));
        let _ = tx.send(Msg::Command(super::instance::Command::Quit));
    });
}

fn smoke_steps(dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::{
        BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
        MOTION_NOTIFY_EVENT,
    };
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (root, w, h) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);
    let pause = |ms| std::thread::sleep(Duration::from_millis(ms));
    let input = |kind: u8, detail: u8, x: i16, y: i16| -> Result<(), ConnectionError> {
        xtest::fake_input(&conn, kind, detail, CURRENT_TIME, root, x, y, 0)?;
        conn.flush()?;
        pause(15);
        Ok(())
    };
    let key = |code: u32, mods: &[u32]| -> Result<(), ConnectionError> {
        for m in mods {
            input(KEY_PRESS_EVENT, *m as u8 + EVDEV_OFFSET, 0, 0)?;
        }
        input(KEY_PRESS_EVENT, code as u8 + EVDEV_OFFSET, 0, 0)?;
        input(KEY_RELEASE_EVENT, code as u8 + EVDEV_OFFSET, 0, 0)?;
        for m in mods.iter().rev() {
            input(KEY_RELEASE_EVENT, *m as u8 + EVDEV_OFFSET, 0, 0)?;
        }
        Ok(())
    };
    let drag = |points: &[(i16, i16)]| -> Result<(), ConnectionError> {
        let (x0, y0) = points[0];
        input(MOTION_NOTIFY_EVENT, 0, x0, y0)?;
        input(BUTTON_PRESS_EVENT, 1, 0, 0)?;
        for &(x, y) in &points[1..] {
            input(MOTION_NOTIFY_EVENT, 0, x, y)?;
        }
        input(BUTTON_RELEASE_EVENT, 1, 0, 0)
    };
    let code = |k: Key| code_from_key(k).unwrap_or(0);
    let (ctrl, alt, shift) = (29, 56, 42);
    pause(500);
    // The default drawing hotkey, Ctrl+Alt+Shift+A, through the passive grab.
    key(code(Key::Char('A')), &[ctrl, alt, shift])?;
    pause(400);
    let wave: Vec<(i16, i16)> = (0..40)
        .map(|i| (100 + i * 8, 300 + ((i as f32 / 4.0).sin() * 60.0) as i16))
        .collect();
    drag(&wave)?;
    key(code(Key::Char('R')), &[])?;
    drag(&[(500, 150), (600, 200), (700, 260)])?;
    key(code(Key::Char('T')), &[])?;
    drag(&[(150, 450)])?;
    key(code(Key::Char('H')), &[shift])?;
    key(code(Key::Char('I')), &[])?;
    key(code(Key::Escape), &[])?;
    key(code(Key::Char('S')), &[ctrl])?;
    pause(600);
    let image = conn
        .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, w, h, !0)?
        .reply()?;
    let mut pixmap = Pixmap::new(w as u32, h as u32).ok_or("size")?;
    for (dst, src) in pixmap
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(image.data.as_chunks::<4>().0)
    {
        *dst = [src[2], src[1], src[0], 255];
    }
    std::fs::create_dir_all(dir)?;
    pixmap.save_png(dir.join("screen.png"))?;
    Ok(())
}
