use crate::error::Error;
use crate::models::ModelId;

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

/// What the model subsystem is doing.
///
/// Deliberately separate from [`State`]. `State` is the dictation cycle, and
/// the two run concurrently — the user can dictate on the current model while
/// the next one downloads, because the swap only happens once the new weights
/// are verified and loaded. One enum would make those mutually exclusive and
/// would corrupt what the widget means.
///
/// In memory only. The persisted counterpart is [`crate::Config::model`], which
/// records what the user *wants*; this records what is actually loaded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ModelStatus {
    /// Nothing loaded and nothing in flight.
    Absent,

    Downloading {
        id: ModelId,
        received: u64,
        /// Bytes expected. 0 when neither the catalog nor the server said.
        total: u64,
    },

    /// Weights are loaded and dictation can run.
    ///
    /// Carries a display label rather than a [`crate::ModelChoice`] because a
    /// catalog id and a local path have to render in the same tray line, and
    /// the shell should not be reimplementing that formatting.
    Ready {
        label: String,
    },

    Failed {
        error: Error,
    },
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

    /// Model download or load progress.
    ///
    /// One event carrying the whole status rather than separate progress,
    /// ready and failed events: consumers get one thing to switch on, and the
    /// throttle in `ModelManager` applies naturally to the `Downloading`
    /// variant.
    ModelStatusChanged {
        status: ModelStatus,
    },
}
