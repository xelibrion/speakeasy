//! The capabilities the core needs from the outside world.
//!
//! Implementations live in `speakeasy-platform` (OS APIs), `speakeasy-whisper`
//! (inference), and [`crate::fakes`] (tests). The core knows only these traits,
//! which is what makes the shell and the platform layer independently
//! replaceable.
//!
//! All ports are `Send + Sync` because the engine drives transcription off the
//! caller's thread.

use std::path::Path;
use std::sync::atomic::AtomicBool;

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

    /// Everything captured so far, at [`crate::TARGET_SAMPLE_RATE`], without
    /// ending the recording. Empty when not recording.
    ///
    /// Returns the whole buffer rather than taking an offset deliberately: an
    /// offset would have to be mapped between the device's native rate and
    /// 16 kHz on every call, and that arithmetic is where an off-by-one becomes
    /// a word cut in half. The caller slices what it wants.
    fn snapshot(&self) -> Result<Vec<f32>>;

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

    /// Load weights from `path`, replacing whatever is loaded.
    ///
    /// Expensive and blocking. Implementations must leave the previous model
    /// intact when this fails — a bad swap should cost the user nothing, which
    /// is what lets the download flow verify and load before committing.
    fn load(&self, path: &Path) -> Result<()>;
}

/// A span of speech within a buffer, in seconds from its start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechSegment {
    pub start: f32,
    pub end: f32,
}

/// Finds speech within captured audio.
///
/// Separate from [`Transcriber`] rather than folded into it, per the contract
/// on that trait: streaming arrives as a new port. What a preview needs is not
/// more transcription but the answer to "has this utterance finished?", which
/// is what lets text be committed and then never revised.
///
/// An energy threshold cannot answer it. Background noise keeps the level high
/// exactly where the gate is most needed, and whisper hallucinates confidently
/// on non-speech audio — a quiet buffer comes back as "Thank you." rather than
/// as nothing.
pub trait SpeechSegmenter: Send + Sync {
    /// Speech spans within `samples`, which is mono f32 at
    /// [`crate::TARGET_SAMPLE_RATE`]. Empty means no speech.
    fn segments(&self, samples: &[f32]) -> Result<Vec<SpeechSegment>>;
}

/// Fetches a model file over the network.
///
/// A port rather than an HTTP call in the core for the same reason
/// [`Transcriber`] is one: it keeps `speakeasy-core` free of a TLS stack, and
/// it keeps every download branch — progress, a corrupt file, a cancel
/// mid-transfer — testable offline against [`crate::fakes::FakeFetcher`].
pub trait ModelFetcher: Send + Sync {
    /// Stream `url` into `destination`, calling `progress(received, total)` as
    /// bytes arrive. `total` is 0 if the server did not say.
    ///
    /// Blocking; call it off the UI thread. Must return promptly once `cancel`
    /// is set, and must not leave `destination` behind when it does — the
    /// caller reads a surviving file as a completed download.
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()>;
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

/// Invoked once with whatever the user pressed, after which the capture
/// disarms itself.
pub type CaptureCallback = Box<dyn Fn(Hotkey) + Send + Sync>;

/// A global push-to-talk binding.
pub trait HotkeyListener: Send + Sync {
    /// Register `hotkey` and invoke `on_event` for press and release.
    /// Replaces any previously registered binding.
    ///
    /// Implementations need not support every [`Hotkey`] shape; an unsupported
    /// one must fail with [`crate::Error::Hotkey`] rather than bind nothing.
    fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback) -> Result<()>;

    fn unregister(&self) -> Result<()>;

    /// Listen for one binding and report it, rather than matching the bound one.
    ///
    /// This is how the settings window asks "which key did the user press?".
    ///
    /// While armed, matching events are swallowed — otherwise binding `⌘Q`
    /// would quit the app on the way in. Implementations must disarm after one
    /// capture, and must not leave the keyboard captured if the caller
    /// disappears: cancel on a timeout as well as on [`Self::cancel_capture`].
    fn capture(&self, on_captured: CaptureCallback) -> Result<()>;

    fn cancel_capture(&self);
}

/// The result of an injection that did not fail.
///
/// Two fields rather than a bare count because "the keystrokes were posted" and
/// "the text arrived" are different claims, and only the first is ever certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub chars: usize,
    /// `false` only when the implementation positively established there was
    /// nowhere for the text to land. Not knowing must report `true`.
    ///
    /// The asymmetry is the point. A wrong `false` costs a Copy button nobody
    /// needed; a wrong `false` that were allowed to *withhold* text would cost
    /// the dictation, which is why nothing may act on this by refusing to
    /// inject. See `speakeasy-platform`'s `macos::focus` for how badly the
    /// underlying signal behaves for apps that draw their own text.
    pub verified: bool,
}

/// Delivers text to whatever application currently has focus.
pub trait TextSink: Send + Sync {
    /// Post `text` to whatever has focus.
    ///
    /// Implementations must fail with [`crate::Error::SecureInputActive`]
    /// rather than silently dropping text when a password field has disabled
    /// event taps.
    fn inject(&self, text: &str) -> Result<Delivery>;

    /// The app blocking injection right now, by display name, or `None` if
    /// nothing is.
    ///
    /// This is what lets the engine refuse to start rather than record a minute
    /// of speech it already knows it cannot deliver. Answering it costs one
    /// syscall, so it is checked afresh every time — secure input comes and
    /// goes as the user moves between fields.
    ///
    /// Defaults to `None`: secure input is a macOS concept, and a platform
    /// without an equivalent should not have to say so.
    fn blocked_by(&self) -> Option<String> {
        None
    }

    /// Put `text` on the system clipboard.
    ///
    /// The escape hatch for when [`TextSink::inject`] cannot deliver. Injection
    /// needs somewhere to type; the clipboard needs nothing, so it still works
    /// when a password field has killed event taps or when nothing on screen
    /// wants text at all.
    ///
    /// Note this is not what paste-mode injection leaves behind: that restores
    /// the user's previous clipboard shortly after pasting, so by the time
    /// anyone reaches for it the transcript is gone.
    fn copy(&self, text: &str) -> Result<()>;
}
