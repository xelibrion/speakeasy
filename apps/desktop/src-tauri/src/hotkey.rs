//! Adapter from Tauri's global-shortcut plugin to the engine.
//!
//! Note this does *not* implement `speakeasy_core::HotkeyListener` — the shell
//! drives the engine directly, because Tauri owns the shortcut registration.
//! The port exists so a SwiftUI shell (or a headless test) can supply its own
//! source of press/release; see `speakeasy_core::fakes::FakeHotkeys`.

use speakeasy_core::Engine;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub fn register(
    app: AppHandle,
    engine: Engine,
    accelerator: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shortcut: Shortcut = accelerator.parse()?;

    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            let engine = engine.clone();
            match event.state() {
                ShortcutState::Pressed => {
                    if let Err(err) = engine.begin_recording() {
                        tracing::warn!(%err, "could not start recording");
                    }
                }
                ShortcutState::Released => {
                    // Transcription blocks; keep it off the event thread so the
                    // UI stays responsive and the next press is not delayed.
                    std::thread::spawn(move || {
                        if let Err(err) = engine.finish_recording() {
                            tracing::warn!(%err, "recording did not complete");
                        }
                    });
                }
            }
        })?;

    Ok(())
}
