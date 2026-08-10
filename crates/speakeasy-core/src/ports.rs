//! The four capabilities the core needs from the outside world.
//!
//! Implementations live in `speakeasy-platform` (OS APIs), `speakeasy-whisper`
//! (inference), and [`crate::fakes`] (tests). The core knows only these traits,
//! which is what makes the shell and the platform layer independently
//! replaceable.
//!
//! All ports are `Send + Sync` because the engine drives transcription off the
//! caller's thread.

use crate::config::Hotkey;
use crate::error::Result;

/// Captures microphone audio, already resampled to [`crate::TARGET_SAMPLE_RATE`]
/// mono f32. Resampling belongs to the implementation so the core never has to
/// care about device-native rates.
pub trait AudioSource: Send + Sync {
    /// Begin capturing. Samples accumulate until [`AudioSource::stop`].
    fn start(&self) -> Result<()>;

    /// Stop capturing and return everything recorded since `start`.
    fn stop(&self) -> Result<Vec<f32>>;

    /// Rolling input level in 0.0..=1.0, for the recording indicator.
    /// Returns 0.0 when not recording.
    fn level(&self) -> f32;

    /// Per-band energy of the last moment of audio, for a spectrum display.
    ///
    /// Exactly [`crate::SPECTRUM_BANDS`] values in 0.0..=1.0, low frequency
    /// first, log-spaced across the speech range. All zeros when not recording.
    ///
    /// Separate from [`AudioSource::level`] because the two answer different
    /// questions — "how loud" versus "how does it sound" — and a caller drawing
    /// bars wants the second without paying for the first.
    fn spectrum(&self) -> Vec<f32>;
}

/// Speech-to-text over a complete utterance.
///
/// Deliberately not streaming: Speakeasy records on push-to-talk and
/// transcribes the buffer on release. Adding a streaming port later is a new
/// trait, not a change to this one.
pub trait Transcriber: Send + Sync {
    /// `samples` is mono f32 at [`crate::TARGET_SAMPLE_RATE`].
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<String>;

    /// Whether weights are loaded and `transcribe` can be called.
    fn is_ready(&self) -> bool;
}

/// What the user did with the push-to-talk key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

/// Invoked on every push-to-talk press and release. Called from whatever thread
/// the implementation listens on, so it must not block.
pub type HotkeyCallback = Box<dyn Fn(HotkeyEvent) + Send + Sync>;

/// A global push-to-talk binding.
pub trait HotkeyListener: Send + Sync {
    /// Register `hotkey` and invoke `on_event` for press and release.
    /// Replaces any previously registered binding.
    ///
    /// Implementations need not support every [`Hotkey`] shape; an unsupported
    /// one must fail with [`crate::Error::Hotkey`] rather than bind nothing.
    fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback) -> Result<()>;

    fn unregister(&self) -> Result<()>;
}

/// Delivers text to whatever application currently has focus.
pub trait TextSink: Send + Sync {
    /// Returns the number of characters delivered.
    ///
    /// Implementations must fail with [`crate::Error::SecureInputActive`]
    /// rather than silently dropping text when a password field has disabled
    /// event taps.
    fn inject(&self, text: &str) -> Result<usize>;
}
