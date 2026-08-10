//! Text injection on macOS.
//!
//! Two strategies, both going through CGEvent, and both with real caveats:
//!
//! - [`InjectionMode::Synthetic`] posts key events carrying a unicode string.
//!   Preserves the clipboard, but several apps (notably Electron and some
//!   terminals) drop or reorder characters under fast synthetic input.
//! - [`InjectionMode::Paste`] writes to `NSPasteboard` and posts Cmd-V. Far more
//!   reliable, at the cost of clobbering the clipboard — save and restore the
//!   previous contents, and be aware the restore races with clipboard managers.
//!
//! Both fail while *secure input* is active: a focused password field disables
//! event taps process-wide. That must surface as
//! [`speakeasy_core::Error::SecureInputActive`] rather than silently dropping
//! the user's dictation.

use speakeasy_core::config::InjectionMode;
use speakeasy_core::{Error, Result, TextSink};

pub struct MacTextSink {
    mode: InjectionMode,
}

impl MacTextSink {
    pub fn new(mode: InjectionMode) -> Self {
        Self { mode }
    }

    /// `IsSecureEventInputEnabled()` from Carbon. Check before every injection,
    /// not once at startup — it toggles as the user moves between fields.
    fn secure_input_active() -> bool {
        // TODO: link Carbon and call IsSecureEventInputEnabled().
        false
    }
}

impl TextSink for MacTextSink {
    fn inject(&self, text: &str) -> Result<usize> {
        if Self::secure_input_active() {
            return Err(Error::SecureInputActive);
        }

        match self.mode {
            InjectionMode::Synthetic => {
                // TODO: CGEventCreateKeyboardEvent(None, 0, true) plus
                // CGEventKeyboardSetUnicodeString, chunked to ~20 UTF-16 units
                // per event, posted to CGEventTapLocation::HID.
                Err(Error::Injection(
                    "synthetic injection not implemented yet".to_string(),
                ))
            }
            InjectionMode::Paste => {
                // TODO: save NSPasteboard contents, write `text`, post Cmd-V,
                // then restore after a short delay.
                let _ = text;
                Err(Error::Injection(
                    "paste injection not implemented yet".to_string(),
                ))
            }
        }
    }
}
