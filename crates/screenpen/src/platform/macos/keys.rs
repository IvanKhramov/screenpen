//! macOS virtual key codes (`kVK_*`) to and from `Key`. Key codes are physical: letters
//! and punctuation are named after the US layout whatever layout is active.

use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use screenpen_core::{Chord, Key};

use crate::canvas::Mods;

const KEYS: [(u16, Key); 70] = [
    (0x00, Key::Char('A')),
    (0x0B, Key::Char('B')),
    (0x08, Key::Char('C')),
    (0x02, Key::Char('D')),
    (0x0E, Key::Char('E')),
    (0x03, Key::Char('F')),
    (0x05, Key::Char('G')),
    (0x04, Key::Char('H')),
    (0x22, Key::Char('I')),
    (0x26, Key::Char('J')),
    (0x28, Key::Char('K')),
    (0x25, Key::Char('L')),
    (0x2E, Key::Char('M')),
    (0x2D, Key::Char('N')),
    (0x1F, Key::Char('O')),
    (0x23, Key::Char('P')),
    (0x0C, Key::Char('Q')),
    (0x0F, Key::Char('R')),
    (0x01, Key::Char('S')),
    (0x11, Key::Char('T')),
    (0x20, Key::Char('U')),
    (0x09, Key::Char('V')),
    (0x0D, Key::Char('W')),
    (0x07, Key::Char('X')),
    (0x10, Key::Char('Y')),
    (0x06, Key::Char('Z')),
    (0x1D, Key::Char('0')),
    (0x12, Key::Char('1')),
    (0x13, Key::Char('2')),
    (0x14, Key::Char('3')),
    (0x15, Key::Char('4')),
    (0x17, Key::Char('5')),
    (0x16, Key::Char('6')),
    (0x1A, Key::Char('7')),
    (0x1C, Key::Char('8')),
    (0x19, Key::Char('9')),
    (0x29, Key::Char(';')),
    (0x2C, Key::Char('/')),
    (0x32, Key::Char('`')),
    (0x21, Key::Char('[')),
    (0x2A, Key::Char('\\')),
    (0x1E, Key::Char(']')),
    (0x27, Key::Char('\'')),
    (0x2B, Key::Char(',')),
    (0x1B, Key::Char('-')),
    (0x2F, Key::Char('.')),
    (0x18, Key::Char('=')),
    (0x35, Key::Escape),
    (0x24, Key::Enter),
    (0x31, Key::Space),
    (0x30, Key::Tab),
    (0x33, Key::Backspace),
    (0x75, Key::Delete),
    // Help sits where Insert is on PC keyboards.
    (0x72, Key::Insert),
    (0x73, Key::Home),
    (0x77, Key::End),
    (0x74, Key::PageUp),
    (0x79, Key::PageDown),
    (0x7B, Key::Left),
    (0x7C, Key::Right),
    (0x7E, Key::Up),
    (0x7D, Key::Down),
    (0x7A, Key::F(1)),
    (0x78, Key::F(2)),
    (0x63, Key::F(3)),
    (0x76, Key::F(4)),
    (0x60, Key::F(5)),
    (0x61, Key::F(6)),
    (0x62, Key::F(7)),
    (0x64, Key::F(8)),
];

/// F9 and up, which don't fit the table above in order.
const MORE_F: [(u16, u8); 12] = [
    (0x65, 9),
    (0x6D, 10),
    (0x67, 11),
    (0x6F, 12),
    (0x69, 13),
    (0x6B, 14),
    (0x71, 15),
    (0x6A, 16),
    (0x40, 17),
    (0x4F, 18),
    (0x50, 19),
    (0x5A, 20),
];

/// Keypad Enter types and confirms like Return.
const KEYPAD_ENTER: u16 = 0x4C;

pub fn key_from_code(code: u16) -> Option<Key> {
    if code == KEYPAD_ENTER {
        return Some(Key::Enter);
    }
    KEYS.iter()
        .find(|(c, _)| *c == code)
        .map(|(_, k)| *k)
        .or_else(|| {
            MORE_F
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| Key::F(*n))
        })
}

pub fn code_from_key(key: Key) -> Option<u16> {
    let key = match key {
        Key::Char(c) => Key::Char(c.to_ascii_uppercase()),
        k => k,
    };
    KEYS.iter()
        .find(|(_, k)| *k == key)
        .map(|(c, _)| *c)
        .or_else(|| match key {
            Key::F(n) => MORE_F.iter().find(|(_, f)| *f == n).map(|(c, _)| *c),
            _ => None,
        })
}

pub fn mods(event: &NSEvent) -> Mods {
    let flags = event.modifierFlags();
    Mods {
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        shift: flags.contains(NSEventModifierFlags::Shift),
    }
}

/// The chord for a key-down event.
pub fn chord(event: &NSEvent) -> Option<Chord> {
    let key = key_from_code(event.keyCode())?;
    let m = mods(event);
    Some(Chord {
        ctrl: m.ctrl,
        alt: m.alt,
        shift: m.shift,
        key,
    })
}

/// Carbon modifier mask (`controlKey`, `optionKey`, `shiftKey`) for a global hotkey.
pub fn carbon_modifiers(chord: Chord) -> u32 {
    const SHIFT: u32 = 0x0200;
    const OPTION: u32 = 0x0800;
    const CONTROL: u32 = 0x1000;
    let mut m = 0;
    if chord.ctrl {
        m |= CONTROL;
    }
    if chord.alt {
        m |= OPTION;
    }
    if chord.shift {
        m |= SHIFT;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for (code, key) in KEYS {
            assert_eq!(key_from_code(code), Some(key));
            assert_eq!(code_from_key(key), Some(code));
        }
        for n in 1..=20 {
            let code = code_from_key(Key::F(n)).expect("F key");
            assert_eq!(key_from_code(code), Some(Key::F(n)));
        }
    }
}
