use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// The key that drives dictation.
    pub hotkey: Hotkey,

    /// Whether that key is held or tapped. See [`ActivationMode`].
    pub activation: ActivationMode,

    /// Path to the whisper model weights. `None` means first-run download.
    pub model_path: Option<String>,

    /// BCP-47 language hint, or `None` to let the model auto-detect.
    pub language: Option<String>,

    /// Injection strategy. See [`InjectionMode`].
    pub injection_mode: InjectionMode,

    /// Drop recordings shorter than this — usually an accidental hotkey tap.
    pub min_recording_ms: u64,
}

/// How the hotkey starts and stops a recording.
///
/// The listener reports press and release either way; the difference is only
/// which of those edges the engine acts on, so switching modes needs no change
/// to the platform layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivationMode {
    /// Record while the key is held; release transcribes. Nothing keeps
    /// recording if the user is interrupted mid-sentence, but a long dictation
    /// means a long hold.
    PushToTalk,

    /// Tap to start, tap again to transcribe. The default: kinder to long
    /// dictation and to anyone for whom holding a key is uncomfortable. The
    /// widget is what keeps a forgotten recording visible.
    Toggle,
}

/// The push-to-talk binding.
///
/// Two shapes, because they are registered by completely different machinery:
/// a bare modifier needs a low-level event tap, while an accelerator can go
/// through an ordinary global-shortcut registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Hotkey {
    /// A single modifier held down. The natural shape for push-to-talk, and
    /// the only one that can tell the left key from the right.
    Modifier { key: ModifierKey },

    /// A conventional chord such as `"CmdOrCtrl+Alt+Space"`. Cannot express a
    /// bare modifier, and cannot distinguish sides.
    Accelerator { accelerator: String },
}

/// A modifier key, with the side it sits on.
///
/// macOS reports side through IOKit's device-dependent flag bits; the ordinary
/// `CGEventFlags` only say "Option is down" without saying which one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModifierKey {
    LeftOption,
    RightOption,
    LeftCommand,
    RightCommand,
    LeftControl,
    RightControl,
    LeftShift,
    RightShift,
    Fn,
}

impl ModifierKey {
    /// Human-readable name for settings UI and logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::LeftOption => "Left Option",
            Self::RightOption => "Right Option",
            Self::LeftCommand => "Left Command",
            Self::RightCommand => "Right Command",
            Self::LeftControl => "Left Control",
            Self::RightControl => "Right Control",
            Self::LeftShift => "Left Shift",
            Self::RightShift => "Right Shift",
            Self::Fn => "Fn",
        }
    }
}

impl Hotkey {
    pub fn label(&self) -> String {
        match self {
            Self::Modifier { key } => key.label().to_string(),
            Self::Accelerator { accelerator } => accelerator.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InjectionMode {
    /// Synthesize key events per character. Preserves the clipboard, but is
    /// unreliable in some apps (Electron, terminals) and slow for long text.
    Synthetic,

    /// Write to the pasteboard and synthesize paste. Reliable nearly
    /// everywhere, but clobbers the clipboard unless it is saved and restored.
    Paste,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Right Option is a good default: it is a modifier most layouts
            // leave free, and holding it reads naturally as push-to-talk.
            hotkey: Hotkey::Modifier {
                key: ModifierKey::RightOption,
            },
            activation: ActivationMode::Toggle,
            model_path: None,
            language: None,
            injection_mode: InjectionMode::Paste,
            min_recording_ms: 250,
        }
    }
}
