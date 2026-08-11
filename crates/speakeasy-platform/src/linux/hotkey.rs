//! Push-to-talk on X11.
//!
//! `XGrabKey` cannot grab a bare modifier, so the modifier binding needs the
//! same treatment as macOS: select `XI_RawKeyPress`/`XI_RawKeyRelease` through
//! XInput2 on the root window and watch for the chosen keycode. XInput2 raw
//! events report the physical key, so left and right are distinguishable.
//!
//! Wayland is out of scope; see the README.

use speakeasy_core::{CaptureCallback, Error, Hotkey, HotkeyCallback, HotkeyListener, Result};

#[derive(Default)]
pub struct X11Hotkeys;

impl X11Hotkeys {
    pub fn new() -> Self {
        Self
    }
}

impl HotkeyListener for X11Hotkeys {
    fn register(&self, hotkey: &Hotkey, _on_event: HotkeyCallback) -> Result<()> {
        // TODO: open an x11rb connection, XISelectEvents for raw key press and
        // release on the root window, and map ModifierKey to the keycodes for
        // Alt_L/Alt_R/Control_L/... via the keyboard mapping.
        let _ = hotkey;
        Err(Error::Hotkey(
            "X11 hotkey listening not implemented yet".to_string(),
        ))
    }

    fn unregister(&self) -> Result<()> {
        Ok(())
    }

    /// Nothing to capture with until `register` works. Failing loudly is what
    /// the port contract asks of a capability an implementation lacks — a
    /// capture that silently never reports would leave the settings window
    /// waiting forever.
    fn capture(&self, _on_captured: CaptureCallback) -> Result<()> {
        Err(Error::Hotkey(
            "X11 hotkey capture not implemented yet".to_string(),
        ))
    }

    fn cancel_capture(&self) {}
}
