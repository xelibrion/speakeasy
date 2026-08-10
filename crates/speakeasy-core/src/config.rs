use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Push-to-talk binding, in the shell's accelerator syntax
    /// (e.g. `"CmdOrCtrl+Alt+Space"`).
    pub hotkey: String,

    /// Path to the whisper model weights. `None` means first-run download.
    pub model_path: Option<String>,

    /// BCP-47 language hint, or `None` to let the model auto-detect.
    pub language: Option<String>,

    /// Injection strategy. See [`InjectionMode`].
    pub injection_mode: InjectionMode,

    /// Drop recordings shorter than this — usually an accidental hotkey tap.
    pub min_recording_ms: u64,
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
            hotkey: "CmdOrCtrl+Alt+Space".to_string(),
            model_path: None,
            language: None,
            injection_mode: InjectionMode::Paste,
            min_recording_ms: 250,
        }
    }
}
