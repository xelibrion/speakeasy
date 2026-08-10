use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::models::ModelId;

/// Where the weights come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModelChoice {
    /// An entry in [`crate::models::CATALOG`], downloaded on demand.
    Catalog { id: ModelId },

    /// A `.bin` the user already has. Never downloaded, and never deleted by
    /// us — it is not ours to remove.
    Local { path: String },
}

impl Default for ModelChoice {
    fn default() -> Self {
        Self::Catalog {
            id: ModelId::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// The key that drives dictation.
    pub hotkey: Hotkey,

    /// Whether that key is held or tapped. See [`ActivationMode`].
    pub activation: ActivationMode,

    /// Which model the user wants.
    ///
    /// *Desired* state, not actual: on a fresh install this names the default
    /// catalog model before anything has been downloaded. `Engine::ensure_model`
    /// reconciles it against what is on disk, which is what makes first run and
    /// a mid-session switch the same code path. What is actually loaded right
    /// now is [`crate::event::ModelStatus`], which is never persisted.
    pub model: ModelChoice,

    /// Pre-catalog setting, read once and migrated to [`ModelChoice::Local`].
    ///
    /// Kept only so an existing config does not lose its model on upgrade.
    /// [`Config::load`] clears it, so it disappears from the file on the next
    /// save. Delete this field once no user could still be on such a build.
    #[serde(skip_serializing_if = "Option::is_none")]
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

        let mut config: Self = serde_json::from_str(&text).map_err(|err| {
            Error::Other(format!("{} is not valid config: {err}", path.display()))
        })?;
        config.migrate();
        Ok(config)
    }

    /// Fold deprecated fields into their replacements.
    ///
    /// Only applied on load, so the migration runs once and the next save
    /// writes the current shape.
    ///
    /// The `model == default` guard is the subtle part. Serde fills `model`
    /// with the default when the key is absent, so "absent" and "explicitly the
    /// default" are indistinguishable here. Migrating only in that case is the
    /// right trade: a user who has a stale `model_path` and has never chosen a
    /// catalog model gets the local file they were already using, and anyone
    /// with a deliberate selection keeps it.
    fn migrate(&mut self) {
        if let Some(path) = self.model_path.take() {
            if self.model == ModelChoice::default() {
                self.model = ModelChoice::Local { path };
            }
        }
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
            model: ModelChoice::default(),
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

    #[test]
    fn the_default_config_wants_the_default_catalog_model() {
        assert_eq!(
            Config::default().model,
            ModelChoice::Catalog {
                id: ModelId::default()
            }
        );
    }

    /// Configs written before the catalog existed carry a bare path. Losing the
    /// user's model on upgrade would mean silently re-downloading something
    /// they already have.
    #[test]
    fn a_legacy_model_path_migrates_to_a_local_choice() {
        let dir = TempDir::new("legacy-model");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"modelPath": "/models/ggml-base.en.bin"}"#)
            .expect("write should succeed");

        let config = Config::load(&path).expect("a legacy config should load");

        assert_eq!(
            config.model,
            ModelChoice::Local {
                path: "/models/ggml-base.en.bin".to_string()
            }
        );
        assert_eq!(config.model_path, None, "the legacy field is consumed");
    }

    /// A newer build may write a model id this build has never heard of. That
    /// must cost the user the model, not every other setting in the file.
    #[test]
    fn an_unknown_model_id_still_loads_the_rest_of_the_config() {
        let dir = TempDir::new("unknown-model");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"model": {"kind": "catalog", "id": "enormous-v9"}, "minRecordingMs": 400}"#,
        )
        .expect("write should succeed");

        let config = Config::load(&path).expect("an unknown id is not a parse error");

        assert_eq!(config.min_recording_ms, 400);
        assert_eq!(
            config.model,
            ModelChoice::Catalog {
                id: ModelId::new("enormous-v9")
            }
        );
    }

    /// Otherwise a stale `modelPath` left by a downgrade-then-upgrade would
    /// override a selection the user made deliberately.
    #[test]
    fn an_explicit_model_beats_a_leftover_model_path() {
        let dir = TempDir::new("both-model-fields");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"model": {"kind": "catalog", "id": "small.en"}, "modelPath": "/old.bin"}"#,
        )
        .expect("write should succeed");

        let config = Config::load(&path).expect("load should succeed");

        assert_eq!(
            config.model,
            ModelChoice::Catalog {
                id: ModelId::new("small.en")
            }
        );
    }

    #[test]
    fn a_model_choice_survives_a_save_and_load() {
        let dir = TempDir::new("model-roundtrip");
        let path = dir.join("config.json");

        let config = Config {
            model: ModelChoice::Local {
                path: "/tmp/custom.bin".to_string(),
            },
            ..Config::default()
        };
        config.save(&path).expect("save should succeed");

        assert_eq!(Config::load(&path).expect("load should succeed"), config);
    }

    /// The deprecated field should not reappear in files written by this build.
    #[test]
    fn a_saved_config_does_not_write_the_legacy_field() {
        let dir = TempDir::new("no-legacy-field");
        let path = dir.join("config.json");

        Config::default().save(&path).expect("save should succeed");

        let text = std::fs::read_to_string(&path).expect("read should succeed");
        assert!(
            !text.contains("modelPath"),
            "unexpected legacy field: {text}"
        );
    }
}
