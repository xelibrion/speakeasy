use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hotkey::{ChordMods, KeyCode};
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
    /// The key that drives dictation, one binding per [`KeyboardBucket`].
    pub hotkeys: HotkeyBindings,

    /// Pre-`HotkeyBindings` setting, read once and migrated into both
    /// buckets of [`Config::hotkeys`].
    ///
    /// Kept only so an existing config does not lose its binding on upgrade.
    /// [`Config::load`] clears it, so it disappears from the file on the next
    /// save — same treatment as [`Config::model_path`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<Hotkey>,

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

    /// Whether to show recognised text in the widget while the user speaks.
    ///
    /// Costs a decode every 1.5s for the whole time the key is held, so it is
    /// worth being able to turn off on battery even though it defaults on.
    pub transcript_preview: bool,

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

        if let Some(hotkey) = self.hotkey.take() {
            if self.hotkeys == HotkeyBindings::default() {
                self.hotkeys.built_in = hotkey.clone();
                self.hotkeys.external = hotkey;
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
#[serde(tag = "kind", rename_all = "camelCase", from = "HotkeyRepr")]
pub enum Hotkey {
    /// A single modifier held down. The natural shape for push-to-talk, and
    /// the only one that can tell the left key from the right. Never swallowed:
    /// the default binding is Right Option, and consuming it would break
    /// Option-based accented input (`⌥e → é`).
    Modifier { key: ModifierKey },

    /// A conventional chord such as `⌘⌥Space`. Cannot distinguish sides, and
    /// unlike a bare modifier it is swallowed while bound, so it does not also
    /// fire in whatever app has focus.
    Chord { mods: ChordMods, key: KeyCode },
}

/// What actually gets deserialised, so a config written by an older build still
/// loads.
///
/// `Hotkey::Accelerator { accelerator: String }` was the previous second shape.
/// It could never be registered on macOS — the listener returned
/// `Error::Hotkey` for it — so there is nothing to preserve and it maps to the
/// default binding. It lives here rather than on `Hotkey` so the dead shape
/// stays out of the public API, which has to stay expressible in Swift.
///
/// This is not the same judgement as `Config::load`'s "malformed JSON is an
/// error": a legacy shape is not malformed, it is old.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum HotkeyRepr {
    Modifier { key: ModifierKey },
    Chord { mods: ChordMods, key: KeyCode },
    Accelerator { accelerator: String },
}

impl From<HotkeyRepr> for Hotkey {
    fn from(repr: HotkeyRepr) -> Self {
        match repr {
            HotkeyRepr::Modifier { key } => Self::Modifier { key },
            HotkeyRepr::Chord { mods, key } => Self::Chord { mods, key },
            HotkeyRepr::Accelerator { accelerator } => {
                tracing::warn!(
                    %accelerator,
                    "this build cannot bind a string accelerator; falling back to the default"
                );
                Self::default()
            }
        }
    }
}

impl Default for Hotkey {
    fn default() -> Self {
        // Right Option is a good default: it is a modifier most layouts leave
        // free, and holding it reads naturally as push-to-talk.
        Self::Modifier {
            key: ModifierKey::RightOption,
        }
    }
}

/// Which physical keyboard is currently driving the binding.
///
/// Two buckets, not per-device identity: any external keyboard attached
/// selects [`KeyboardBucket::External`], regardless of which model it is or
/// how many are plugged in at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyboardBucket {
    BuiltIn,
    External,
}

/// A separate push-to-talk binding for each [`KeyboardBucket`].
///
/// Two bindings because the natural key differs by hardware: Right Option
/// (the default `built_in` binding) exists on every Mac keyboard and on no
/// PC keyboard, while the Menu key (the default `external` binding) is the
/// reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindings {
    pub built_in: Hotkey,
    pub external: Hotkey,
}

impl HotkeyBindings {
    pub fn get(&self, bucket: KeyboardBucket) -> &Hotkey {
        match bucket {
            KeyboardBucket::BuiltIn => &self.built_in,
            KeyboardBucket::External => &self.external,
        }
    }

    pub fn set(&mut self, bucket: KeyboardBucket, hotkey: Hotkey) {
        match bucket {
            KeyboardBucket::BuiltIn => self.built_in = hotkey,
            KeyboardBucket::External => self.external = hotkey,
        }
    }
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            built_in: Hotkey::default(),
            // No Apple keyboard has this key, which is exactly why it is a
            // good push-to-talk binding for anything else — see
            // `KeyCode::ContextMenu`'s own doc comment.
            external: Hotkey::Chord {
                mods: ChordMods::default(),
                key: KeyCode::ContextMenu,
            },
        }
    }
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
            Self::Chord { mods, key } => format!("{}{}", mods.label(), key.label()),
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
            hotkeys: HotkeyBindings::default(),
            hotkey: None,
            activation: ActivationMode::Toggle,
            model: ModelChoice::default(),
            model_path: None,
            language: None,
            injection_mode: InjectionMode::Paste,
            min_recording_ms: 250,
            transcript_preview: true,
            widget_position: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    /// An existing config predates this field and must keep loading.
    #[test]
    fn a_config_without_the_preview_field_defaults_it_on() {
        let dir = TempDir::new("preview-default");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"activation":"toggle"}"#).expect("write");

        let config = Config::load(&path).expect("old configs still load");

        assert!(config.transcript_preview);
    }

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
    fn a_chord_round_trips_through_json() {
        let hotkey = Hotkey::Chord {
            mods: ChordMods {
                command: true,
                option: true,
                control: false,
                shift: false,
            },
            key: KeyCode::Space,
        };
        let text = serde_json::to_string(&hotkey).expect("serialises");
        let back: Hotkey = serde_json::from_str(&text).expect("deserialises");
        assert_eq!(back, hotkey);
    }

    #[test]
    fn hotkey_bindings_default_to_right_option_and_menu() {
        let bindings = HotkeyBindings::default();
        assert_eq!(
            bindings.built_in,
            Hotkey::Modifier {
                key: ModifierKey::RightOption
            }
        );
        assert_eq!(
            bindings.external,
            Hotkey::Chord {
                mods: ChordMods::default(),
                key: KeyCode::ContextMenu
            }
        );
    }

    #[test]
    fn hotkey_bindings_get_and_set_route_to_the_right_slot() {
        let mut bindings = HotkeyBindings::default();
        let chord = Hotkey::Chord {
            mods: ChordMods {
                command: true,
                ..Default::default()
            },
            key: KeyCode::Space,
        };
        bindings.set(KeyboardBucket::External, chord.clone());
        assert_eq!(bindings.get(KeyboardBucket::External), &chord);
        assert_eq!(bindings.get(KeyboardBucket::BuiltIn), &Hotkey::default());
    }

    #[test]
    fn hotkey_bindings_round_trip_through_json() {
        let mut bindings = HotkeyBindings::default();
        bindings.set(
            KeyboardBucket::External,
            Hotkey::Chord {
                mods: ChordMods::default(),
                key: KeyCode::F13,
            },
        );
        let text = serde_json::to_string(&bindings).expect("serialises");
        let back: HotkeyBindings = serde_json::from_str(&text).expect("deserialises");
        assert_eq!(back, bindings);
    }

    /// A config written before this feature has a bare top-level `hotkey`, not
    /// `hotkeys`. It must migrate into both buckets, preserving whatever custom
    /// binding was there — the same "absent vs. explicitly default" guard
    /// `model_path` migration already uses.
    #[test]
    fn a_legacy_single_hotkey_config_migrates_to_both_buckets() {
        let dir = TempDir::new("legacy-hotkey-migration");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"hotkey":{"kind":"chord","mods":{"command":true,"option":true,"control":false,"shift":false},"key":"space"}}"#,
        )
        .expect("write should succeed");

        let config = Config::load(&path).expect("a legacy config should load");

        let expected = Hotkey::Chord {
            mods: ChordMods {
                command: true,
                option: true,
                control: false,
                shift: false,
            },
            key: KeyCode::Space,
        };
        assert_eq!(config.hotkeys.built_in, expected);
        assert_eq!(config.hotkeys.external, expected);
        assert_eq!(config.hotkey, None, "the legacy field is consumed");
    }

    /// The legacy `Accelerator` shape maps to the ordinary default binding (see
    /// the existing `HotkeyRepr` migration below), which then propagates to both
    /// buckets the same way any other legacy value would.
    #[test]
    fn a_legacy_accelerator_config_falls_back_to_the_default_binding_on_both_buckets() {
        let dir = TempDir::new("legacy-hotkey");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"hotkey":{"kind":"accelerator","accelerator":"CmdOrCtrl+Alt+Space"}}"#,
        )
        .expect("write should succeed");

        let config = Config::load(&path).expect("a legacy config should still load");
        assert_eq!(config.hotkeys.built_in, Hotkey::default());
        assert_eq!(config.hotkeys.external, Hotkey::default());
    }

    #[test]
    fn a_saved_config_does_not_write_the_legacy_hotkey_field() {
        let dir = TempDir::new("no-legacy-hotkey-field");
        let path = dir.join("config.json");

        Config::default().save(&path).expect("save should succeed");

        let text = std::fs::read_to_string(&path).expect("read should succeed");
        assert!(
            !text.contains("\"hotkey\":"),
            "unexpected legacy field: {text}"
        );
    }

    #[test]
    fn a_chord_labels_in_the_order_macos_writes_modifiers() {
        let hotkey = Hotkey::Chord {
            mods: ChordMods {
                command: true,
                option: true,
                control: true,
                shift: true,
            },
            key: KeyCode::Space,
        };
        assert_eq!(hotkey.label(), "⌃⌥⇧⌘Space");
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
