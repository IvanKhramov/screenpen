//! Native Wayland backend for compositors with wlr-layer-shell (KDE Plasma, Sway,
//! Hyprland): every window is a layer surface on the overlay layer, drawn from wl_shm
//! buffers. Overlays fill their output and ignore exclusive zones; ghost mode gives
//! them an empty input region; drawing takes the keyboard exclusively.
//!
//! Wayland has no global hotkeys (users bind `screenpen --toggle` in their desktop
//! settings), no screen capture without a portal and no key injection, so freeze,
//! the screen under a snapshot and slide-key forwarding are unavailable here.

use std::collections::HashMap;
use std::ffi::CString;
use std::io::Read;
use std::sync::mpsc::{Sender, SyncSender, sync_channel};

use screenpen_core::{Action, Chord, Key};
use sctk::compositor::{CompositorHandler, CompositorState, Region};
use sctk::output::{OutputHandler, OutputState};
use sctk::reexports::client::globals::registry_queue_init;
use sctk::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface,
};
use sctk::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::seat::pointer::cursor_shape::CursorShapeManager;
use sctk::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use sctk::seat::{Capability, SeatHandler, SeatState};
use sctk::shell::WaylandSurface;
use sctk::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use sctk::shm::slot::{Buffer, SlotPool};
use sctk::shm::{Shm, ShmHandler};
use sctk::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, registry_handlers,
};
use tiny_skia::{IntRect, Pixmap, PixmapRef};
use xkbcommon_dl::{
    XkbCommon, xkb_context, xkb_context_flags, xkb_keymap, xkb_keymap_compile_flags,
    xkb_keymap_format, xkb_state,
};

use super::display::{Area, Cursor, Display, Event, Monitor, PointerKind, Win};
use super::keys::key_from_code;
use super::ui::monitor_at;
use super::{Msg, log};
use crate::canvas::Mods;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// The latest pixels of a window, kept while it's hidden too: a layer surface is
/// created on show and needs a buffer after its first configure.
struct Frame {
    at: (i32, i32),
    width: u32,
    height: u32,
    data: Vec<u8>,
    /// What changed since the last commit.
    damage: Option<IntRect>,
}

struct Mapped {
    layer: LayerSurface,
    output: usize,
    scale: i32,
    configured: bool,
    /// A frame callback is pending; commit when it fires.
    waiting: bool,
    /// The buffer on screen, kept alive until replaced.
    buffer: Option<Buffer>,
}

/// Keymap state from libxkbcommon (loaded at runtime), for typed characters.
struct Xkb {
    lib: &'static XkbCommon,
    context: *mut xkb_context,
    keymap: *mut xkb_keymap,
    state: *mut xkb_state,
}

impl Xkb {
    fn new(lib: &'static XkbCommon, keymap: &str) -> Option<Xkb> {
        let text = CString::new(keymap.trim_end_matches('\0')).ok()?;
        unsafe {
            let context = (lib.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS);
            if context.is_null() {
                return None;
            }
            let keymap = (lib.xkb_keymap_new_from_string)(
                context,
                text.as_ptr(),
                xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            );
            let state = if keymap.is_null() {
                std::ptr::null_mut()
            } else {
                (lib.xkb_state_new)(keymap)
            };
            let xkb = Xkb {
                lib,
                context,
                keymap,
                state,
            };
            (!state.is_null()).then_some(xkb)
        }
    }

    fn char(&self, evdev: u32) -> Option<char> {
        let c = unsafe { (self.lib.xkb_state_key_get_utf32)(self.state, evdev + 8) };
        char::from_u32(c).filter(|c| *c != '\0' && !c.is_control())
    }
}

impl Drop for Xkb {
    fn drop(&mut self) {
        unsafe {
            if !self.state.is_null() {
                (self.lib.xkb_state_unref)(self.state);
            }
            if !self.keymap.is_null() {
                (self.lib.xkb_keymap_unref)(self.keymap);
            }
            (self.lib.xkb_context_unref)(self.context);
        }
    }
}

struct State {
    qh: QueueHandle<State>,
    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    cursor_shapes: Option<CursorShapeManager>,
    /// Monitors (in pixels) and their outputs, as last listed.
    monitors: Vec<(Monitor, wl_output::WlOutput)>,
    /// Overlay n covers `monitors[overlays[n]]`.
    overlays: Vec<usize>,
    frames: HashMap<Win, Frame>,
    mapped: HashMap<Win, Mapped>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<(wl_pointer::WlPointer, Option<WpCursorShapeDeviceV1>)>,
    xkb: Option<Xkb>,
    mods: Mods,
    key_focus: Option<Win>,
    /// Serial of the last pointer enter, for setting the cursor.
    enter_serial: Option<u32>,
    pointer_at: Option<(i32, i32)>,
    /// Buttons held on our surfaces.
    buttons: u32,
    cursor: Cursor,
    click_through: bool,
    keyboard_overlay: Option<usize>,
    events: Vec<Event>,
}

pub struct Wayland {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    /// Lets the reader thread read the next batch once this one is dispatched.
    ack: SyncSender<()>,
}

impl Wayland {
    pub fn connect(tx: Sender<Msg>) -> Result<Wayland, String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let layer_shell =
            LayerShell::bind(&globals, &qh).map_err(|_| "no wlr-layer-shell".to_string())?;
        let compositor = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let shm = Shm::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let pool = SlotPool::new(1 << 22, &shm).map_err(|e| e.to_string())?;
        let mut state = State {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &qh),
            seats: SeatState::new(&globals, &qh),
            cursor_shapes: CursorShapeManager::bind(&globals, &qh).ok(),
            qh,
            compositor,
            layer_shell,
            shm,
            pool,
            monitors: Vec::new(),
            overlays: Vec::new(),
            frames: HashMap::new(),
            mapped: HashMap::new(),
            keyboard: None,
            pointer: None,
            xkb: None,
            mods: Mods::default(),
            key_focus: None,
            enter_serial: None,
            pointer_at: None,
            buttons: 0,
            cursor: Cursor::Cross,
            click_through: false,
            keyboard_overlay: None,
            events: Vec::new(),
        };
        // Outputs and seats announce themselves over the first round trips.
        for _ in 0..2 {
            queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        }
        state.events.clear();
        log("native Wayland with layer-shell");

        let (ack, next) = sync_channel::<()>(1);
        let reader = conn.clone();
        std::thread::spawn(move || {
            loop {
                if let Some(guard) = reader.prepare_read() {
                    let fd = guard.connection_fd();
                    let mut fds = [rustix::event::PollFd::new(
                        &fd,
                        rustix::event::PollFlags::IN | rustix::event::PollFlags::ERR,
                    )];
                    while let Err(rustix::io::Errno::INTR) = rustix::event::poll(&mut fds, None) {}
                    if let Err(e) = guard.read()
                        && !matches!(&e, sctk::reexports::client::backend::WaylandError::Io(io)
                            if io.kind() == std::io::ErrorKind::WouldBlock)
                    {
                        log(&format!("Wayland connection lost: {e}"));
                        let _ = tx.send(Msg::Disconnected);
                        break;
                    }
                }
                if tx.send(Msg::Wayland).is_err() || next.recv().is_err() {
                    break;
                }
            }
        });
        Ok(Wayland {
            conn,
            queue,
            state,
            ack,
        })
    }
}

impl State {
    fn output_info(&self) -> Vec<(Monitor, wl_output::WlOutput)> {
        self.outputs
            .outputs()
            .filter_map(|output| {
                let info = self.outputs.info(&output)?;
                let scale = info.scale_factor.max(1);
                let (x, y) = info.logical_position.unwrap_or(info.location);
                let (w, h) = info.logical_size.or_else(|| {
                    info.modes
                        .iter()
                        .find(|m| m.current)
                        .map(|m| (m.dimensions.0 / scale, m.dimensions.1 / scale))
                })?;
                let area = Area {
                    x: x * scale,
                    y: y * scale,
                    w: w * scale,
                    h: h * scale,
                };
                Some((
                    Monitor {
                        area,
                        scale: scale as f32,
                    },
                    output,
                ))
            })
            .collect()
    }

    fn win_of(&self, surface: &wl_surface::WlSurface) -> Option<Win> {
        self.mapped
            .iter()
            .find(|(_, m)| m.layer.wl_surface() == surface)
            .map(|(w, _)| *w)
    }

    /// Where the window's top-left corner is, in global pixels.
    fn origin(&self, win: Win) -> (i32, i32) {
        match win {
            Win::Overlay(i) => self
                .overlays
                .get(i)
                .and_then(|&m| self.monitors.get(m))
                .map_or((0, 0), |(m, _)| (m.area.x, m.area.y)),
            _ => self.frames.get(&win).map_or((0, 0), |f| f.at),
        }
    }

    /// The monitor a window belongs to: its overlay's, or the one under its corner.
    fn monitor_index(&self, win: Win) -> Option<usize> {
        match win {
            Win::Overlay(i) => self.overlays.get(i).copied(),
            _ => {
                let at = self.frames.get(&win)?.at;
                let list: Vec<Monitor> = self.monitors.iter().map(|(m, _)| *m).collect();
                let m = monitor_at(&list, at)?;
                list.iter().position(|x| *x == m)
            }
        }
    }

    /// Size and offset of a popup in surface coordinates on its monitor.
    fn place(&self, win: Win, m: usize, layer: &LayerSurface) {
        let (Some(f), Some((monitor, _))) = (self.frames.get(&win), self.monitors.get(m)) else {
            return;
        };
        let s = monitor.scale as i32;
        let (w, h) = (f.width.div_ceil(s as u32), f.height.div_ceil(s as u32));
        layer.set_size(w, h);
        let left = (f.at.0 - monitor.area.x) / s;
        let top = (f.at.1 - monitor.area.y) / s;
        layer.set_margin(top, 0, 0, left);
    }

    fn create(&mut self, win: Win) {
        let Some(m) = self.monitor_index(win) else {
            return;
        };
        let Some((monitor, output)) = self.monitors.get(m).cloned() else {
            return;
        };
        let scale = monitor.scale as i32;
        let surface = self.compositor.create_surface(&self.qh);
        surface.set_buffer_scale(scale);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("screenpen"),
            Some(&output),
        );
        match win {
            Win::Overlay(i) => {
                layer.set_anchor(Anchor::all());
                layer.set_exclusive_zone(-1);
                layer.set_size(0, 0);
                if self.keyboard_overlay == Some(i) {
                    layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
                }
            }
            _ => {
                layer.set_anchor(Anchor::TOP | Anchor::LEFT);
                self.place(win, m, &layer);
            }
        }
        let through = win == Win::Tooltip || matches!(win, Win::Overlay(_)) && self.click_through;
        self.set_input(&layer, !through);
        layer.commit();
        self.mapped.insert(
            win,
            Mapped {
                layer,
                output: m,
                scale,
                configured: false,
                waiting: false,
                buffer: None,
            },
        );
    }

    fn set_input(&self, layer: &LayerSurface, on: bool) {
        if on {
            layer.wl_surface().set_input_region(None);
        } else if let Ok(region) = Region::new(&self.compositor) {
            layer
                .wl_surface()
                .set_input_region(Some(region.wl_region()));
        }
    }

    /// Sends the frame's pixels when the surface can take them.
    fn commit(&mut self, win: Win) {
        let (Some(f), Some(m)) = (self.frames.get_mut(&win), self.mapped.get_mut(&win)) else {
            return;
        };
        if !m.configured || m.waiting {
            return;
        }
        let Some(damage) = f.damage.take() else {
            return;
        };
        // Buffers must be a multiple of the scale; pad with transparent pixels.
        let s = m.scale as u32;
        let (bw, bh) = (f.width.div_ceil(s) * s, f.height.div_ceil(s) * s);
        let Ok((buffer, canvas)) = self.pool.create_buffer(
            bw as i32,
            bh as i32,
            bw as i32 * 4,
            wl_shm::Format::Argb8888,
        ) else {
            log("no shared memory for a frame");
            return;
        };
        canvas.fill(0);
        let row = f.width as usize * 4;
        for (dst, src) in canvas
            .chunks_exact_mut(bw as usize * 4)
            .zip(f.data.chunks_exact(row))
        {
            dst[..row].copy_from_slice(src);
        }
        let surface = m.layer.wl_surface();
        if buffer.attach_to(surface).is_err() {
            return;
        }
        surface.damage_buffer(
            damage.x(),
            damage.y(),
            damage.width() as i32,
            damage.height() as i32,
        );
        surface.frame(&self.qh, surface.clone());
        m.layer.commit();
        m.waiting = true;
        m.buffer = Some(buffer);
    }

    fn apply_keyboard(&self) {
        for (win, m) in &self.mapped {
            if let Win::Overlay(i) = win {
                let mode = if self.keyboard_overlay == Some(*i) {
                    KeyboardInteractivity::Exclusive
                } else {
                    KeyboardInteractivity::None
                };
                m.layer.set_keyboard_interactivity(mode);
                m.layer.commit();
            }
        }
    }

    fn apply_cursor(&self) {
        if let (Some((_, Some(device))), Some(serial)) = (&self.pointer, self.enter_serial) {
            let shape = match self.cursor {
                Cursor::Arrow => Shape::Default,
                Cursor::Cross => Shape::Crosshair,
                Cursor::Text => Shape::Text,
            };
            device.set_shape(serial, shape);
        }
    }
}

impl Display for Wayland {
    fn monitors(&mut self) -> Vec<Monitor> {
        self.state.monitors = self.state.output_info();
        self.state.monitors.iter().map(|(m, _)| *m).collect()
    }

    fn set_overlays(&mut self, monitors: &[Monitor]) {
        let s = &mut self.state;
        s.mapped.retain(|w, _| !matches!(w, Win::Overlay(_)));
        s.frames.retain(|w, _| !matches!(w, Win::Overlay(_)));
        s.overlays = monitors
            .iter()
            .filter_map(|m| s.monitors.iter().position(|(x, _)| x == m))
            .collect();
    }

    fn present(&mut self, win: Win, at: (i32, i32), pixels: PixmapRef, dirty: Option<IntRect>) {
        let s = &mut self.state;
        let (w, h) = (pixels.width(), pixels.height());
        let full = IntRect::from_xywh(0, 0, w, h);
        let frame = s.frames.entry(win).or_insert_with(|| Frame {
            at,
            width: 0,
            height: 0,
            data: Vec::new(),
            damage: None,
        });
        let resized = (frame.width, frame.height) != (w, h);
        let moved = frame.at != at;
        let region = if resized {
            frame.width = w;
            frame.height = h;
            frame.data = pixels.data().to_vec();
            full
        } else {
            let r = dirty.or(full);
            if let Some(r) = r {
                let stride = w as usize * 4;
                for y in r.top() as usize..r.bottom() as usize {
                    let span =
                        y * stride + r.left() as usize * 4..y * stride + r.right() as usize * 4;
                    frame.data[span.clone()].copy_from_slice(&pixels.data()[span]);
                }
            }
            r
        };
        frame.at = at;
        frame.damage = match (frame.damage, region) {
            (Some(a), Some(b)) => a.to_rect().join(&b.to_rect()).and_then(|r| r.round_out()),
            (a, b) => a.or(b),
        };
        if (resized || moved) && !matches!(win, Win::Overlay(_)) && s.mapped.contains_key(&win) {
            // A popup that moved to another monitor needs a surface there.
            if s.monitor_index(win) != s.mapped.get(&win).map(|m| m.output) {
                s.mapped.remove(&win);
                s.create(win);
            } else if let Some(m) = s.mapped.get(&win) {
                s.place(win, m.output, &m.layer);
            }
        }
        s.commit(win);
    }

    fn show(&mut self, win: Win) {
        if !self.state.mapped.contains_key(&win) {
            if let Some(f) = self.state.frames.get_mut(&win) {
                f.damage = IntRect::from_xywh(0, 0, f.width, f.height);
            }
            self.state.create(win);
        }
    }

    fn hide(&mut self, win: Win) {
        self.state.mapped.remove(&win);
    }

    fn set_click_through(&mut self, on: bool) {
        let s = &mut self.state;
        s.click_through = on;
        for (win, m) in &s.mapped {
            if matches!(win, Win::Overlay(_)) {
                s.set_input(&m.layer, !on);
                m.layer.commit();
            }
        }
    }

    fn set_keyboard(&mut self, grab: bool, overlay: usize) {
        self.state.keyboard_overlay = grab.then_some(overlay);
        self.state.apply_keyboard();
    }

    fn set_cursor(&mut self, cursor: Cursor) {
        self.state.cursor = cursor;
        self.state.apply_cursor();
    }

    fn grab_hotkeys(&mut self, chords: &[(Action, Chord)]) -> Vec<String> {
        if !chords.is_empty() {
            log("Wayland has no global hotkeys: bind `screenpen --toggle` to a desktop shortcut");
        }
        Vec::new()
    }

    fn capture(&mut self, areas: &[Area]) -> Vec<Option<Pixmap>> {
        log("screen capture isn't available on Wayland; freeze and snapshots show the ink only");
        areas.iter().map(|_| None).collect()
    }

    fn copy_png(&mut self, _png: Vec<u8>) -> bool {
        false
    }

    fn forward_key(&mut self, key: Key) {
        log(&format!(
            "{key} not passed on: Wayland doesn't let apps send keys (use ghost mode)"
        ));
    }

    fn pointer_position(&mut self) -> Option<(i32, i32)> {
        self.state.pointer_at
    }

    fn handle(&mut self, msg: Msg) -> Vec<Event> {
        if !matches!(msg, Msg::Wayland) {
            return Vec::new();
        }
        if let Err(e) = self.queue.dispatch_pending(&mut self.state) {
            log(&format!("Wayland dispatch failed: {e}"));
        }
        let _ = self.ack.try_send(());
        std::mem::take(&mut self.state.events)
    }

    fn flush(&mut self) {
        let _ = self.conn.flush();
    }

    fn has_hotkeys(&self) -> bool {
        false
    }

    fn watch_input(&mut self, on: bool) -> bool {
        if on {
            log("Wayland doesn't let apps see other apps' clicks and keys");
        }
        false
    }
}

// MARK: protocol handlers

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(win) = self.win_of(surface) {
            if let Some(m) = self.mapped.get_mut(&win) {
                m.waiting = false;
            }
            self.commit(win);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.events.push(Event::MonitorsChanged);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.events.push(Event::MonitorsChanged);
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.events.push(Event::MonitorsChanged);
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.mapped.retain(|_, m| &m.layer != layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(win) = self.win_of(layer.wl_surface()) else {
            return;
        };
        if let Some(m) = self.mapped.get_mut(&win)
            && !m.configured
        {
            m.configured = true;
            if let Some(f) = self.frames.get_mut(&win) {
                f.damage = IntRect::from_xywh(0, 0, f.width, f.height);
            }
        }
        self.commit(win);
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = Some(seat.get_keyboard(qh, ()));
        }
        if capability == Capability::Pointer
            && self.pointer.is_none()
            && let Ok(pointer) = self.seats.get_pointer(qh, &seat)
        {
            let device = self
                .cursor_shapes
                .as_ref()
                .map(|c| c.get_shape_device(&pointer, qh));
            self.pointer = Some((pointer, device));
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
        if capability == Capability::Pointer
            && let Some((p, device)) = self.pointer.take()
        {
            if let Some(d) = device {
                d.destroy();
            }
            p.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for e in events {
            let Some(win) = self.win_of(&e.surface) else {
                continue;
            };
            let scale = self.mapped.get(&win).map_or(1, |m| m.scale) as f64;
            let origin = self.origin(win);
            let at = (
                origin.0 + (e.position.0 * scale) as i32,
                origin.1 + (e.position.1 * scale) as i32,
            );
            let kind = match e.kind {
                PointerEventKind::Enter { serial } => {
                    self.enter_serial = Some(serial);
                    self.apply_cursor();
                    PointerKind::Move
                }
                // Leaving with a button down means the compositor took the pointer.
                PointerEventKind::Leave { .. } if std::mem::take(&mut self.buttons) > 0 => {
                    PointerKind::Lost
                }
                PointerEventKind::Leave { .. } => PointerKind::Leave,
                PointerEventKind::Motion { .. } => PointerKind::Move,
                PointerEventKind::Press { button, .. } => match button_number(button) {
                    Some(b) => {
                        self.buttons += 1;
                        PointerKind::Down(b)
                    }
                    None => continue,
                },
                PointerEventKind::Release { button, .. } => match button_number(button) {
                    Some(b) => {
                        self.buttons = self.buttons.saturating_sub(1);
                        PointerKind::Up(b)
                    }
                    None => continue,
                },
                PointerEventKind::Axis { vertical, .. } if vertical.absolute != 0.0 => {
                    PointerKind::Wheel(vertical.absolute < 0.0)
                }
                PointerEventKind::Axis { .. } => continue,
            };
            self.pointer_at = Some(at);
            self.events.push(Event::Pointer {
                win,
                kind,
                at,
                mods: self.mods,
            });
        }
    }
}

fn button_number(button: u32) -> Option<u8> {
    match button {
        BTN_LEFT => Some(1),
        BTN_MIDDLE => Some(2),
        BTN_RIGHT => Some(3),
        _ => None,
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                state.xkb = None;
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    return;
                }
                let mut text = String::new();
                let read = std::fs::File::from(fd)
                    .take(size as u64)
                    .read_to_string(&mut text);
                match (read, xkbcommon_dl::xkbcommon_option()) {
                    (Ok(_), Some(lib)) => state.xkb = Xkb::new(lib, &text),
                    (_, None) => log("libxkbcommon not found: typed text uses no layout"),
                    (Err(e), _) => log(&format!("keymap not read: {e}")),
                }
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                state.key_focus = state.win_of(&surface);
            }
            wl_keyboard::Event::Leave { .. } => {
                state.key_focus = None;
                state.mods = Mods::default();
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(x) = &state.xkb {
                    unsafe {
                        (x.lib.xkb_state_update_mask)(
                            x.state,
                            mods_depressed,
                            mods_latched,
                            mods_locked,
                            0,
                            0,
                            group,
                        );
                    }
                }
                // Standard keymaps put Shift, Control and Mod1 (Alt) at bits 0, 2 and 3.
                let held = mods_depressed | mods_latched;
                state.mods = Mods {
                    shift: held & 1 != 0,
                    ctrl: held & 4 != 0,
                    alt: held & 8 != 0,
                };
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                let mods = state.mods;
                let text = if mods.ctrl || mods.alt {
                    None
                } else {
                    state.xkb.as_ref().and_then(|x| x.char(key))
                };
                state.events.push(Event::Key {
                    win: state.key_focus,
                    key: key_from_code(key),
                    mods,
                    text,
                });
            }
            _ => {}
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_registry!(State);
