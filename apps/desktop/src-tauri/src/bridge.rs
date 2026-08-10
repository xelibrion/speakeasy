//! Core event stream → webview events.
//!
//! The core emits over a `std::sync::mpsc::Receiver`; a background thread
//! drains it and re-emits through Tauri. A SwiftUI shell writes the equivalent
//! twenty lines against the same `Engine::subscribe`, which is why the core
//! stays free of both.

use speakeasy_core::{Engine, Event};
use tauri::{AppHandle, Emitter};

use crate::{tray, widget};

/// Event name the frontend listens on.
pub const EVENT: &str = "speakeasy://event";

pub fn forward_events(app: AppHandle, engine: Engine) {
    let events = engine.subscribe();
    std::thread::spawn(move || {
        // Ends when the engine drops its sender, i.e. on shutdown.
        for event in events {
            // The tray status line is the only UI visible when no window is
            // open, so it tracks state even while the webview is hidden.
            if let Event::StateChanged { state } = &event {
                // AppKit window and menu calls must happen on the main thread,
                // and the engine emits from a worker. Hopping here is what lets
                // `tray` and `widget` stay free of threading concerns — and
                // skipping it is not a race, it is an immediate hard crash
                // ("Must only be used from the main thread").
                let state = state.clone();
                let ui = app.clone();
                if let Err(err) = app.run_on_main_thread(move || {
                    tray::refresh(&state);
                    widget::sync(&ui, &state);
                }) {
                    tracing::warn!(%err, "could not schedule the UI update");
                }
            }
            if let Err(err) = app.emit(EVENT, &event) {
                tracing::warn!(%err, "failed to forward engine event to webview");
            }
        }
    });
}
