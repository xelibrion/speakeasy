//! Platform implementations of the [`speakeasy_core::ports`] traits.
//!
//! This is the only crate that may use `cfg(target_os)`. Everything above it
//! sees one API; everything below it is per-OS.

pub mod audio;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as sys;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as sys;

use speakeasy_core::{Config, TextSink};

pub use audio::CpalAudioSource;

/// The platform's text injector, configured for the chosen strategy.
pub fn text_sink(config: &Config) -> Box<dyn TextSink> {
    sys::text_sink(config.injection_mode)
}

/// Whether the OS-level permissions needed for capture and injection have been
/// granted. On macOS this covers Microphone plus Accessibility; on Linux
/// (X11) there is nothing to grant, so it is always `true`.
pub fn permissions_granted() -> bool {
    sys::permissions_granted()
}

/// Open the OS settings pane where the user grants the missing permissions.
/// No-op where the concept does not apply.
pub fn open_permission_settings() {
    sys::open_permission_settings();
}
