//! Wires the platform's hotkey listener to the engine.
//!
//! This used to drive Tauri's global-shortcut plugin. It cannot: that plugin
//! needs an accelerator with a non-modifier key, and it reports "Option" with
//! no side. Push-to-talk on a bare Right Option needs the event tap in
//! `speakeasy-platform`, behind the `HotkeyListener` port.
//!
//! Push-to-talk versus toggle is not decided here — the engine reads the
//! configured activation mode and answers with a [`HotkeyAction`], so this
//! layer stays a pure translator.

use std::sync::OnceLock;

use speakeasy_core::{Engine, Hotkey, HotkeyAction, HotkeyListener};

/// The listener owns an OS-level tap and must outlive `setup`.
static LISTENER: OnceLock<Box<dyn HotkeyListener>> = OnceLock::new();

pub fn register(engine: Engine, hotkey: &Hotkey) -> Result<(), Box<dyn std::error::Error>> {
    let listener = LISTENER.get_or_init(speakeasy_platform::hotkey_listener);

    listener.register(
        hotkey,
        Box::new(move |event| match engine.hotkey_action(event) {
            HotkeyAction::Begin => {
                if let Err(err) = engine.begin_recording() {
                    tracing::warn!(%err, "could not start recording");
                }
            }
            HotkeyAction::Finish => {
                // Transcription blocks; keep it off the tap's thread or the
                // event tap stalls and macOS eventually disables it.
                let engine = engine.clone();
                std::thread::spawn(move || {
                    if let Err(err) = engine.finish_recording() {
                        tracing::warn!(%err, "recording did not complete");
                    }
                });
            }
            HotkeyAction::Ignore => {}
        }),
    )?;

    Ok(())
}
