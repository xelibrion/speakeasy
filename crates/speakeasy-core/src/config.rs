use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

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

    /// Where the user dragged the status widget, in physical screen pixels.
    /// `None` means "wherever the shell puts it by default".
    ///
    /// Shell-facing rather than behavioural, and it lives here for one reason:
    /// this is the struct that gets written to disk. A shell with no floating
    /// widget simply leaves it alone.
    pub widget_position: Option<WidgetPosition>,
}

/// Physical screen coordinates of the widget's top-left corner.
///
/// Physical rather than logical because that is what the window APIs on both
/// platforms take, and converting twice is how a window ends up on the wrong
/// monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetPosition {
    pub x: i32,
    pub y: i32,
}

impl Config {
    /// Read config from `path`.
    ///
    /// A missing file is not an error — it is a first run, and the defaults are
    /// the right answer. Malformed JSON *is* an error: silently overwriting a
    /// file the user hand-edited would be worse than refusing to start from it.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => {
                return Err(Error::Other(format!(
                    "could not read {}: {err}",
                    path.display()
                )));
            }
        };

        serde_json::from_str(&text)
            .map_err(|err| Error::Other(format!("{} is not valid config: {err}", path.display())))
    }

    /// Write config to `path`, creating the directory if needed.
    ///
    /// Writes through a temporary file and renames, so an interrupted save
    /// leaves the previous config intact rather than a truncated one.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                Error::Other(format!("could not create {}: {err}", parent.display()))
            })?;
        }

        let text = serde_json::to_string_pretty(self)
            .map_err(|err| Error::Other(format!("could not serialise config: {err}")))?;

        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text).map_err(|err| {
            Error::Other(format!("could not write {}: {err}", temporary.display()))
        })?;
        std::fs::rename(&temporary, path)
            .map_err(|err| Error::Other(format!("could not replace {}: {err}", path.display())))
    }
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
            widget_position: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn a_missing_file_loads_the_defaults() {
        let dir = TempDir::new("missing");
        let config = Config::load(&dir.join("config.json")).expect("first run is not an error");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn config_survives_a_save_and_load() {
        let dir = TempDir::new("roundtrip");
        let path = dir.join("config.json");

        let config = Config {
            activation: ActivationMode::PushToTalk,
            widget_position: Some(WidgetPosition { x: 1200, y: 900 }),
            ..Config::default()
        };
        config.save(&path).expect("save should succeed");

        assert_eq!(Config::load(&path).expect("load should succeed"), config);
    }

    #[test]
    fn saving_creates_the_directory() {
        let dir = TempDir::new("nested");
        let path = dir.join("deeper").join("config.json");

        Config::default().save(&path).expect("save should succeed");
        assert!(path.exists());
    }

    #[test]
    fn malformed_config_is_an_error_rather_than_a_silent_reset() {
        let dir = TempDir::new("malformed");
        let path = dir.join("config.json");
        std::fs::write(&path, "{ not json").expect("write should succeed");

        assert!(Config::load(&path).is_err());
    }

    /// Every field is `#[serde(default)]`, so a file written by an older build
    /// must still load — that is what makes adding a setting a non-event.
    #[test]
    fn a_config_missing_new_fields_still_loads() {
        let dir = TempDir::new("partial");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"minRecordingMs": 400}"#).expect("write should succeed");

        let config = Config::load(&path).expect("a partial config should load");
        assert_eq!(config.min_recording_ms, 400);
        assert_eq!(config.activation, Config::default().activation);
    }
}
