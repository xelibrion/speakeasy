//! Core event stream → webview events.
//!
//! The core emits over a `std::sync::mpsc::Receiver`; a background thread
//! drains it and re-emits through Tauri. A SwiftUI shell writes the equivalent
//! twenty lines against the same `Engine::subscribe`, which is why the core
//! stays free of both.

use speakeasy_core::Engine;
use tauri::{AppHandle, Emitter};

/// Event name the frontend listens on.
pub const EVENT: &str = "speakeasy://event";

pub fn forward_events(app: AppHandle, engine: Engine) {
    let events = engine.subscribe();
    std::thread::spawn(move || {
        // Ends when the engine drops its sender, i.e. on shutdown.
        for event in events {
            if let Err(err) = app.emit(EVENT, &event) {
                tracing::warn!(%err, "failed to forward engine event to webview");
            }
        }
    });
}
