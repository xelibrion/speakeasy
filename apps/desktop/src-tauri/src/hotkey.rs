//! Wires the platform's push-to-talk listener to the engine.
//!
//! This used to drive Tauri's global-shortcut plugin. It cannot: that plugin
//! needs an accelerator with a non-modifier key, and it reports "Option" with
//! no side. Push-to-talk on a bare Right Option needs the event tap in
//! `speakeasy-platform`, behind the `HotkeyListener` port.

use std::sync::OnceLock;

use speakeasy_core::{Engine, Hotkey, HotkeyEvent, HotkeyListener};

/// The listener owns an OS-level tap and must outlive `setup`.
static LISTENER: OnceLock<Box<dyn HotkeyListener>> = OnceLock::new();

pub fn register(engine: Engine, hotkey: &Hotkey) -> Result<(), Box<dyn std::error::Error>> {
    let listener = LISTENER.get_or_init(speakeasy_platform::hotkey_listener);

    listener.register(
        hotkey,
        Box::new(move |event| match event {
            HotkeyEvent::Pressed => {
                if let Err(err) = engine.begin_recording() {
                    tracing::warn!(%err, "could not start recording");
                }
            }
            HotkeyEvent::Released => {
                // Transcription blocks; keep it off the tap's thread or the
                // event tap stalls and macOS eventually disables it.
                let engine = engine.clone();
                std::thread::spawn(move || {
                    if let Err(err) = engine.finish_recording() {
                        tracing::warn!(%err, "recording did not complete");
                    }
                });
            }
        }),
    )?;

    Ok(())
}
