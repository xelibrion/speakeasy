use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Hotkey;
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
    /// Keystrokes were posted, but nothing on screen looked able to receive
    /// them. Distinct from [`State::Failed`] because it is not a failure: the
    /// text may well have arrived, and the detection behind it is known to be
    /// wrong about apps that draw their own text.
    ///
    /// Exists so the widget can stay up with the transcript in reach instead of
    /// dropping to `Idle` and taking the only copy with it.
    Unconfirmed,
    Failed {
        error: Error,
    },
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

/// One completed transcription, kept so a failed or misdirected injection is
/// recoverable. In memory only — see the design doc's non-goals for why this
/// never touches disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Transcript {
    pub text: String,
    /// Milliseconds since the Unix epoch, not a `SystemTime`: the shell reads
    /// this as a plain number to build a relative time ("2 min ago"), and
    /// serde's built-in `SystemTime` representation is a `{secs, nanos}`
    /// struct no frontend wants to unpack.
    #[serde(serialize_with = "serialize_millis")]
    pub at: SystemTime,
    /// `false` only when `sink.inject` returned `Err` — a genuine failure.
    /// Never set for `State::Unconfirmed`: that detection is a known-wrong
    /// hint for apps that draw their own text (see `6701333`), and badging it
    /// here would mark rows that delivered fine.
    pub delivered: bool,
}

fn serialize_millis<S>(time: &SystemTime, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let millis = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    serializer.serialize_u64(millis)
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

    /// Text recognised so far, while the user is still speaking.
    ///
    /// Advisory and display-only. The committed prefix and the live tail are
    /// joined into one string because the widget renders only the tail of it
    /// and has no way to style them differently; if that changes, this grows a
    /// field rather than every consumer growing a rule.
    ///
    /// Never the text that gets injected — that is [`Event::Transcript`].
    TranscriptPreview {
        text: String,
    },

    /// Injection succeeded; `chars` is what actually reached the focused app.
    Injected {
        chars: usize,
    },

    Failed {
        error: Error,
    },

    /// The dictation binding changed.
    ///
    /// The shell re-registers on this. Core cannot do it itself: the listener
    /// is owned by the shell, and moving it in here would mean the engine held
    /// a listener holding a callback holding the engine.
    HotkeyChanged {
        hotkey: Hotkey,
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

    /// History gained or lost an entry. Carries nothing — an open History
    /// tab re-reads [`crate::Engine::history`] on receipt rather than
    /// diffing a hundred small structs.
    HistoryChanged,
}
