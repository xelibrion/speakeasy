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

use speakeasy_core::{Config, HotkeyListener, TextSink};

pub use audio::CpalAudioSource;

/// The platform's text injector, configured for the chosen strategy.
pub fn text_sink(config: &Config) -> Box<dyn TextSink> {
    sys::text_sink(config.injection_mode)
}

/// The platform's push-to-talk listener.
///
/// This lives here rather than in the shell because binding a bare, side-
/// specific modifier needs OS-level input monitoring that no cross-platform
/// shortcut library exposes.
pub fn hotkey_listener() -> Box<dyn HotkeyListener> {
    sys::hotkey_listener()
}

/// Whether the OS-level permissions needed for capture and injection have been
/// granted. On macOS this is Accessibility — the microphone is prompted for by
/// the OS when the device is opened. On Linux (X11) there is nothing to grant,
/// so it is always `true`.
pub fn permissions_granted() -> bool {
    sys::permissions_granted()
}

/// Ask for those permissions, prompting if they are missing.
///
/// Distinct from [`permissions_granted`] because asking has a side effect: on
/// macOS it is what puts the app in the Accessibility list in the first place.
/// Somewhere on the startup path has to call this, or the user is sent to a
/// settings pane that does not list the app they were told to enable.
pub fn request_permissions() -> bool {
    #[cfg(target_os = "macos")]
    {
        sys::request_permissions()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Open the OS settings pane where the user grants the missing permissions.
/// No-op where the concept does not apply.
pub fn open_permission_settings() {
    sys::open_permission_settings();
}

/// Note which app owns the focus, before showing any UI of our own. Injection
/// aims at this app. No-op where showing a window cannot steal focus.
pub fn remember_focus() {
    #[cfg(target_os = "macos")]
    sys::remember_focus();
}
