use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Flat enum with owned payloads — directly expressible in a Swift binding.
#[derive(Debug, Error, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum Error {
    #[error("microphone unavailable: {0}")]
    AudioUnavailable(String),

    #[error("microphone permission denied")]
    MicrophonePermissionDenied,

    /// macOS: Accessibility / Input Monitoring not granted. The user has to
    /// grant these in System Settings; there is no way to re-prompt.
    #[error("input permission denied: {0}")]
    InputPermissionDenied(String),

    /// macOS secure input mode: a password field has globally disabled event
    /// taps, so the hotkey and text injection are both dead until it clears.
    #[error("secure input is active; text cannot be injected right now")]
    SecureInputActive,

    #[error("no speech model loaded")]
    ModelNotLoaded,

    #[error("could not download the model: {0}")]
    ModelDownload(String),

    /// The bytes arrived but the digest did not match. Its own variant because
    /// the remedy is specific — retry — where a transport failure may mean the
    /// machine is offline and retrying is pointless.
    #[error("the downloaded model was corrupted")]
    ModelVerification,

    #[error("transcription failed: {0}")]
    Transcription(String),

    #[error("could not inject text: {0}")]
    Injection(String),

    #[error("hotkey registration failed: {0}")]
    Hotkey(String),

    #[error("{0}")]
    Other(String),
}
