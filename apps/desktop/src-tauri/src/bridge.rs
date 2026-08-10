//! Core event stream → webview events.
//!
//! The core emits over a `std::sync::mpsc::Receiver`; a background thread
//! drains it and re-emits through Tauri. A SwiftUI shell writes the equivalent
//! twenty lines against the same `Engine::subscribe`, which is why the core
//! stays free of both.

use speakeasy_core::{Engine, Event, State};
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
                // Hand focus back at the *start* of a dictation, not just at
                // launch. The transcript is aimed at whatever had focus when
                // the user spoke, so being the active app means pasting into
                // ourselves — and anything the user does in between (the tray,
                // the settings window) can make us active again. Doing it here
                // leaves the whole of recording and transcription for the
                // change to settle before the paste goes out.
                if matches!(state, State::Recording) {
                    // Before the widget is shown, because showing it is what
                    // takes the focus away.
                    speakeasy_platform::remember_focus();
                }

                // AppKit window and menu calls must happen on the main thread,
                // and the engine emits from a worker. Hopping here is what lets
                // `tray` and `widget` stay free of threading concerns — and
                // skipping it is not a race, it is an immediate hard crash
                // ("Must only be used from the main thread").
                let state = state.clone();
                let ui = app.clone();
                // Read on this thread: the main thread must not block on the
                // engine's config lock.
                let saved = engine.config().widget_position;
                if let Err(err) = app.run_on_main_thread(move || {
                    tray::refresh(&state);
                    widget::sync(&ui, &state, saved);
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
