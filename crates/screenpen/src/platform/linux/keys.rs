//! Linux evdev key codes to and from `Key`. Both X11 (keycode - 8) and Wayland
//! report physical keys this way, so shortcuts follow the US layout whatever layout
//! is active.

use screenpen_core::Key;

const KEYS: [(u32, Key); 62] = [
    (1, Key::Escape),
    (2, Key::Char('1')),
    (3, Key::Char('2')),
    (4, Key::Char('3')),
    (5, Key::Char('4')),
    (6, Key::Char('5')),
    (7, Key::Char('6')),
    (8, Key::Char('7')),
    (9, Key::Char('8')),
    (10, Key::Char('9')),
    (11, Key::Char('0')),
    (12, Key::Char('-')),
    (13, Key::Char('=')),
    (14, Key::Backspace),
    (15, Key::Tab),
    (16, Key::Char('Q')),
    (17, Key::Char('W')),
    (18, Key::Char('E')),
    (19, Key::Char('R')),
    (20, Key::Char('T')),
    (21, Key::Char('Y')),
    (22, Key::Char('U')),
    (23, Key::Char('I')),
    (24, Key::Char('O')),
    (25, Key::Char('P')),
    (26, Key::Char('[')),
    (27, Key::Char(']')),
    (28, Key::Enter),
    (30, Key::Char('A')),
    (31, Key::Char('S')),
    (32, Key::Char('D')),
    (33, Key::Char('F')),
    (34, Key::Char('G')),
    (35, Key::Char('H')),
    (36, Key::Char('J')),
    (37, Key::Char('K')),
    (38, Key::Char('L')),
    (39, Key::Char(';')),
    (40, Key::Char('\'')),
    (41, Key::Char('`')),
    (43, Key::Char('\\')),
    (44, Key::Char('Z')),
    (45, Key::Char('X')),
    (46, Key::Char('C')),
    (47, Key::Char('V')),
    (48, Key::Char('B')),
    (49, Key::Char('N')),
    (50, Key::Char('M')),
    (51, Key::Char(',')),
    (52, Key::Char('.')),
    (53, Key::Char('/')),
    (57, Key::Space),
    (102, Key::Home),
    (103, Key::Up),
    (104, Key::PageUp),
    (105, Key::Left),
    (106, Key::Right),
    (107, Key::End),
    (108, Key::Down),
    (109, Key::PageDown),
    (110, Key::Insert),
    (111, Key::Delete),
];

/// Codes of F1-F10, F11-F12 and F13-F24, which aren't contiguous.
const F_KEYS: [(u32, u8, u8); 3] = [(59, 1, 10), (87, 11, 12), (183, 13, 24)];

/// Keypad Enter, which types like Enter.
const KP_ENTER: u32 = 96;

pub fn key_from_code(code: u32) -> Option<Key> {
    if code == KP_ENTER {
        return Some(Key::Enter);
    }
    for (first, lo, hi) in F_KEYS {
        if (first..=first + (hi - lo) as u32).contains(&code) {
            return Some(Key::F(lo + (code - first) as u8));
        }
    }
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, k)| *k)
}

pub fn code_from_key(key: Key) -> Option<u32> {
    if let Key::F(n) = key {
        return F_KEYS
            .iter()
            .find(|(_, lo, hi)| (*lo..=*hi).contains(&n))
            .map(|(first, lo, _)| first + (n - lo) as u32);
    }
    KEYS.iter().find(|(_, k)| *k == key).map(|(c, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for (code, key) in KEYS {
            assert_eq!(key_from_code(code), Some(key));
            assert_eq!(code_from_key(key), Some(code));
        }
        for n in 1..=24 {
            assert_eq!(
                key_from_code(code_from_key(Key::F(n)).unwrap()),
                Some(Key::F(n))
            );
        }
        assert_eq!(code_from_key(Key::F(12)), Some(88));
    }
}
