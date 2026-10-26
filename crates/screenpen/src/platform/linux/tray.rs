//! Tray icon as a StatusNotifierItem (KDE, most wlroots bars, GNOME with the
//! AppIndicator extension) and desktop notifications. Both talk D-Bus on their own
//! threads; menu picks reach the app as `Msg::Tray`.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::{Msg, log};
use crate::i18n::{Lang, T};
use crate::state::{DIM_LEVELS, FADE_SECONDS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Drawing,
    Ghost,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    ToggleDrawing,
    ToggleGhost,
    ToggleWhiteboard,
    SetDim(usize),
    /// Index into `FADE_SECONDS`, or off.
    SetFade(Option<usize>),
    ToggleAutostart,
    Settings,
    Quit,
}

/// What the menu and the tooltip show.
#[derive(Clone)]
pub struct MenuState {
    pub autostart: bool,
    pub lang: Lang,
    pub version: String,
    pub mode: Mode,
    pub whiteboard: bool,
    pub dim: usize,
    pub fade: Option<usize>,
    pub toggle_key: String,
    pub ghost_key: String,
    pub quit_key: String,
}

struct Item {
    tx: Sender<Msg>,
    menu: MenuState,
}

impl Item {
    fn send(&self, command: Command) {
        let _ = self.tx.send(Msg::Tray(command));
    }
}

fn with_key(label: &str, key: &str) -> String {
    if key.is_empty() {
        label.to_string()
    } else {
        format!("{label}    {key}")
    }
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "screenpen".into()
    }

    fn title(&self) -> String {
        self.menu.version.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::ToggleDrawing);
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        [22, 32, 48]
            .into_iter()
            .filter_map(|size| icon(self.menu.mode, size))
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        let l = self.menu.lang;
        let description = match self.menu.mode {
            Mode::Idle => l.t(T::TipIdle).replace("{key}", &self.menu.toggle_key),
            Mode::Drawing => l.t(T::TipDrawing).to_string(),
            Mode::Ghost => l.t(T::TipGhost).replace("{key}", &self.menu.ghost_key),
        };
        ToolTip {
            title: self.menu.version.clone(),
            description,
            ..ToolTip::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let m = &self.menu;
        let l = m.lang;
        let check = |label: String, checked: bool, enabled: bool, command: Command| {
            CheckmarkItem {
                label,
                checked,
                enabled,
                activate: Box::new(move |this: &mut Self| this.send(command)),
                ..CheckmarkItem::default()
            }
            .into()
        };
        let button = |label: String, command: Command| {
            StandardItem {
                label,
                activate: Box::new(move |this: &mut Self| this.send(command)),
                ..StandardItem::default()
            }
            .into()
        };
        let dim_names = [T::DimOff, T::DimSlightly, T::DimMedium, T::DimStrongly];
        let dim = RadioGroup {
            selected: m.dim,
            select: Box::new(|this: &mut Self, i| this.send(Command::SetDim(i))),
            options: dim_names
                .iter()
                .take(DIM_LEVELS.len())
                .map(|name| RadioItem {
                    label: l.t(*name).to_string(),
                    ..RadioItem::default()
                })
                .collect(),
        };
        let mut fade_names = vec![l.t(T::FadeOff).to_string()];
        fade_names.extend(
            FADE_SECONDS
                .iter()
                .map(|s| l.t(T::FadeAfter).replace("{n}", &s.to_string())),
        );
        let fade = RadioGroup {
            selected: m.fade.map_or(0, |i| i + 1),
            select: Box::new(|this: &mut Self, i| {
                this.send(Command::SetFade(i.checked_sub(1)));
            }),
            options: fade_names
                .into_iter()
                .map(|label| RadioItem {
                    label,
                    ..RadioItem::default()
                })
                .collect(),
        };
        vec![
            StandardItem {
                label: m.version.clone(),
                enabled: false,
                ..StandardItem::default()
            }
            .into(),
            MenuItem::Separator,
            check(
                with_key(l.t(T::Draw), &m.toggle_key),
                m.mode != Mode::Idle,
                true,
                Command::ToggleDrawing,
            ),
            check(
                l.t(T::GhostMenu).to_string(),
                m.mode == Mode::Ghost,
                m.mode != Mode::Idle,
                Command::ToggleGhost,
            ),
            check(
                l.t(T::Whiteboard).to_string(),
                m.whiteboard,
                true,
                Command::ToggleWhiteboard,
            ),
            MenuItem::Separator,
            SubMenu {
                label: l.t(T::DimMenu).to_string(),
                submenu: vec![dim.into()],
                ..SubMenu::default()
            }
            .into(),
            SubMenu {
                label: l.t(T::FadeMenu).to_string(),
                submenu: vec![fade.into()],
                ..SubMenu::default()
            }
            .into(),
            // X11 and Wayland can't keep a window out of screen sharing: shown disabled.
            CheckmarkItem {
                label: l.t(T::HidePalette).to_string(),
                enabled: false,
                ..CheckmarkItem::default()
            }
            .into(),
            MenuItem::Separator,
            check(
                l.t(T::Autostart).to_string(),
                m.autostart,
                true,
                Command::ToggleAutostart,
            ),
            button(l.t(T::Settings).to_string(), Command::Settings),
            button(with_key(l.t(T::Quit), &m.quit_key), Command::Quit),
        ]
    }
}

pub struct Tray {
    handle: Option<Handle<Item>>,
}

impl Tray {
    /// Starts the tray service. Without a StatusNotifierWatcher (plain GNOME) there's
    /// no icon; screenpen still runs and is reachable through its shortcuts.
    pub fn new(tx: Sender<Msg>, menu: MenuState) -> Tray {
        let handle = Item { tx, menu }
            .spawn()
            .inspect_err(|e| {
                log(&format!(
                    "no tray icon ({e}); on GNOME install the AppIndicator extension"
                ))
            })
            .ok();
        Tray { handle }
    }

    pub fn update(&self, menu: MenuState) {
        if let Some(h) = &self.handle {
            h.update(|item| item.menu = menu);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown().wait();
        }
    }
}

/// A desktop notification through org.freedesktop.Notifications, sent in the
/// background so a slow or missing daemon doesn't stall drawing.
pub fn notify(title: &str, text: &str) {
    let (title, text) = (title.to_string(), text.to_string());
    std::thread::spawn(move || {
        let sent = zbus::blocking::Connection::session().and_then(|c| {
            let hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
            c.call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "screenpen",
                    0u32,
                    "",
                    title.as_str(),
                    text.as_str(),
                    Vec::<&str>::new(),
                    hints,
                    5000i32,
                ),
            )
        });
        if let Err(e) = sent {
            log(&format!("notification not shown ({e}): {title}: {text}"));
        }
    });
}

/// A round badge in the mode's color with a pen stroke across it, as SNI wants it:
/// straight (not premultiplied) ARGB, big-endian.
fn icon(mode: Mode, size: u32) -> Option<Icon> {
    let mut pixmap = Pixmap::new(size, size)?;
    let s = size as f32 / 32.0;
    let t = Transform::from_scale(s, s);
    let (r, g, b) = match mode {
        Mode::Idle => (0xff, 0xcc, 0x00),
        Mode::Drawing => (0xff, 0x3b, 0x30),
        Mode::Ghost => (0xaf, 0x52, 0xde),
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    let circle = PathBuilder::from_circle(16.0, 16.0, 15.0)?;
    pixmap.fill_path(&circle, &paint, FillRule::Winding, t, None);
    let mut pb = PathBuilder::new();
    pb.move_to(9.0, 23.0);
    pb.line_to(23.0, 9.0);
    paint.set_color(Color::from_rgba8(0x1c, 0x1c, 0x1e, 255));
    let stroke = Stroke {
        width: 4.5,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&pb.finish()?, &paint, &stroke, t, None);
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.alpha(), c.red(), c.green(), c.blue()]
        })
        .collect();
    Some(Icon {
        width: size as i32,
        height: size as i32,
        data,
    })
}
