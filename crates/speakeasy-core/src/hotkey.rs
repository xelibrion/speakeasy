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

use crate::config::{Hotkey, ModifierKey};
use crate::ports::HotkeyEvent;

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

/// What a platform event tap saw, flattened.
///
/// Deliberately not a `CGEvent`: this crate must not learn about CoreGraphics,
/// and a flat description is what makes the matching below testable anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawKeyEvent {
    pub kind: RawEventKind,

    /// The non-modifier key involved, when there is one. `None` for a flags
    /// change, which is how a bare modifier reports.
    pub key: Option<KeyCode>,

    /// Side-agnostic modifier state at the moment of the event.
    pub mods: ChordMods,

    /// Which side-specific modifiers are currently held.
    ///
    /// Separate from `mods` because only this can answer a
    /// [`Hotkey::Modifier`] binding, and only `mods` can answer a chord.
    pub sides: Vec<ModifierKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEventKind {
    KeyDown,
    KeyUp,
    FlagsChanged,
}

impl RawKeyEvent {
    pub fn key_down(key: KeyCode, mods: ChordMods) -> Self {
        Self {
            kind: RawEventKind::KeyDown,
            key: Some(key),
            mods,
            sides: Vec::new(),
        }
    }

    pub fn key_up(key: KeyCode, mods: ChordMods) -> Self {
        Self {
            kind: RawEventKind::KeyUp,
            key: Some(key),
            mods,
            sides: Vec::new(),
        }
    }

    pub fn flags_changed(mods: ChordMods) -> Self {
        Self {
            kind: RawEventKind::FlagsChanged,
            key: None,
            mods,
            sides: Vec::new(),
        }
    }

    /// A flags change carrying the side-specific modifiers currently down.
    pub fn modifier_changed(sides: &[ModifierKey]) -> Self {
        Self {
            kind: RawEventKind::FlagsChanged,
            key: None,
            mods: ChordMods::default(),
            sides: sides.to_vec(),
        }
    }
}

/// Does this event start or end the binding?
///
/// Stateless on purpose. It answers "this event satisfies the binding" and
/// nothing about what came before, so a held key that auto-repeats yields
/// `Pressed` every time. **Suppressing repeats is the caller's job** — the
/// macOS tap already holds an `AtomicBool` and acts only on a transition, and
/// that is where the edge detection belongs, next to the thing that has a
/// lifetime.
pub fn match_event(hotkey: &Hotkey, event: RawKeyEvent) -> Option<HotkeyEvent> {
    match hotkey {
        Hotkey::Modifier { key } => match_modifier(*key, &event),
        Hotkey::Chord { mods, key } => match_chord(*mods, *key, &event),
    }
}

/// A bare modifier is held or it is not; the side is the whole point, so this
/// reads `sides` and ignores the side-agnostic set entirely.
fn match_modifier(key: ModifierKey, event: &RawKeyEvent) -> Option<HotkeyEvent> {
    if event.kind != RawEventKind::FlagsChanged {
        return None;
    }
    Some(if event.sides.contains(&key) {
        HotkeyEvent::Pressed
    } else {
        HotkeyEvent::Released
    })
}

fn match_chord(mods: ChordMods, key: KeyCode, event: &RawKeyEvent) -> Option<HotkeyEvent> {
    match event.kind {
        // Exact match, not a subset: `⌘⌥Space` firing on `⌘⇧⌥Space` would make
        // the binding collide with every chord that merely contains it.
        RawEventKind::KeyDown if event.key == Some(key) && event.mods == mods => {
            Some(HotkeyEvent::Pressed)
        }
        RawEventKind::KeyUp if event.key == Some(key) => Some(HotkeyEvent::Released),

        // A modifier lifting while the key is still down ends the hold. Without
        // this, push-to-talk waits for a key-up it will never see, because the
        // user's hand leaves the modifier first.
        RawEventKind::FlagsChanged if event.mods != mods => Some(HotkeyEvent::Released),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord() -> Hotkey {
        Hotkey::Chord {
            mods: ChordMods {
                command: true,
                option: true,
                ..Default::default()
            },
            key: KeyCode::Space,
        }
    }

    fn modifier() -> Hotkey {
        Hotkey::Modifier {
            key: ModifierKey::RightOption,
        }
    }

    fn mods(command: bool, option: bool, shift: bool) -> ChordMods {
        ChordMods {
            command,
            option,
            shift,
            control: false,
        }
    }

    #[test]
    fn capturing_a_bare_modifier_waits_for_it_to_be_let_go() {
        let mut recognizer = CaptureRecognizer::new();

        // Held: nothing is settled yet, because this could still become a chord.
        let down = RawKeyEvent::modifier_changed(&[ModifierKey::RightOption]);
        assert_eq!(recognizer.observe(down), None);

        let up = RawKeyEvent::modifier_changed(&[]);
        assert_eq!(
            recognizer.observe(up),
            Some(Hotkey::Modifier {
                key: ModifierKey::RightOption
            })
        );
    }

    /// The case that makes the delay necessary: binding the first modifier of a
    /// chord would make chords impossible to enter at all.
    #[test]
    fn capturing_a_chord_is_not_derailed_by_its_modifiers_arriving_first() {
        let mut recognizer = CaptureRecognizer::new();

        assert_eq!(
            recognizer.observe(RawKeyEvent::modifier_changed(&[ModifierKey::LeftCommand])),
            None
        );
        assert_eq!(
            recognizer.observe(RawKeyEvent::modifier_changed(&[
                ModifierKey::LeftCommand,
                ModifierKey::LeftOption
            ])),
            None
        );

        let pressed = RawKeyEvent::key_down(KeyCode::Space, mods(true, true, false));
        assert_eq!(
            recognizer.observe(pressed),
            Some(Hotkey::Chord {
                mods: mods(true, true, false),
                key: KeyCode::Space,
            })
        );
    }

    #[test]
    fn letting_go_of_two_modifiers_captures_nothing() {
        let mut recognizer = CaptureRecognizer::new();
        recognizer.observe(RawKeyEvent::modifier_changed(&[
            ModifierKey::LeftCommand,
            ModifierKey::LeftOption,
        ]));
        assert_eq!(recognizer.observe(RawKeyEvent::modifier_changed(&[])), None);
    }

    /// A key with no name cannot be bound, so the gesture stays open rather
    /// than settling on nothing and disarming the capture.
    #[test]
    fn an_unmappable_key_does_not_settle_the_capture() {
        let mut recognizer = CaptureRecognizer::new();
        let event = RawKeyEvent {
            kind: RawEventKind::KeyDown,
            key: None,
            mods: ChordMods::default(),
            sides: Vec::new(),
        };
        assert_eq!(recognizer.observe(event), None);
    }

    #[test]
    fn a_chord_fires_when_the_key_goes_down_with_exactly_its_modifiers() {
        let event = RawKeyEvent::key_down(KeyCode::Space, mods(true, true, false));
        assert_eq!(match_event(&chord(), event), Some(HotkeyEvent::Pressed));
    }

    #[test]
    fn a_chord_does_not_fire_when_an_extra_modifier_is_held() {
        let event = RawKeyEvent::key_down(KeyCode::Space, mods(true, true, true));
        assert_eq!(match_event(&chord(), event), None);
    }

    #[test]
    fn a_chord_does_not_fire_when_a_required_modifier_is_missing() {
        let event = RawKeyEvent::key_down(KeyCode::Space, mods(true, false, false));
        assert_eq!(match_event(&chord(), event), None);
    }

    #[test]
    fn a_chord_releases_when_its_key_lifts() {
        let event = RawKeyEvent::key_up(KeyCode::Space, mods(true, true, false));
        assert_eq!(match_event(&chord(), event), Some(HotkeyEvent::Released));
    }

    #[test]
    fn a_chord_releases_when_a_required_modifier_lifts_before_the_key() {
        let event = RawKeyEvent::flags_changed(mods(false, true, false));
        assert_eq!(match_event(&chord(), event), Some(HotkeyEvent::Released));
    }

    #[test]
    fn an_unrelated_key_is_ignored() {
        let event = RawKeyEvent::key_down(KeyCode::A, mods(true, true, false));
        assert_eq!(match_event(&chord(), event), None);
    }

    #[test]
    fn a_modifier_binding_ignores_ordinary_keys() {
        let event = RawKeyEvent::key_down(KeyCode::Space, ChordMods::default());
        assert_eq!(match_event(&modifier(), event), None);
    }

    #[test]
    fn a_modifier_binding_fires_on_its_own_side_only() {
        let down = RawKeyEvent::modifier_changed(&[ModifierKey::RightOption]);
        assert_eq!(match_event(&modifier(), down), Some(HotkeyEvent::Pressed));

        let other = RawKeyEvent::modifier_changed(&[ModifierKey::LeftOption]);
        assert_eq!(match_event(&modifier(), other), Some(HotkeyEvent::Released));
    }
}

/// Turns a stream of raw events into the one binding the user meant.
///
/// Needed because a chord arrives one key at a time. Pressing `⌘⌥Space` starts
/// with `⌘` going down, and binding `Left Command` there — the first thing the
/// tap sees — would make chords impossible to enter.
///
/// So the rule is: a key going down settles it immediately as a chord, while a
/// bare modifier is only settled once every modifier has been let go, and only
/// if exactly one was held. That way `⌘` on the way to a chord is indistinct
/// from noise, and `⌥` pressed and released on its own is unambiguous.
#[derive(Debug, Default)]
pub struct CaptureRecognizer {
    /// The modifiers held during the current gesture, if it is still only
    /// modifiers.
    held: Vec<ModifierKey>,
}

impl CaptureRecognizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Some` once the gesture is unambiguous. The caller then disarms.
    pub fn observe(&mut self, event: RawKeyEvent) -> Option<Hotkey> {
        match event.kind {
            RawEventKind::KeyDown => {
                // An unmapped key — a media key, say — is not bindable, so the
                // gesture is left open rather than settled on nothing.
                let key = event.key?;
                Some(Hotkey::Chord {
                    mods: event.mods,
                    key,
                })
            }
            RawEventKind::FlagsChanged => {
                if event.sides.is_empty() {
                    // Everything let go. One modifier held throughout is a
                    // binding; two or more is an abandoned chord.
                    let held = std::mem::take(&mut self.held);
                    match held.as_slice() {
                        [key] => Some(Hotkey::Modifier { key: *key }),
                        _ => None,
                    }
                } else {
                    self.held = event.sides;
                    None
                }
            }
            RawEventKind::KeyUp => None,
        }
    }
}
