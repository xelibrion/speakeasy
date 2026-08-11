//! The bindable key shapes, and the decision of whether a raw key event fires
//! the binding.
//!
//! Matching lives here rather than in the platform's event tap because this is
//! where the fiddly cases are — a chord must match its modifiers exactly, and
//! its release can arrive either as the key lifting or as one of its modifiers
//! lifting first. Logic inside a tap callback is reachable only by pressing
//! keys on a real Mac with Accessibility granted; here it is a pure function
//! over a flat event, and a table of cases covers it.

use serde::{Deserialize, Serialize};

/// The modifiers a chord requires, side-agnostic.
///
/// `⌘⌥Space` should not care which Command key was used. A bare modifier
/// binding is the one case where the side is the whole point, and that shape is
/// [`crate::ModifierKey`].
///
/// Four bools rather than a bitflag set: flat owned fields are what
/// `uniffi`/`swift-bridge` can express, so the binding layer stays mechanical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordMods {
    pub command: bool,
    pub option: bool,
    pub control: bool,
    pub shift: bool,
}

impl ChordMods {
    /// The symbols, in the order macOS writes them.
    ///
    /// Menus everywhere on this platform render `⌃⌥⇧⌘` in that order regardless
    /// of how the chord was typed, so a binding that renders any other way
    /// reads as a different shortcut than the one the user set.
    pub fn label(self) -> String {
        let mut out = String::new();
        if self.control {
            out.push('⌃');
        }
        if self.option {
            out.push('⌥');
        }
        if self.shift {
            out.push('⇧');
        }
        if self.command {
            out.push('⌘');
        }
        out
    }

    /// Whether any modifier is required at all.
    ///
    /// A chord with none is a bare key such as `F13`, which is a legitimate
    /// binding — unlike a chord of modifiers with no key, which is a
    /// [`crate::Hotkey::Modifier`] instead.
    pub fn is_empty(self) -> bool {
        !(self.command || self.option || self.control || self.shift)
    }
}

/// A non-modifier key.
///
/// An enum rather than a raw keycode so the config stays readable and
/// portable: a `CGKeyCode` means nothing on another platform, and nothing to a
/// human editing `config.json`. The mapping to platform codes lives in
/// `speakeasy-platform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    Space,
    Return,
    Escape,
    Tab,
    Delete,
    Left,
    Right,
    Up,
    Down,
}

impl KeyCode {
    /// Every variant, so the platform crate can prove its mapping table is
    /// total in both directions. A one-way table silently drops keys during
    /// capture: the user presses F13, the tap reads it, and nothing comes back.
    pub const ALL: [Self; 65] = [
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::N,
        Self::O,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::U,
        Self::V,
        Self::W,
        Self::X,
        Self::Y,
        Self::Z,
        Self::Digit0,
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::Digit5,
        Self::Digit6,
        Self::Digit7,
        Self::Digit8,
        Self::Digit9,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::F13,
        Self::F14,
        Self::F15,
        Self::F16,
        Self::F17,
        Self::F18,
        Self::F19,
        Self::F20,
        Self::Space,
        Self::Return,
        Self::Escape,
        Self::Tab,
        Self::Delete,
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
    ];

    /// How the key is written in a binding.
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::Digit0 => "0",
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::Digit6 => "6",
            Self::Digit7 => "7",
            Self::Digit8 => "8",
            Self::Digit9 => "9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::F13 => "F13",
            Self::F14 => "F14",
            Self::F15 => "F15",
            Self::F16 => "F16",
            Self::F17 => "F17",
            Self::F18 => "F18",
            Self::F19 => "F19",
            Self::F20 => "F20",
            Self::Space => "Space",
            Self::Return => "Return",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Delete => "Delete",
            Self::Left => "←",
            Self::Right => "→",
            Self::Up => "↑",
            Self::Down => "↓",
        }
    }
}
