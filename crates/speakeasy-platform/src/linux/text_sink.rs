//! Text injection on X11 via the XTEST extension.
//!
//! Deliberately X11-only. Wayland restricts synthetic input and would need the
//! RemoteDesktop portal or libei, with support varying by compositor; that is
//! out of scope until someone asks for it.

use speakeasy_core::config::InjectionMode;
use speakeasy_core::{Error, Result, TextSink};

pub struct X11TextSink {
    mode: InjectionMode,
}

impl X11TextSink {
    pub fn new(mode: InjectionMode) -> Self {
        Self { mode }
    }
}

impl TextSink for X11TextSink {
    fn inject(&self, text: &str) -> Result<usize> {
        let _ = (text, self.mode);
        // TODO: connect with x11rb, then for Synthetic remap a spare keycode to
        // each character's keysym and drive xtest_fake_input; for Paste, own the
        // CLIPBOARD selection and fake Ctrl-V.
        Err(Error::Injection(
            "X11 injection not implemented yet".to_string(),
        ))
    }

    fn copy(&self, text: &str) -> Result<()> {
        let _ = text;
        // TODO: own the CLIPBOARD selection with x11rb, alongside the paste
        // path above — the two want the same machinery.
        Err(Error::Injection(
            "X11 clipboard not implemented yet".to_string(),
        ))
    }
}
