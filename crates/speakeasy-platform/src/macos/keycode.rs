//! `KeyCode` ↔ `CGKeyCode`.
//!
//! Values are the `kVK_*` constants from `Carbon/HIToolbox/Events.h`. They are
//! positional — `kVK_ANSI_A` is whatever key sits where A sits on a US layout —
//! which is what we want: a binding should follow the physical key, not the
//! character a layout happens to produce there.
//!
//! One table drives both directions. Writing them separately is how a key ends
//! up bindable but not capturable.

use speakeasy_core::KeyCode;

/// Every key we can bind, paired with its virtual keycode.
const TABLE: &[(KeyCode, u16)] = &[
    (KeyCode::A, 0x00),
    (KeyCode::S, 0x01),
    (KeyCode::D, 0x02),
    (KeyCode::F, 0x03),
    (KeyCode::H, 0x04),
    (KeyCode::G, 0x05),
    (KeyCode::Z, 0x06),
    (KeyCode::X, 0x07),
    (KeyCode::C, 0x08),
    (KeyCode::V, 0x09),
    (KeyCode::B, 0x0B),
    (KeyCode::Q, 0x0C),
    (KeyCode::W, 0x0D),
    (KeyCode::E, 0x0E),
    (KeyCode::R, 0x0F),
    (KeyCode::Y, 0x10),
    (KeyCode::T, 0x11),
    (KeyCode::Digit1, 0x12),
    (KeyCode::Digit2, 0x13),
    (KeyCode::Digit3, 0x14),
    (KeyCode::Digit4, 0x15),
    (KeyCode::Digit6, 0x16),
    (KeyCode::Digit5, 0x17),
    (KeyCode::Digit9, 0x19),
    (KeyCode::Digit7, 0x1A),
    (KeyCode::Digit8, 0x1C),
    (KeyCode::Digit0, 0x1D),
    (KeyCode::O, 0x1F),
    (KeyCode::U, 0x20),
    (KeyCode::I, 0x22),
    (KeyCode::P, 0x23),
    (KeyCode::Return, 0x24),
    (KeyCode::L, 0x25),
    (KeyCode::J, 0x26),
    (KeyCode::K, 0x28),
    (KeyCode::N, 0x2D),
    (KeyCode::M, 0x2E),
    (KeyCode::Tab, 0x30),
    (KeyCode::Space, 0x31),
    (KeyCode::Delete, 0x33),
    (KeyCode::Escape, 0x35),
    (KeyCode::F17, 0x40),
    (KeyCode::F18, 0x4F),
    (KeyCode::F19, 0x50),
    (KeyCode::F20, 0x5A),
    (KeyCode::F5, 0x60),
    (KeyCode::F6, 0x61),
    (KeyCode::F7, 0x62),
    (KeyCode::F3, 0x63),
    (KeyCode::F8, 0x64),
    (KeyCode::F9, 0x65),
    (KeyCode::F11, 0x67),
    (KeyCode::F13, 0x69),
    (KeyCode::F16, 0x6A),
    (KeyCode::F14, 0x6B),
    (KeyCode::F10, 0x6D),
    (KeyCode::F12, 0x6F),
    (KeyCode::F15, 0x71),
    (KeyCode::F4, 0x76),
    (KeyCode::F2, 0x78),
    (KeyCode::F1, 0x7A),
    (KeyCode::Left, 0x7B),
    (KeyCode::Right, 0x7C),
    (KeyCode::Down, 0x7D),
    (KeyCode::Up, 0x7E),
    (KeyCode::Equal, 0x18),
    (KeyCode::Minus, 0x1B),
    (KeyCode::RightBracket, 0x1E),
    (KeyCode::LeftBracket, 0x21),
    (KeyCode::Quote, 0x27),
    (KeyCode::Semicolon, 0x29),
    (KeyCode::Backslash, 0x2A),
    (KeyCode::Comma, 0x2B),
    (KeyCode::Slash, 0x2C),
    (KeyCode::Period, 0x2F),
    (KeyCode::Grave, 0x32),
    (KeyCode::Home, 0x73),
    (KeyCode::PageUp, 0x74),
    (KeyCode::ForwardDelete, 0x75),
    (KeyCode::End, 0x77),
    (KeyCode::PageDown, 0x79),
    // No `kVK_*` constant exists for this one: Events.h only names keys Apple
    // ships. macOS still reports the PC menu key positionally, as 110.
    (KeyCode::ContextMenu, 0x6E),
];

/// `None` for a key we cannot name — a media key, or a modifier, which reports
/// through the flags instead. Returning `None` rather than guessing is what
/// keeps a capture from binding the wrong key.
pub fn from_cg_keycode(raw: u16) -> Option<KeyCode> {
    TABLE
        .iter()
        .find(|(_, candidate)| *candidate == raw)
        .map(|(key, _)| *key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mapped_keycode_resolves_to_its_key() {
        for (key, raw) in TABLE {
            assert_eq!(from_cg_keycode(*raw), Some(*key), "{key:?} did not return");
        }
    }

    /// The table must cover every variant, not merely agree with itself. A key
    /// missing here is one the user can press during capture and never bind.
    #[test]
    fn every_keycode_is_mapped() {
        for key in KeyCode::ALL {
            assert!(
                TABLE.iter().any(|(candidate, _)| *candidate == key),
                "{key:?} has no keycode"
            );
        }
    }

    /// A duplicated raw value would make `from_cg_keycode` return whichever
    /// entry came first, so a captured key could bind as a different one.
    #[test]
    fn no_two_keys_share_a_keycode() {
        for (index, (_, raw)) in TABLE.iter().enumerate() {
            let duplicate = TABLE[index + 1..].iter().any(|(_, other)| other == raw);
            assert!(!duplicate, "keycode {raw:#x} appears twice");
        }
    }

    #[test]
    fn an_unmapped_keycode_is_none_rather_than_a_wrong_key() {
        assert_eq!(from_cg_keycode(0xFFFF), None);
    }
}
