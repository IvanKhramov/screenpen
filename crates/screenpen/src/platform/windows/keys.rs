//! Virtual-key codes to and from `Key`. Keys are physical: letters and punctuation are
//! named after the US layout whatever layout is active.

use screenpen_core::{Chord, Key};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
    VK_INSERT, VK_LEFT, VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5,
    VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

const NAMED: [(u16, Key); 15] = [
    (VK_ESCAPE, Key::Escape),
    (VK_RETURN, Key::Enter),
    (VK_SPACE, Key::Space),
    (VK_TAB, Key::Tab),
    (VK_BACK, Key::Backspace),
    (VK_DELETE, Key::Delete),
    (VK_INSERT, Key::Insert),
    (VK_HOME, Key::Home),
    (VK_END, Key::End),
    (VK_PRIOR, Key::PageUp),
    (VK_NEXT, Key::PageDown),
    (VK_LEFT, Key::Left),
    (VK_RIGHT, Key::Right),
    (VK_UP, Key::Up),
    (VK_DOWN, Key::Down),
];

const PUNCTUATION: [(u16, char); 11] = [
    (VK_OEM_1, ';'),
    (VK_OEM_2, '/'),
    (VK_OEM_3, '`'),
    (VK_OEM_4, '['),
    (VK_OEM_5, '\\'),
    (VK_OEM_6, ']'),
    (VK_OEM_7, '\''),
    (VK_OEM_COMMA, ','),
    (VK_OEM_MINUS, '-'),
    (VK_OEM_PERIOD, '.'),
    (VK_OEM_PLUS, '='),
];

pub fn key_from_vk(vk: u16) -> Option<Key> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(Key::Char(vk as u8 as char)),
        f if (VK_F1..VK_F1 + 24).contains(&f) => Some(Key::F((f - VK_F1 + 1) as u8)),
        _ => NAMED
            .iter()
            .find(|(v, _)| *v == vk)
            .map(|(_, k)| *k)
            .or_else(|| {
                PUNCTUATION
                    .iter()
                    .find(|(v, _)| *v == vk)
                    .map(|(_, c)| Key::Char(*c))
            }),
    }
}

pub fn vk_from_key(key: Key) -> Option<u16> {
    match key {
        Key::Char(c) if c.is_ascii_uppercase() || c.is_ascii_digit() => Some(c as u16),
        Key::Char(c) => PUNCTUATION.iter().find(|(_, p)| *p == c).map(|(v, _)| *v),
        Key::F(n) if (1..=24).contains(&n) => Some(VK_F1 + n as u16 - 1),
        Key::F(_) => None,
        named => NAMED.iter().find(|(_, k)| *k == named).map(|(v, _)| *v),
    }
}

fn down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

pub fn modifiers() -> (bool, bool, bool) {
    (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT))
}

/// The chord for a key press, with the modifiers held right now.
pub fn chord_from_vk(vk: u16) -> Option<Chord> {
    let key = key_from_vk(vk)?;
    let (ctrl, alt, shift) = modifiers();
    Some(Chord {
        ctrl,
        alt,
        shift,
        key,
    })
}
