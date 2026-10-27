//! Menu-bar status item: shows that screenpen is running and in which mode; a click
//! opens the menu (the macOS convention, unlike the Windows tray's left click).

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{MainThreadMarker, NSString};
use tiny_skia::Color;

use super::view::{Target, ns_image};
use crate::i18n::{Lang, T};
use crate::state::{DIM_LEVELS, FADE_SECONDS};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Idle,
    Drawing,
    Ghost,
}

#[derive(Clone, Copy)]
pub enum Command {
    ToggleDrawing,
    ToggleGhost,
    ToggleWhiteboard,
    ToggleCaptureHiding,
    SetDim(usize),
    /// Index into `FADE_SECONDS`, or off.
    SetFade(Option<usize>),
    ToggleAutostart,
    Settings,
    Quit,
}

impl Command {
    /// Menu item tags, as in the Windows tray menu.
    pub fn from_tag(tag: isize) -> Option<Command> {
        Some(match tag {
            1 => Command::ToggleDrawing,
            2 => Command::ToggleGhost,
            3 => Command::ToggleWhiteboard,
            4 => Command::ToggleCaptureHiding,
            5 => Command::Quit,
            6 => Command::Settings,
            7 => Command::ToggleAutostart,
            n @ 100..=199 => Command::SetDim(n as usize - 100),
            200 => Command::SetFade(None),
            n @ 201..=299 => Command::SetFade(Some(n as usize - 201)),
            _ => return None,
        })
    }
}

/// What the menu shows checked.
pub struct MenuState {
    pub autostart: bool,
    pub lang: Lang,
    pub version: String,
    pub mode: Mode,
    pub whiteboard: bool,
    pub hide_from_capture: bool,
    pub dim: usize,
    pub fade: Option<usize>,
    pub toggle_key: String,
    pub quit_key: String,
}

pub struct Tray {
    item: Retained<NSStatusItem>,
    icons: Vec<Retained<NSImage>>,
    mode: Mode,
    lang: Lang,
    /// Chords shown in the tooltip: start drawing, leave ghost mode.
    keys: (String, String),
}

impl Tray {
    pub fn new(mtm: MainThreadMarker, target: &Target) -> Tray {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(ProtocolObject::<dyn NSMenuDelegate>::from_ref(target)));
        item.setMenu(Some(&menu));
        let icons = [Mode::Idle, Mode::Drawing, Mode::Ghost]
            .iter()
            .filter_map(|m| make_icon(*m))
            .collect();
        let mut tray = Tray {
            item,
            icons,
            mode: Mode::Ghost,
            lang: Lang::En,
            keys: Default::default(),
        };
        tray.set_mode(mtm, Mode::Idle);
        tray
    }

    pub fn set_text(&mut self, mtm: MainThreadMarker, lang: Lang, keys: (String, String)) {
        self.lang = lang;
        self.keys = keys;
        self.update(mtm);
    }

    pub fn set_mode(&mut self, mtm: MainThreadMarker, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.update(mtm);
        }
    }

    /// Where the icon is, in Cocoa screen coordinates.
    pub fn anchor(&self, mtm: MainThreadMarker) -> Option<CGRect> {
        Some(self.item.button(mtm)?.window()?.frame())
    }

    fn update(&self, mtm: MainThreadMarker) {
        let Some(button) = self.item.button(mtm) else {
            return;
        };
        button.setImage(self.icons.get(self.mode as usize).map(|i| &**i));
        let tip = match self.mode {
            Mode::Idle => self.lang.t(T::TipIdle).replace("{key}", &self.keys.0),
            Mode::Drawing => self.lang.t(T::TipDrawing).to_string(),
            Mode::Ghost => self.lang.t(T::TipGhost).replace("{key}", &self.keys.1),
        };
        button.setToolTip(Some(&NSString::from_str(&tip)));
    }
}

/// Fills `menu` with the items for `m`; picks go to `target`'s `menuItem:`.
pub fn fill_menu(mtm: MainThreadMarker, menu: &NSMenu, target: &Target, m: &MenuState) {
    menu.removeAllItems();
    let l = m.lang;
    let add = |menu: &NSMenu, tag: isize, title: &str, checked: bool, enabled: bool| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel!(menuItem:)),
                &NSString::new(),
            )
        };
        unsafe { item.setTarget(Some(target)) };
        item.setTag(tag);
        item.setEnabled(enabled);
        item.setState(if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        menu.addItem(&item);
    };
    let submenu = |menu: &NSMenu, title: &str| {
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(title));
        let sub = NSMenu::new(mtm);
        sub.setAutoenablesItems(false);
        item.setSubmenu(Some(&sub));
        menu.addItem(&item);
        sub
    };
    let separator = |menu: &NSMenu| menu.addItem(&NSMenuItem::separatorItem(mtm));

    add(menu, 0, &m.version, false, false);
    separator(menu);
    let draw = format!("{}    {}", l.t(T::Draw), m.toggle_key);
    add(menu, 1, &draw, m.mode != Mode::Idle, true);
    add(
        menu,
        2,
        l.t(T::GhostMenu),
        m.mode == Mode::Ghost,
        m.mode != Mode::Idle,
    );
    add(menu, 3, l.t(T::Whiteboard), m.whiteboard, true);
    separator(menu);

    let dim = submenu(menu, l.t(T::DimMenu));
    let dim_names = [T::DimOff, T::DimSlightly, T::DimMedium, T::DimStrongly];
    for (i, name) in dim_names.iter().enumerate().take(DIM_LEVELS.len()) {
        add(&dim, 100 + i as isize, l.t(*name), m.dim == i, true);
    }
    let fade = submenu(menu, l.t(T::FadeMenu));
    add(&fade, 200, l.t(T::FadeOff), m.fade.is_none(), true);
    for (i, seconds) in FADE_SECONDS.iter().enumerate() {
        let text = l.t(T::FadeAfter).replace("{n}", &seconds.to_string());
        add(&fade, 201 + i as isize, &text, m.fade == Some(i), true);
    }
    add(menu, 4, l.t(T::HidePalette), m.hide_from_capture, true);
    separator(menu);
    // The string names Windows; the rest of it fits macOS as is.
    let autostart = l.t(T::Autostart);
    add(menu, 7, autostart, m.autostart, true);
    add(menu, 6, l.t(T::Settings), false, true);
    let quit = format!("{}    {}", l.t(T::Quit), m.quit_key);
    add(menu, 5, &quit, false, true);
}

/// The tray icon is the app icon; a dot tells drawing (red) and mouse mode (purple).
fn dot(mode: Mode) -> Option<Color> {
    match mode {
        Mode::Idle => None,
        Mode::Drawing => Some(Color::from_rgba8(0xff, 0x3b, 0x30, 255)),
        Mode::Ghost => Some(Color::from_rgba8(0xaf, 0x52, 0xde, 255)),
    }
}

/// The icon drawn at 36x36 pixels and shown at 18x18 points.
fn make_icon(mode: Mode) -> Option<Retained<NSImage>> {
    let pixmap = screenpen_core::icon::with_dot(36, dot(mode))?;
    ns_image(&pixmap, 18.0)
}
