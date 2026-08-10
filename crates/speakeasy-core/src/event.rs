use crate::error::Error;

/// Where the engine is in the push-to-talk cycle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum State {
    Idle,
    Recording,
    Transcribing,
    Injecting,
    Failed { error: Error },
}

/// Everything a shell needs to render. Tauri re-emits these to the webview;
/// a SwiftUI shell would map them onto an `@Observable` model.
// No `Eq`: `AudioLevel` carries an f32.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    StateChanged {
        state: State,
    },

    /// Rolling input level, 0.0..=1.0, for a recording indicator.
    AudioLevel {
        level: f32,
    },

    /// Final transcript, before injection.
    Transcript {
        text: String,
    },

    /// Injection succeeded; `chars` is what actually reached the focused app.
    Injected {
        chars: usize,
    },

    Failed {
        error: Error,
    },
}
